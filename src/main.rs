mod app;
mod chain;
mod suite;
#[cfg(test)]
mod tests;
mod workspace;

use std::io;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::prelude::*;

use app::state::{App, Mode, Screen, clusters};
use chain::solana;
use suite::tap as rpc_tap;

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let signature = args.next();
    if signature.as_deref() == Some("rpc") {
        return rpc_command(&args.collect::<Vec<_>>());
    }

    // Held until the TUI exits: dropping it kills the validator we started.
    let mut _validator = None;
    let mut app = if signature.as_deref() == Some("test") {
        match test_command(&mut _validator)? {
            Some(app) => app,
            None => return Ok(()),
        }
    } else {
        let cluster = args.next().unwrap_or_else(|| "devnet".to_string());
        let mut app = App::new(cluster);
        if let Some(signature) = &signature {
            app.open(signature);
        }
        app
    };

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    while !app.quit {
        term.draw(|f| app::screens::draw(f, &mut app))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => on_key(&mut app, key),
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => app.click(mouse.column, mouse.row),
                MouseEventKind::ScrollDown => move_selection(&mut app, 1),
                MouseEventKind::ScrollUp => move_selection(&mut app, -1),
                _ => {}
            },
            _ => {}
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    term.show_cursor()
}

/// Runs the project's suite and opens the TUI on the first failed transaction
/// found in its output. None means there was nothing to open.
fn test_command(validator: &mut Option<suite::Validator>) -> io::Result<Option<App>> {
    let root = workspace::project_root().unwrap_or_default();
    let Some((framework, mut command)) = suite::detect(&root) else {
        eprintln!(
            "no test suite found in {} (looked for Anchor.toml, Cargo.toml)",
            root.display()
        );
        std::process::exit(2);
    };
    if !suite::available(&command[0]) {
        eprintln!(
            "`{}` not on PATH — is it installed where you are running panic?",
            command[0]
        );
        if cfg!(windows) {
            eprintln!("anchor is usually a WSL install: run panic inside WSL, from /mnt/<drive>/...");
        }
        std::process::exit(2);
    }
    if framework == "anchor" {
        *validator = suite::Validator::start(&root)?;
        if validator.is_none() {
            eprintln!("solana-test-validator not on PATH — install it to keep failed transactions fetchable");
            return Ok(None);
        }
        eprintln!("validator up on localnet");
    }

    // With the suite pointed at the tap we hear what the validator told it. Its
    // own output is a lossy witness: anchor's client throws the simulation result
    // away and mocha prints a sentence.
    let tap = validator
        .as_ref()
        .and_then(|_| rpc_tap::upstreams(&solana::cluster_url("localnet")))
        .and_then(|(http, ws)| rpc_tap::Tap::start(&http, &ws).ok());
    match (&tap, command.iter().position(|arg| arg == suite::CLUSTER_ARG)) {
        (Some(tap), Some(at)) => {
            command[at] = tap.url();
            eprintln!("rpc tap on {}", tap.url());
        }
        // Without the tap the suite talks to the validator directly and we are
        // back to scraping its output.
        _ => eprintln!("no rpc tap — falling back to the suite's own output"),
    }
    eprintln!("{framework}: {} (in {})", command.join(" "), root.display());

    let output = suite::run(&root, &command)
        .map_err(|err| io::Error::new(err.kind(), format!("could not run `{}`: {err}", command[0])))?;
    // Our own validator is still up, so prefer the real transaction over the one
    // reconstructed from log lines: it carries accounts, balances and state diff.
    let (cluster, keys) = match validator {
        Some(_) => ("localnet", suite::failed_signatures(&output, "localnet")),
        None => (suite::CLUSTER, Vec::new()),
    };
    let (cluster, keys) = if keys.is_empty() {
        // Best evidence first: what the validator said, then program logs the
        // suite happened to print, then the error text as a last resort.
        let mut cases = tap
            .as_ref()
            .map(|tap| suite::captured(&tap.failures()))
            .unwrap_or_default();
        let mut source = "the rpc tap";
        if validator.is_some() {
            // Still up, so the payer's balance is still readable.
            suite::add_payer_balance(&mut cases, "localnet");
        }
        if cases.is_empty() {
            cases = suite::cases(&output);
            source = "program logs in the suite output";
        }
        if !cases.is_empty() {
            eprintln!("{} case(s) from {source}", cases.len());
        }
        (suite::CLUSTER, suite::stash(&cases))
    } else {
        (cluster, keys)
    };

    // Nothing more will be fetched off localnet once we fall back to the cases
    // rebuilt from log lines, so the validator has no reason to stay up.
    if let Some(validator) = validator.as_ref() {
        if cluster == suite::CLUSTER {
            validator.stop();
        } else {
            validator.expire_after(suite::TTL);
        }
    }

    let Some(key) = keys.last().cloned() else {
        print!("{output}");
        eprintln!("no failed transaction in the output — nothing to diagnose");
        return Ok(None);
    };
    eprintln!("{} failed transaction(s) on {cluster}", keys.len());

    let mut app = App::new(cluster.to_string());
    app.open_transaction(&key);
    Ok(Some(app))
}

