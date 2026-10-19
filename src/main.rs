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
    let current = app.selected_row() as isize;
    app.list.select(Some(current.saturating_add(delta).clamp(0, rows as isize - 1) as usize));
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
        let Some(parent) = app.parent_node(index) else { return false };
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

    fn transfer_fixture() -> serde_json::Value {
        let mut system = vec![2u8, 0, 0, 0];
        system.extend_from_slice(&1_500_000_000u64.to_le_bytes());
        let mut token = vec![12u8];
        token.extend_from_slice(&2_500_000u64.to_le_bytes());
        token.push(6);
        let system_data = solana::bs58_encode(&system);
        let token_data = solana::bs58_encode(&token);
        json!({"result": {
            "slot": 1u64,
            "meta": {
                "err": null,
                "fee": 5000u64,
                "computeUnitsConsumed": 4000u64,
                "preBalances": [10_000_000_000u64, 0u64],
                "postBalances": [8_500_000_000u64, 1_500_000_000u64],
                "innerInstructions": [],
                "logMessages": [
                    "Program 11111111111111111111111111111111 invoke [1]",
                    "Program 11111111111111111111111111111111 success",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [1]",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA consumed 4000 of 200000 compute units",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA success"
                ]
            },
            "transaction": {"message": {
                "header": {"numRequiredSignatures": 1, "numReadonlySignedAccounts": 0, "numReadonlyUnsignedAccounts": 1},
                "accountKeys": ["Payer1111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Dest2222bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Mint3333cccccccccccccccccccccccccccccc", "ToknAcct4444dddddddddddddddddddddddddd"],
                "instructions": [
                    {"programIdIndex": 0, "accounts": [0, 1], "data": system_data},
                    {"programIdIndex": 0, "accounts": [3, 2, 1], "data": token_data}
                ]
            }}
        }})
    }

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
        app.cluster = "devnet".into();
        app.goto(Screen::Execution);
        app.focus_nav = false;
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
        assert_eq!(app.selected_node(), Some(1), "selection starts on the failure");

        on_key(&mut app, KeyEvent::from(KeyCode::Left));
        assert_eq!(app.visible_nodes().len(), 1, "left from a leaf collapses its parent");
        assert_eq!(app.selected_node(), Some(0));
        assert!(render(&mut app, 120, 34).contains("(+1)"));

        on_key(&mut app, KeyEvent::from(KeyCode::Right));
        assert_eq!(app.visible_nodes().len(), 2);
    }

    #[test]
    fn navigation_switches_workspaces_and_modes() {
        let mut app = loaded();
        app.goto(Screen::StateDiff);
        assert!(render(&mut app, 120, 34).contains("STATE DIFF"));
        app.goto(Screen::Logs);
        assert!(render(&mut app, 120, 34).contains("PROGRAM LOGS"));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('o')));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('7')));
        assert!(render(&mut app, 120, 34).contains("signature › 7"));
        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
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
    fn instruction_is_a_drill_in_screen_not_a_section() {
        let mut app = loaded();
        app.goto(Screen::Overview);
        let screen = render(&mut app, 120, 34);
        assert!(
            !app.sections().iter().any(|(screen, _, _)| *screen == Screen::Invocation),
            "the drill-in view is not a listed section"
        );
        assert!(screen.contains("┌ Execution"));

        app.goto(Screen::Execution);
        app.inspect_invocation();
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("INSTRUCTION"));

        on_key(&mut app, KeyEvent::from(KeyCode::Char('b')));
        assert!(matches!(app.screen, Screen::Execution));
        on_key(&mut app, KeyEvent::from(KeyCode::Char('b')));
        assert!(matches!(app.screen, Screen::Overview));
    }

    #[test]
    fn value_flow_derives_transfers_and_links_to_instructions() {
        let mut app = App::new("devnet".into());
        app.tx = Some(solana::parse("4vK9mP", "devnet", &transfer_fixture()));
        app.kind = solana::Kind::Tx;
        app.goto(Screen::Flow);

        let transfers = solana::transfers(app.tx().unwrap());
        assert_eq!(transfers.len(), 2);
        assert_eq!(transfers[0].kind, "System::Transfer");
        assert!((transfers[0].amount - 1.5).abs() < 1e-9);
        assert_eq!(transfers[1].kind, "Token::TransferChecked");
        assert!((transfers[1].amount - 2.5).abs() < 1e-9);

        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("VALUE FLOW"));
        assert!(screen.contains("▶"));
        assert!(screen.contains("-1.500000 SOL"));
        assert!(screen.contains("System::Transfer"));
        assert!(screen.contains("+2.500000"));

        on_key(&mut app, KeyEvent::from(KeyCode::Down));
        on_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.screen, Screen::Invocation));
        assert_eq!(app.node_index, Some(1));
    }

    #[test]
    fn networks_switch_from_the_keyboard() {
        let mut app = loaded();
        app.tx = None;
        on_key(&mut app, KeyEvent::from(KeyCode::Char(']')));
        assert_eq!(app.cluster, "testnet");
        on_key(&mut app, KeyEvent::from(KeyCode::Char(']')));
        assert_eq!(app.cluster, "localnet");
        on_key(&mut app, KeyEvent::from(KeyCode::Char('[')));
        assert_eq!(app.cluster, "testnet");
        assert!(!app.cluster_menu, "direct switching never opens the dropdown");
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
    fn clicking_a_section_opens_it() {
        let mut app = loaded();
        app.goto(Screen::Overview);
        render(&mut app, 120, 34);
        let logs_row = app
            .sections()
            .iter()
            .position(|(screen, _, _)| *screen == Screen::Logs)
            .expect("logs is a section");
        let hit = app
            .hits
            .iter()
            .filter(|hit| hit.click == Click::Row(logs_row))
            .map(|hit| (hit.area.x, hit.area.y))
            .next_back()
            .expect("section row is clickable");
        app.click(hit.0, hit.1);
        assert!(matches!(app.screen, Screen::Logs));
        assert!(render(&mut app, 120, 34).contains("PROGRAM LOGS"));
    }

    #[test]
    fn overview_is_the_hub_for_every_section() {
        let mut app = loaded();
        app.goto(Screen::Overview);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("┌ Execution"), "sections render as boxes");
        assert!(screen.contains("failed at 1.1"));
        assert!(screen.contains("┌ Value Flow"));
        assert!(screen.contains("lines"));

        app.list.select(Some(0));
        on_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.screen, Screen::Execution));
        assert!(!app.focus_nav, "entering a section keeps focus in the panel");
        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(matches!(app.screen, Screen::Overview), "esc returns to the hub");

        app.account = Some(solana::parse_account("8tK4iuyx", &account_fixture()));
        app.kind = solana::Kind::Account;
        app.goto(Screen::Info);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("┌ Data"));
        assert!(screen.contains("82 B"));
        assert!(!screen.contains("Execution"));

        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(app.focus_nav, "focus returns to the history sidebar");
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("HISTORY"));
    }

    #[test]
    fn failed_transactions_open_on_the_failing_instruction() {
        let mut app = App::new("devnet".into());
        app.tx = Some(solana::parse("4vK9mP", "devnet", &fixture()));
        app.kind = solana::Kind::Tx;
        app.goto(Screen::Execution);

        assert_eq!(app.selected_node(), app.tx().unwrap().failed_node);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("custom program error: 0x1"));
        assert!(screen.contains("8,420 self"));
    }

    #[test]
    fn the_failure_path_is_marked_from_root_to_error() {
        let mut app = loaded();
        let tx = app.tx().unwrap();
        assert_eq!(tx.failure_path(), vec![0, 1]);

        app.goto(Screen::Execution);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("↯"), "ancestors carry the propagation marker");
        assert!(screen.contains("propagated"));
        assert!(screen.contains("✕"));

        app.collapsed.insert(0);
        app.focus_failure();
        assert!(!app.collapsed.contains(&0), "ancestors expand to reveal the failure");
    }

    #[test]
    fn topbar_shows_only_the_loaded_entity() {
        let mut app = loaded();
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("Slot 312,441,221"));
        assert!(screen.contains("CU 12,620"));

        app.account = Some(solana::parse_account("8tK4iuyx", &account_fixture()));
        app.kind = solana::Kind::Account;
        app.goto(Screen::Info);
        let screen = render(&mut app, 120, 34);
        assert!(!screen.contains("Slot 312,441,221"));
        assert!(!screen.contains("CU 12,620"));
        assert!(screen.contains("Balance 0.002039280 SOL"));
        assert!(screen.contains("Size 82 B"));
    }

    #[test]
    fn escape_pages_back_through_visited_entities() {
        let mut app = loaded();
        app.goto(Screen::Accounts);
        app.account = Some(solana::parse_account("8tK4iuyx", &account_fixture()));
        app.kind = solana::Kind::Account;
        app.stack.push(app::Location {
            kind: solana::Kind::Tx,
            key: "4vK9mP".into(),
            screen: Screen::Accounts,
        });
        app.goto(Screen::Info);
        app.focus_nav = true;

        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.kind, solana::Kind::Tx);
        assert!(matches!(app.screen, Screen::Accounts));

        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(app.focus_nav);
    }

    #[test]
    fn opening_an_entity_directly_clears_the_back_stack() {
        let mut app = loaded();
        app.stack.push(app::Location {
            kind: solana::Kind::Tx,
            key: "4vK9mP".into(),
            screen: Screen::Accounts,
        });

        app.open_account("8tK4iuyx");
        assert!(app.stack.is_empty(), "history/prompt navigation starts a fresh stack");

        app.account = Some(solana::parse_account("8tK4iuyx", &account_fixture()));
        app.kind = solana::Kind::Account;
        app.focus_nav = true;
        on_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(app.focus_nav);
    }

    #[test]
    fn enter_on_an_account_row_fetches_that_account() {
        let mut app = loaded();
        app.goto(Screen::Accounts);
        app.list.select(Some(1));
        let key = app.selected_account().expect("account row resolves to a pubkey");
        assert_eq!(key, "Vau1t111");

        app.goto(Screen::Execution);
        app.list.select(Some(1));
        assert_eq!(app.selected_account(), None, "the tree never opens accounts");

        on_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.screen, Screen::Invocation));
        assert_eq!(app.node_index, Some(1));
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("INSTRUCTION 1.1"));
        assert!(screen.contains("✕ failed"));
        assert!(screen.contains("Call path"));
        assert!(screen.contains("Vau1…1111  →  Token Program"));
        assert!(screen.contains("custom program error: 0x1"));
        assert!(screen.contains("LOGS"));

        app.list.select(Some(0));
        assert_eq!(app.selected_account().as_deref(), Some("Auth1111"));

        on_key(&mut app, KeyEvent::from(KeyCode::Char('b')));
        assert!(matches!(app.screen, Screen::Execution), "b steps back to the tree");
    }

    #[test]
    fn overview_previews_the_failure_path_and_state_changes() {
        let mut app = loaded();
        app.goto(Screen::Overview);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("BALANCE CHANGES"));
        assert!(screen.contains("PRE"));
        assert!(screen.contains("POST"));
        assert!(screen.contains("CHANGE"));
        assert!(screen.contains("FAILURE PATH"));
        assert!(screen.contains("↯ 1 Vau1…1111"));
        assert!(screen.contains("✕ 1.1 Token Program"));
        assert!(screen.contains("Transaction Failed"));
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
        app.tx = None;
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
