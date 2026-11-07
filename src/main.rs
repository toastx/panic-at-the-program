mod ai;
mod app;
mod suite;
#[cfg(test)]
mod tests;
mod ui;
mod workspace;

use std::io;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::prelude::*;

use app::solana;
use suite::tap as rpc_tap;
use ui::run::{Outcome, Run};
use ui::state::{App, Mode, Screen, clusters};

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let signature = args.next();
    if signature.as_deref() == Some("rpc") {
        return rpc_command(&args.collect::<Vec<_>>());
    }
    if signature.as_deref() == Some("ai") {
        return ai_command(&args.collect::<Vec<_>>());
    }

    let suite = (signature.as_deref() == Some("test")).then(prepare_suite);
    let verbose = suite.is_some() && args.any(|arg| arg == "--verbose");

    if !ai::configured() {
        ai::setup()?;
    }

    ui::widgets::load_theme();
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    // Held until the TUI exits: dropping it kills the validator we started.
    let mut _validator = None;
    let mut app = match suite {
        Some((root, framework, command)) => match test_screen(&mut term, root, framework, command, verbose, &mut _validator)? {
            Ok(app) => app,
            Err(report) => {
                leave(&mut term)?;
                // Left in the scrollback, the way a plain `anchor test` would.
                for line in report {
                    eprintln!("{line}");
                }
                return Ok(());
            }
        },
        None => {
            let cluster = args.next().unwrap_or_else(|| "devnet".to_string());
            let mut app = App::new(cluster);
            if let Some(signature) = &signature {
                app.open(signature);
            }
            app
        }
    };

    while !app.quit {
        term.draw(|f| ui::screens::draw(f, &mut app))?;
        // ~60fps while bars grow in; otherwise block on input as before.
        if app.animating() && !event::poll(std::time::Duration::from_millis(16))? {
            continue;
        }
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
        if let Some(prompt) = app.agent_prompt.take() {
            run_agent(&mut term, &mut app, &prompt)?;
        }
    }

    leave(&mut term)
}

fn leave(term: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    term.show_cursor()
}

/// The suite to run, or exit: checks that need no screen, done before there is one.
fn prepare_suite() -> (std::path::PathBuf, suite::Framework, Vec<String>) {
    let root = workspace::project_root().unwrap_or_default();
    let Some((framework, command)) = suite::detect(&root) else {
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
    (root, framework, command)
}

/// What the suite thread tells the run screen.
enum Progress {
    Step(String),
    Note(String),
    Log(String),
    /// Ours to hold, so quitting at any point still shuts it down.
    Validator(suite::Validator),
    Done(Outcome),
}

/// Runs the suite behind the run screen. Ok(app) opens the TUI on the latest
/// failed transaction; Err(report) is what to print once the screen is gone.
fn test_screen(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    root: std::path::PathBuf,
    framework: suite::Framework,
    command: Vec<String>,
    verbose: bool,
    validator: &mut Option<suite::Validator>,
) -> io::Result<Result<App, Vec<String>>> {
    let mut run = Run::new(framework.to_string(), verbose);
    let (sender, progress) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let send = |message| drop(sender.send(message));
        let outcome = run_suite(&root, framework, command, &send);
        send(Progress::Done(outcome));
    });

    loop {
        loop {
            let message = match progress.try_recv() {
                Ok(message) => message,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if run.outcome.is_none() {
                        run.finish(Outcome::Error("the test runner stopped unexpectedly".to_string()));
                    }
                    break;
                }
            };
            match message {
                Progress::Step(text) => run.step(text),
                Progress::Note(text) => run.note(text),
                Progress::Log(line) => run.log(&line),
                Progress::Validator(v) => *validator = Some(v),
                Progress::Done(outcome) => {
                    // Only a transaction still on localnet needs it; give the TUI a while.
                    if let Some(v) = validator.as_ref() {
                        match &outcome {
                            Outcome::Open { cluster: "localnet", .. } => v.expire_after(suite::TTL),
                            _ => v.stop(),
                        }
                    }
                    run.finish(outcome);
                }
            }
        }
        if !verbose && let Some(Outcome::Open { cluster, keys }) = &run.outcome {
            return Ok(Ok(open_failure(cluster, keys)));
        }

        term.draw(|f| ui::run::draw(f, &run))?;
        if !event::poll(std::time::Duration::from_millis(80))? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => key,
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => run.scroll_by(3),
                    MouseEventKind::ScrollDown => run.scroll_by(-3),
                    _ => {}
                }
                continue;
            }
            _ => continue,
        };
        let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(event::KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            _ if ctrl_c => break,
            KeyCode::Enter => {
                if let Some(Outcome::Open { cluster, keys }) = &run.outcome {
                    return Ok(Ok(open_failure(cluster, keys)));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => run.scroll_by(1),
            KeyCode::Down | KeyCode::Char('j') => run.scroll_by(-1),
            KeyCode::PageUp => run.scroll_by(20),
            KeyCode::PageDown => run.scroll_by(-20),
            KeyCode::Char('g') => run.scroll_by(isize::MAX),
            KeyCode::Char('G') => run.scroll = 0,
            _ => {}
        }
    }

    suite::abort();
    Ok(Err(match run.outcome {
        None => vec!["aborted".to_string()],
        Some(Outcome::Open { .. }) => Vec::new(),
        Some(Outcome::Error(err)) => vec![err],
        Some(Outcome::Nothing(text)) => run.logs.into_iter().chain(text).collect(),
    }))
}

