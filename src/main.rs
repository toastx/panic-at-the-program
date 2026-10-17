mod app;
mod screens;
mod solana;
mod ui;

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::prelude::*;

use app::{App, Mode, Screen};

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let signature = args.next();
    let cluster = args.next().unwrap_or_else(|| "devnet".to_string());

    let mut app = App::new(cluster);
    if let Some(signature) = &signature {
        app.open(signature);
    }

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    while !app.quit {
        term.draw(|f| screens::draw(f, &mut app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                on_key(&mut app, key);
            }
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()
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
    let current = app.list.selected().unwrap_or(0) as isize;
    app.list.select(Some(current.saturating_add(delta).clamp(0, rows as isize - 1) as usize));
}

fn toggle_collapse(app: &mut App, collapse: bool) -> bool {
    let Some(index) = app.selected_node() else { return false };
    let has_children = app.tx().is_some_and(|tx| tx.nodes[index].children > 0);
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
            app.mode = Mode::Normal;
            app.search.clear();
            app.message.clear();
        }
        KeyCode::Tab => app.focus_nav = !app.focus_nav,
        KeyCode::Char(c @ '1'..='7') => {
            let index = c as usize - '1' as usize;
            app.goto(Screen::ALL[index]);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.focus_nav {
                app.nav_index = (app.nav_index + 1).min(Screen::ALL.len() - 1);
                app.goto(Screen::ALL[app.nav_index]);
                app.focus_nav = true;
            } else {
                move_selection(app, 1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.focus_nav {
                app.nav_index = app.nav_index.saturating_sub(1);
                app.goto(Screen::ALL[app.nav_index]);
                app.focus_nav = true;
            } else {
                move_selection(app, -1);
            }
        }
        KeyCode::PageDown => move_selection(app, 10),
        KeyCode::PageUp => move_selection(app, -10),
        KeyCode::Char('g') => move_selection(app, isize::MIN / 2),
        KeyCode::Char('G') => move_selection(app, isize::MAX / 2),
        KeyCode::Right | KeyCode::Char('l') => {
            if app.focus_nav {
                app.focus_nav = false;
            } else {
                toggle_collapse(app, false);
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if !app.focus_nav && !toggle_collapse(app, true) {
                app.focus_nav = true;
            }
        }
        KeyCode::Enter => match app.screen {
            Screen::History => {
                let rows = app.history_rows();
                if let Some(signature) = app
                    .list
                    .selected()
                    .and_then(|i| rows.get(i))
                    .and_then(|&i| app.history.get(i))
                    .map(|s| s.signature.clone())
                {
                    app.open(&signature);
                }
            }
            _ => {
                let collapsed = app.selected_node().is_some_and(|i| app.collapsed.contains(&i));
                toggle_collapse(app, !collapsed);
            }
        },
        KeyCode::Char('r') => {
            if let Some(signature) = app.tx().map(|tx| tx.signature.clone()) {
                match solana::refetch(&app.cluster, &signature) {
                    Ok(value) => app.tx = Some(solana::parse(&signature, &app.cluster, &value)),
                    Err(err) => app.message = err.to_string(),
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    fn fixture() -> serde_json::Value {
        json!({"result": {
            "slot": 312441221u64,
            "meta": {
                "err": {"InstructionError": [0, "Custom"]},
                "fee": 10000,
                "computeUnitsConsumed": 12620,
                "preBalances": [1000000000u64, 0, 0, 0],
                "postBalances": [999990000u64, 0, 0, 0],
                "innerInstructions": [{"index": 0, "instructions": [{"programIdIndex": 3, "accounts": [0, 2], "data": "3Bxs"}]}],
                "logMessages": [
                    "Program Vau1t11111111111111111111111111111111111 invoke [1]",
                    "Program log: withdraw",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [2]",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA consumed 8420 of 200000 compute units",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA failed: custom program error: 0x1",
                    "Program Vau1t11111111111111111111111111111111111 consumed 12620 of 200000 compute units",
                    "Program Vau1t11111111111111111111111111111111111 failed: custom program error: 0x1"
                ]
            },
            "transaction": {"message": {
                "header": {"numRequiredSignatures": 1, "numReadonlySignedAccounts": 0, "numReadonlyUnsignedAccounts": 1},
                "accountKeys": ["Auth1111", "Vau1t111", "TokenAcct1111", "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"],
                "instructions": [{"programIdIndex": 3, "accounts": [0, 1, 2], "data": "9xQe"}]
            }}
        }})
    }

    fn loaded() -> App {
        let mut app = App::new("devnet".into());
        app.tx = Some(solana::parse("4vK9mP", "devnet", &fixture()));
        app.goto(Screen::Execution);
        app
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| screens::draw(f, app)).unwrap();
        let buffer = term.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn splits_self_cost_from_inclusive_cost() {
        let tx = solana::parse("4vK9mP", "devnet", &fixture());
        assert_eq!(tx.nodes[0].cu, 12620);
        assert_eq!(tx.nodes[0].self_cu, 12620 - 8420);
        assert_eq!(tx.nodes[0].children, 1);
        assert_eq!(tx.nodes[1].self_cu, 8420);
        assert_eq!(tx.failed_node, Some(1));
        assert_eq!(tx.error.as_deref(), Some("custom program error: 0x1"));
        assert_eq!(tx.accounts[0].role(), "signer, writable");
        assert_eq!(tx.accounts[3].role(), "readonly");
        assert_eq!(tx.balances.len(), 1);
    }

    #[test]
    fn execution_screen_shows_tree_error_and_chart() {
        let mut app = loaded();
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("EXECUTION FLOW"));
        assert!(screen.contains("└─"));
        assert!(screen.contains("custom program error: 0x1"));
        assert!(screen.contains("SELF COST"));
        assert!(screen.contains("TOTAL (inclusive)"));
        assert!(screen.contains("signer, writable"));
    }

    #[test]
    fn collapsing_hides_nested_calls() {
        let mut app = loaded();
        assert_eq!(app.visible_nodes().len(), 2);
        on_key(&mut app, KeyEvent::from(KeyCode::Left));
        assert_eq!(app.visible_nodes().len(), 1);
        assert!(render(&mut app, 120, 34).contains("(+1)"));
        on_key(&mut app, KeyEvent::from(KeyCode::Right));
        assert_eq!(app.visible_nodes().len(), 2);
    }

    #[test]
    fn navigation_switches_workspaces_and_modes() {
        let mut app = loaded();
        on_key(&mut app, KeyEvent::from(KeyCode::Char('5')));
        assert!(render(&mut app, 120, 34).contains("STATE DIFF"));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('6')));
        assert!(render(&mut app, 120, 34).contains("PROGRAM LOGS"));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('o')));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('4')));
        assert!(render(&mut app, 120, 34).contains("signature › 4"));
        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('?')));
        assert!(render(&mut app, 120, 34).contains("expand / collapse"));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('q')));
        assert!(!app.quit);
    }

    #[test]
    fn narrow_terminal_drops_side_panels() {
        let mut app = loaded();
        let screen = render(&mut app, 80, 24);
        assert!(!screen.contains("SELECTED INSTRUCTION"));
        assert!(screen.contains("EXECUTION FLOW"));
    }

    #[test]
    fn detail_panel_shows_selected_instruction() {
        let mut app = loaded();
        on_key(&mut app, KeyEvent::from(KeyCode::Down));
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("SELECTED INSTRUCTION"));
        assert!(screen.contains("8,420 self"));
        assert!(screen.contains("Failed"));
    }

    #[test]
    fn search_filters_rows_and_fades_the_tree() {
        let mut app = loaded();
        app.goto(Screen::Logs);
        let all = app.rows();
        on_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        for c in "token".chars() {
            on_key(&mut app, KeyEvent::from(KeyCode::Char(c)));
        }
        assert!(app.rows() < all && app.rows() > 0);
        assert!(render(&mut app, 120, 34).contains("match(es)"));

        on_key(&mut app, KeyEvent::from(KeyCode::Enter));
        app.goto(Screen::Execution);
        app.jump_to_match();
        assert_eq!(app.selected_node(), Some(1));

        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(app.search.is_empty());
        assert_eq!(app.rows(), 2);
    }

    #[test]
    fn open_prompt_rejects_blank_input() {
        let mut app = loaded();
        on_key(&mut app, KeyEvent::from(KeyCode::Char('o')));
        on_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(app.message.is_empty());
        assert!(app.tx.is_some());
    }

    #[test]
    #[ignore = "network"]
    fn fetches_and_caches_from_mainnet() {
        let signature = "32A7NyijRbsu3x6DVt1aKJwhPo27PHWSqgxKe54n2xdjUS68XKMkbhnYivyEjnWZ4gU5TxjgRVQcGLmeh6oP8YhX";
        let value = solana::fetch("mainnet", signature).unwrap();
        let tx = solana::parse(signature, "mainnet", &value);
        assert!(!tx.nodes.is_empty());
        assert!(tx.cu > 0);
        assert!(solana::cached().contains(&signature.to_string()));
    }
}
