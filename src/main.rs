mod app;
mod screens;
mod solana;
mod ui;

use std::io;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, MouseButton,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::prelude::*;

use app::{App, CLUSTERS, Mode, Screen};
#[cfg(test)]
use app::Click;

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
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    while !app.quit {
        term.draw(|f| screens::draw(f, &mut app))?;
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

    if app.cluster_menu {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                app.cluster_index = (app.cluster_index + 1) % CLUSTERS.len();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.cluster_index = (app.cluster_index + CLUSTERS.len() - 1) % CLUSTERS.len();
            }
            KeyCode::Enter => {
                let cluster = CLUSTERS[app.cluster_index];
                app.set_cluster(cluster);
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
            app.mode = Mode::Normal;
            app.search.clear();
            app.message.clear();
        }
        KeyCode::Tab => app.cycle_screen(1),
        KeyCode::BackTab => app.cycle_screen(-1),
        KeyCode::Char('w') => app.focus_nav = !app.focus_nav,
        KeyCode::Char('x') => app.open_in_explorer(),
        KeyCode::Char('c') => app.toggle_cluster_menu(),
        KeyCode::Char(c @ '1'..='6') => {
            let index = c as usize - '1' as usize;
            if let Some(screen) = app.screens().get(index).copied() {
                app.goto(screen);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.focus_nav {
                app.entry_index = (app.entry_index + 1).min(app.entries.len().saturating_sub(1));
            } else {
                move_selection(app, 1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.focus_nav {
                app.entry_index = app.entry_index.saturating_sub(1);
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
        KeyCode::Enter => {
            if app.focus_nav {
                app.open_entry(app.entry_index);
            } else {
                let collapsed = app.selected_node().is_some_and(|i| app.collapsed.contains(&i));
                toggle_collapse(app, !collapsed);
            }
        }
        KeyCode::Char('r') => {
            if app.kind == solana::Kind::Account {
                if let Some(key) = app.account.as_ref().map(|a| a.key.clone()) {
                    match solana::refetch_account(&app.cluster, &key) {
                        Ok(value) => app.account = Some(solana::parse_account(&key, &value)),
                        Err(err) => app.message = err.to_string(),
                    }
                }
            } else if let Some(signature) = app.tx().map(|tx| tx.signature.clone()) {
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
    use solana::Status;

    fn account_fixture() -> serde_json::Value {
        let mut data = vec![0u8; 82];
        data[0..4].copy_from_slice(&1u32.to_le_bytes());
        data[36..44].copy_from_slice(&21_000_000_000_000u64.to_le_bytes());
        data[44] = 6;
        data[45] = 1;
        let encoded = base64_encode(&data);
        json!({"result": {"value": {
            "lamports": 2039280u64,
            "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
            "executable": false,
            "rentEpoch": u64::MAX,
            "data": [encoded, "base64"]
        }}})
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[((n >> (18 - i * 6)) & 0x3f) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

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
        app.kind = solana::Kind::Tx;
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
        on_key(&mut app, KeyEvent::from(KeyCode::Char('4')));
        assert!(render(&mut app, 120, 34).contains("STATE DIFF"));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('5')));
        assert!(render(&mut app, 120, 34).contains("PROGRAM LOGS"));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('o')));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('7')));
        assert!(render(&mut app, 120, 34).contains("signature › 7"));
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
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn cluster_dropdown_opens_on_click_and_switches_network() {
        let mut app = loaded();
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("● devnet ▾"));
        assert!(!screen.contains("NETWORK"));

        let button = app
            .hits
            .iter()
            .find(|hit| hit.click == Click::ClusterMenu)
            .map(|hit| (hit.area.x, hit.area.y))
            .expect("cluster button is clickable");
        app.click(button.0, button.1);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("NETWORK"));
        assert!(screen.contains("testnet"));

        let item = app
            .hits
            .iter()
            .find(|hit| hit.click == Click::Cluster("testnet"))
            .map(|hit| (hit.area.x, hit.area.y))
            .expect("dropdown item is clickable");
        app.click(item.0, item.1);
        assert_eq!(app.cluster, "testnet");
        assert!(!app.cluster_menu);
        assert!(render(&mut app, 120, 34).contains("● testnet ▾"));
    }

    #[test]
    fn cluster_dropdown_is_keyboard_driven() {
        let mut app = loaded();
        on_key(&mut app, KeyEvent::from(KeyCode::Char('c')));
        assert!(app.cluster_menu);
        on_key(&mut app, KeyEvent::from(KeyCode::Down));
        on_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.cluster, "testnet");

        on_key(&mut app, KeyEvent::from(KeyCode::Char('c')));
        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(!app.cluster_menu);
        assert_eq!(app.cluster, "testnet");
    }

    #[test]
    fn clicking_a_tab_switches_screen() {
        let mut app = loaded();
        render(&mut app, 120, 34);
        let tab = app
            .hits
            .iter()
            .find(|hit| hit.click == Click::Workspace(Screen::Logs))
            .map(|hit| (hit.area.x, hit.area.y))
            .expect("tab is clickable");
        app.click(tab.0, tab.1);
        assert!(render(&mut app, 120, 34).contains("PROGRAM LOGS"));
    }

    #[test]
    fn tabs_follow_the_loaded_entity_kind() {
        let mut app = loaded();
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("1 Overview"));
        assert!(screen.contains("5 Logs"));
        assert!(!screen.contains("Info"));

        app.account = Some(solana::parse_account("8tK4iuyx", &account_fixture()));
        app.kind = solana::Kind::Account;
        app.goto(Screen::Info);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("1 Info"));
        assert!(screen.contains("2 Data"));
        assert!(screen.contains("3 Raw"));
        assert!(!screen.contains("Execution"));
    }

    #[test]
    fn sidebar_lists_entities_by_kind() {
        let mut app = loaded();
        app.entries = vec![
            solana::Entry {
                kind: solana::Kind::Tx,
                key: "4GbWN1uNx2TH78dCtrZszmja1Jg6DW9F426S7QPt5uRa".into(),
                status: Some(Status::Failed),
                detail: " 6 ix  114,678 CU".into(),
            },
            solana::Entry {
                kind: solana::Kind::Account,
                key: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
                status: None,
                detail: "   82 B  Token Program".into(),
            },
        ];
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("tx:4GbW…5uRa"));
        assert!(screen.contains("acc:EPjF…Dt1v"));

        let entry = app
            .hits
            .iter()
            .find(|hit| hit.click == Click::Entry(1))
            .map(|hit| (hit.area.x, hit.area.y))
            .expect("entry row is clickable");
        assert!(entry.1 > 0);
    }

    #[test]
    fn explorer_urls_follow_the_cluster() {
        let mut app = loaded();
        let (kind, key) = app.explorer_target().unwrap();
        assert_eq!(kind, "tx");
        assert_eq!(
            solana::explorer_url(&app.cluster, &kind, &key),
            "https://explorer.solana.com/tx/4vK9mP?cluster=devnet"
        );
        app.set_cluster("mainnet");
        assert_eq!(
            solana::explorer_url("mainnet", "tx", &key),
            "https://explorer.solana.com/tx/4vK9mP"
        );
        assert!(solana::explorer_url("localnet", "address", "abc").contains("customUrl=http%3A"));
    }

    #[test]
    fn account_screens_show_parsed_fields_and_raw_bytes() {
        let mut app = App::new("devnet".into());
        app.account = Some(solana::parse_account("8tK4iuyxABCD", &account_fixture()));
        app.kind = solana::Kind::Account;
        app.goto(Screen::Info);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("0.002039280 SOL"));
        assert!(screen.contains("Token Program"));
        assert!(screen.contains("82 bytes"));
        assert!(screen.contains("SPL Mint"));
        assert!(screen.contains("decimals"));

        app.goto(Screen::Data);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("PARSED — SPL Mint"));
        assert!(screen.contains("RAW (hex)"));
        assert!(screen.contains("00000000"));
    }

    #[test]
    fn instruction_data_is_shown_parsed_and_raw() {
        let transfer = solana::bs58_encode(&[3, 64, 66, 15, 0, 0, 0, 0, 0]);
        let decoded = solana::decode_instruction("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", &transfer);
        let (name, fields) = decoded.expect("token transfer decodes");
        assert_eq!(name, "Transfer");
        assert_eq!(fields[1], ("amount".to_string(), "1000000".to_string()));
        assert_eq!(solana::bs58_decode(&transfer), vec![3, 64, 66, 15, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn clicking_an_account_row_targets_inspection() {
        let mut app = loaded();
        app.goto(Screen::Accounts);
        render(&mut app, 120, 34);
        let inspect = app.hits.iter().filter_map(|hit| match &hit.click {
            Click::Inspect(key) => Some(key.clone()),
            _ => None,
        });
        assert!(inspect.count() >= 3);
    }

    #[test]
    #[ignore = "network"]
    fn fetches_and_caches_from_mainnet() {
        let signature = "32A7NyijRbsu3x6DVt1aKJwhPo27PHWSqgxKe54n2xdjUS68XKMkbhnYivyEjnWZ4gU5TxjgRVQcGLmeh6oP8YhX";
        let value = solana::fetch("mainnet", signature).unwrap();
        let tx = solana::parse(signature, "mainnet", &value);
        assert!(!tx.nodes.is_empty());
        assert!(tx.cu > 0);
        assert!(solana::cached("mainnet").contains(&signature.to_string()));
    }
}