fn rpc_command(args: &[String]) -> io::Result<()> {
    match args {
        [add, name, url] if add == "add" => {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                eprintln!("rpc url must start with http:// or https://");
                std::process::exit(2);
            }
            workspace::set_global("rpc", name, url)?;
            println!("saved {name} -> {url} in {}", workspace::global_path().display());
        }
        [rm, name] if rm == "rm" => {
            if !workspace::remove_global("rpc", name)? {
                eprintln!("no custom rpc named {name}");
                std::process::exit(1);
            }
            println!("removed {name}");
        }
        [] => {
            for name in clusters() {
                println!("{name:<12} {}", solana::cluster_url(&name));
            }
        }
        _ => {
            eprintln!(
                "usage: panic rpc                    list clusters\n       panic rpc add <name> <url>  save a custom endpoint\n       panic rpc rm <name>         remove a custom endpoint"
            );
            std::process::exit(2);
        }
    }
    Ok(())
}

fn move_selection(app: &mut App, delta: isize) {
    if app.screen == Screen::Raw {
        app.scroll = app.scroll.saturating_add_signed(delta);
        return;
    }
    let rows = app.rows();
    if rows == 0 {
        return;
    }
    let current = app.selected_row() as isize;
    app.list
        .select(Some(current.saturating_add(delta).clamp(0, rows as isize - 1) as usize));
}

fn grid_step(app: &App) -> isize {
    if app.screen == Screen::hub(app.kind) {
        app.section_columns.max(1) as isize
    } else {
        1
    }
}

fn step_back(app: &mut App) {
    if app.back() {
        return;
    }
    if !app.focus_nav {
        app.focus_nav = true;
    } else {
        app.back_to_entities();
    }
}

fn toggle_collapse(app: &mut App, collapse: bool) -> bool {
    let Some(index) = app.selected_node() else { return false };
    let has_children = app.tx().is_some_and(|tx| tx.nodes[index].children > 0);

    if collapse && (!has_children || app.collapsed.contains(&index)) {
        let Some(parent) = app.parent_node(index) else {
            return false;
        };
        app.collapsed.insert(parent);
        if let Some(row) = app.visible_nodes().iter().position(|&i| i == parent) {
            app.list.select(Some(row));
        }
        return true;
    }
    if !has_children {
        return false;
    }
    if collapse {
        app.collapsed.insert(index)
    } else {
        app.collapsed.remove(&index)
    }
}