fn open_failure(cluster: &str, keys: &[String]) -> App {
    let mut app = App::new(cluster.to_string());
    if let Some(key) = keys.last() {
        app.open_transaction(key);
    }
    app
}

/// Runs the project's suite and finds the failed transactions in what it did.
fn run_suite(
    root: &std::path::Path,
    framework: suite::Framework,
    mut command: Vec<String>,
    send: &(dyn Fn(Progress) + Sync),
) -> Outcome {
    let note = |text: String| send(Progress::Note(text));
    let mut have_validator = false;
    if framework == suite::Framework::Anchor {
        send(Progress::Step("starting solana-test-validator".to_string()));
        match suite::Validator::start(root) {
            Ok(Some(validator)) => {
                note(match validator.reused() {
                    true => "reusing the validator already serving localnet".to_string(),
                    false => "validator up on localnet".to_string(),
                });
                send(Progress::Validator(validator));
                have_validator = true;
            }
            Ok(None) => {
                return Outcome::Error(
                    "solana-test-validator not on PATH — install it to keep failed transactions fetchable".to_string(),
                );
            }
            Err(err) => return Outcome::Error(err.to_string()),
        }
    }

    // With the suite pointed at the tap we hear what the validator told it. Its
    // own output is a lossy witness: anchor's client throws the simulation result
    // away and mocha prints a sentence.
    let tap = have_validator
        .then(|| rpc_tap::upstreams(&solana::cluster_url("localnet")))
        .flatten()
        .and_then(|(http, ws)| rpc_tap::Tap::start(&http, &ws).ok());
    match (&tap, command.iter().position(|arg| arg == suite::CLUSTER_ARG)) {
        (Some(tap), Some(at)) => {
            command[at] = tap.url();
            note(format!("rpc tap on {}", tap.url()));
        }
        // Without the tap the suite talks to the validator directly and we are
        // back to scraping its output.
        _ => note("no rpc tap — falling back to the suite's own output".to_string()),
    }

    send(Progress::Step(format!("{framework}: {}", command.join(" "))));
    note(format!("in {}", root.display()));
    let output = match suite::run(root, &command, &|line| send(Progress::Log(line.to_string()))) {
        Ok(output) => output,
        Err(err) => return Outcome::Error(format!("could not run `{}`: {err}", command[0])),
    };

    send(Progress::Step("looking for failed transactions".to_string()));
    // Our own validator is still up, so prefer the real transaction over the one
    // reconstructed from log lines: it carries accounts, balances and state diff.
    let (cluster, keys) = match have_validator {
        true => ("localnet", suite::failed_signatures(&output, "localnet")),
        false => (suite::CLUSTER, Vec::new()),
    };
    let (cluster, keys) = if keys.is_empty() {
        // Best evidence first: what the validator said, then program logs the
        // suite happened to print, then the error text as a last resort.
        let mut cases = tap
            .as_ref()
            .map(|tap| suite::captured(&tap.failures()))
            .unwrap_or_default();
        let mut source = "the rpc tap";
        if have_validator {
            // Still up, so the payer's balance is still readable.
            suite::add_payer_balance(&mut cases, "localnet");
        }
        if cases.is_empty() {
            cases = suite::cases(&output);
            source = "program logs in the suite output";
        }
        if !cases.is_empty() {
            note(format!("{} case(s) from {source}", cases.len()));
        }
        (suite::CLUSTER, suite::stash(&cases))
    } else {
        (cluster, keys)
    };

    if !keys.is_empty() {
        return Outcome::Open { cluster, keys };
    }
    // A build that fails never reaches the tests, so there is nothing to
    // diagnose for a reason the generic message would hide.
    Outcome::Nothing(match suite::setup_error(&output) {
        Some(setup) => {
            let mut text = vec![match setup.stage {
                suite::Stage::Compile => {
                    let plural = if setup.count == 1 { "error" } else { "errors" };
                    format!("the build failed with {} {plural} — the suite never ran", setup.count)
                }
                suite::Stage::Startup => "the suite stopped before any test ran".to_string(),
            }];
            text.push(format!("  {}", setup.headline));
            text.extend(setup.location.as_ref().map(|location| format!("  at {location}")));
            text
        }
        None => vec!["no failed transaction in the output — nothing to diagnose".to_string()],
    })
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

fn ai_command(args: &[String]) -> io::Result<()> {
    match args {
        [on] if on == "on" => workspace::set_global("ai", "enabled", "true")?,
        [off] if off == "off" => workspace::set_global("ai", "enabled", "false")?,
        // Joined, so `panic ai agent codex exec` needs no quoting.
        [agent, command @ ..] if agent == "agent" && !command.is_empty() => {
            workspace::set_global("ai", "enabled", "true")?;
            workspace::set_global("ai", "agent", &command.join(" "))?;
        }
        [] => {
            match ai::agent() {
                Some(agent) => println!("on, agent {agent}"),
                None => println!("off"),
            }
            return Ok(());
        }
        _ => {
            eprintln!(
                "usage: panic ai                     show the current setting\n       panic ai on | off            enable or disable the export\n       panic ai agent <command>     the agent to hand it to (claude, codex, ...)"
            );
            std::process::exit(2);
        }
    }
    println!("saved in {}", workspace::global_path().display());
    Ok(())
}

/// Lets the agent have the terminal, then takes it back. Spawning a window
/// instead would be a guess at which emulator is running.
fn run_agent(term: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App, prompt: &str) -> io::Result<()> {
    let Some(agent) = ai::agent() else { return Ok(()) };
    if !suite::available(ai::program(&agent)) {
        app.message = format!("`{}` not on PATH", ai::program(&agent));
        return Ok(());
    }
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    let result = ai::send(&agent, prompt);
    enable_raw_mode()?;
    execute!(term.backend_mut(), EnterAlternateScreen, EnableMouseCapture)?;
    term.clear()?;
    app.message = match result {
        Ok(()) => format!("{agent} exited"),
        Err(err) => format!("{agent}: {err}"),
    };
    Ok(())
}

/// The markdown for what is loaded, when AI export is on and it is a transaction.
fn ai_context(app: &App) -> Option<String> {
    ai::agent()?;
    let tx = app.tx().filter(|_| app.kind == solana::Kind::Tx)?;
    Some(ai::markdown(tx))
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
        KeyCode::Char('t') => {
            if let Err(err) = ui::widgets::cycle_theme() {
                app.message = format!("theme not saved: {err}");
            }
        }
        KeyCode::Char('y') => {
            if let Some(text) = ai_context(app) {
                app.message = match ai::copy(&text) {
                    Ok(()) => format!("copied diagnosis ({} lines)", text.lines().count()),
                    Err(err) => format!("clipboard: {err}"),
                };
            }
        }
        KeyCode::Char('a') => app.agent_prompt = ai_context(app),
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