pub fn on_key(app: &mut App, key: KeyEvent) {
    match &mut app.mode {
        Mode::Help => {
            app.mode = Mode::Normal;
            return;
        }
        Mode::Open(buffer) => {
            match key.code {
                KeyCode::Char(c) => buffer.push(c),
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Enter => {
                    let input = buffer.trim().to_string();
                    app.mode = Mode::Normal;
                    if !input.is_empty() {
                        app.open(&input);
                    }
                }
                _ => {}
            }
            return;
        }
        Mode::Search => {
            match key.code {
                KeyCode::Char(c) => app.search.push(c),
                KeyCode::Backspace => {
                    app.search.pop();
                }
                KeyCode::Esc => {
                    app.search.clear();
                    app.mode = Mode::Normal;
                }
                KeyCode::Enter => app.mode = Mode::Normal,
                _ => {}
            }
            app.list.select(Some(0));
            app.jump_to_match();
            return;
        }
        Mode::Normal => {}
    }

    if app.cluster_menu {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                app.cluster_index = (app.cluster_index + 1) % clusters().len();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let count = clusters().len();
                app.cluster_index = (app.cluster_index + count - 1) % count;
            }
            KeyCode::Enter => {
                let cluster = clusters()[app.cluster_index].clone();
                app.set_cluster(&cluster);
            }
            KeyCode::Char('q') => app.quit = true,
            _ => app.cluster_menu = false,
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('o') => app.mode = Mode::Open(String::new()),
        KeyCode::Char('/') => {
            app.mode = Mode::Search;
            app.search.clear();
        }
        KeyCode::Char('n') => app.jump_to_match(),
        KeyCode::Esc => {
            let dismissed = !app.search.is_empty() || !app.message.is_empty();
            app.mode = Mode::Normal;
            app.search.clear();
            app.message.clear();
            if !dismissed {
                step_back(app);
            }
        }
        KeyCode::Tab => app.focus_nav = !app.focus_nav,
        KeyCode::BackTab => app.cycle_screen(-1),
        KeyCode::Char('x') => app.open_in_explorer(),
        KeyCode::Char('c') => app.toggle_cluster_menu(),
        KeyCode::Char(']') => app.step_cluster(1),
        KeyCode::Char('[') => app.step_cluster(-1),
        KeyCode::Backspace | KeyCode::Char('b') => step_back(app),
        KeyCode::Down | KeyCode::Char('j') => {
            if app.focus_nav {
                app.entry_index = (app.entry_index + 1).min(app.entries.len().saturating_sub(1));
            } else {
                move_selection(app, grid_step(app));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.focus_nav {
                app.entry_index = app.entry_index.saturating_sub(1);
            } else {
                move_selection(app, -grid_step(app));
            }
        }
        KeyCode::PageDown => move_selection(app, 10),
        KeyCode::PageUp => move_selection(app, -10),
        KeyCode::Char('g') => move_selection(app, isize::MIN / 2),
        KeyCode::Char('G') => move_selection(app, isize::MAX / 2),
        KeyCode::Right | KeyCode::Char('l') => {
            if app.focus_nav {
                app.focus_nav = false;
            } else if app.screen == Screen::hub(app.kind) {
                move_selection(app, 1);
            } else {
                toggle_collapse(app, false);
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if app.focus_nav {
            } else if app.screen == Screen::hub(app.kind) {
                move_selection(app, -1);
            } else if !toggle_collapse(app, true) {
                app.focus_nav = true;
            }
        }
        KeyCode::Enter => match (app.focus_nav, app.screen) {
            (true, _) => app.open_entry(app.entry_index),
            (_, Screen::Overview | Screen::Info) => {
                app.open_section();
                app.focus_nav = false;
            }
            _ => match app.screen {
                Screen::Execution => app.inspect_invocation(),
                Screen::Flow => {
                    if let Some(node) = app.selected_transfer_node() {
                        app.node_index = Some(node);
                        app.goto(Screen::Invocation);
                    }
                }
                _ => {
                    if let Some(key) = app.selected_account() {
                        app.drill_into_account(&key);
                    }
                }
            },
        },
        KeyCode::Char('r') => {
            if let Some(key) = app.loaded_key() {
                let (kind, screen) = (app.kind, app.screen);
                let cache_key = if kind == solana::Kind::Account {
                    format!("account-{key}")
                } else {
                    key.clone()
                };
                solana::evict(&app.cluster, &cache_key);
                app.load(kind, &key);
                if app.message.is_empty() {
                    app.goto(screen);
                }
            }
        }
        _ => {}
    }
}
