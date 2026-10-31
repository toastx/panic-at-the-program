use crossterm::event::{KeyCode, KeyEvent};
use ratatui::backend::TestBackend;
use ratatui::prelude::*;

use crate::app::screens;
use crate::app::state::{self as app, App, Click, Mode, Screen};
use crate::chain::{diagnose, idl, solana, validate};
use crate::on_key;
use crate::workspace;
use serde_json::{Value, json};
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

fn constraint_fixture() -> serde_json::Value {
    json!({"result": {
        "slot": 1u64,
        "meta": {
            "err": {"InstructionError": [0, {"Custom": 2006}]},
            "fee": 5000u64,
            "computeUnitsConsumed": 4000u64,
            "preBalances": [10u64],
            "postBalances": [10u64],
            "innerInstructions": [],
            "logMessages": [
                "Program Vau1t11111111111111111111111111111111111 invoke [1]",
                "Program log: Instruction: Withdraw",
                "Program log: AnchorError thrown in programs/vault/src/lib.rs:88. Error Code: ConstraintSeeds. Error Number: 2006. Error Message: A seeds constraint was violated.",
                "Program log: AnchorError caused by account: vault. Error Code: ConstraintSeeds. Error Number: 2006. Error Message: A seeds constraint was violated.",
                "Program log: Left:",
                "Program log: 3sPq1aBxGv2kkR7ZLczdKJ2w7xkGk4trQJcM3pQWvNnT",
                "Program log: Right:",
                "Program log: 7xKX9mPaRvQw1bNcTzWd4rYfG6hJ8uL2sE5vA3dC9kFq",
                "Program Vau1t11111111111111111111111111111111111 consumed 4000 of 200000 compute units",
                "Program Vau1t11111111111111111111111111111111111 failed: custom program error: 0x7d6"
            ]
        },
        "transaction": {"message": {
            "header": {"numRequiredSignatures": 1, "numReadonlySignedAccounts": 0, "numReadonlyUnsignedAccounts": 0},
            "accountKeys": ["Auth1111", "Vau1t111"],
            "instructions": [{"programIdIndex": 0, "accounts": [0, 1], "data": ""}]
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
                "Program log: Instruction: Withdraw",
                "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [2]",
                "Program log: Instruction: TransferChecked",
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
        .find(|hit| hit.click == Click::Cluster("testnet".to_string()))
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
        !app.sections()
            .iter()
            .any(|(screen, _, _)| *screen == Screen::Invocation),
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
fn program_addresses_derive_off_curve() {
    let token = solana::bs58_decode("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
    let mint = solana::bs58_decode("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    let owner = solana::bs58_decode("BdHB1RVLPXjs1yvH1Nt1rMW4nNzBjSuRtsvBh5zm8BBA");
    let ata = solana::bs58_decode("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

    let (derived, bump) = idl::find_program_address(&[&owner, &token, &mint], &ata).unwrap();
    assert!(bump > 240, "canonical bumps are near 255, got {bump}");
    assert_eq!(derived.len(), 32);

    let (again, same_bump) = idl::find_program_address(&[&owner, &token, &mint], &ata).unwrap();
    assert_eq!((derived, bump), (again, same_bump), "derivation is deterministic");
}

#[test]
fn idl_decodes_errors_instructions_and_accounts() {
    let idl = json!({
        "address": "Vau1t1111111111111111111111111111111111111",
        "instructions": [{
            "name": "withdraw",
            "discriminator": [1, 2, 3, 4, 5, 6, 7, 8],
            "accounts": [{"name": "vault"}, {"name": "authority"}],
            "args": [{"name": "amount", "type": "u64"}, {"name": "memo", "type": "string"}]
        }],
        "accounts": [{"name": "Vault", "discriminator": [9, 9, 9, 9, 9, 9, 9, 9]}],
        "types": [{"name": "Vault", "type": {"fields": [
            {"name": "owner", "type": "pubkey"},
            {"name": "balance", "type": "u64"},
            {"name": "frozen", "type": "bool"}
        ]}}],
        "errors": [{"code": 6001, "name": "SlippageExceeded", "msg": "price moved beyond tolerance"}]
    });

    let program = idl::parse(&idl);
    assert_eq!(program.flavor, idl::Flavor::Anchor);
    let (name, message) = idl::error(&program, 6001).unwrap();
    assert_eq!(name, "SlippageExceeded");
    assert_eq!(message, "price moved beyond tolerance");
    assert!(idl::error(&program, 6002).is_none());

    let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
    data.extend_from_slice(&7_500_000u64.to_le_bytes());
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(b"hi");
    let decoded = idl::instruction(&program, &data).unwrap();
    assert_eq!(decoded.name, "withdraw");
    assert_eq!(decoded.args[0], ("amount".into(), "7500000".into()));
    assert_eq!(decoded.args[1], ("memo".into(), "hi".into()));
    assert_eq!(
        decoded.specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["vault", "authority"]
    );

    let mut account_data = vec![9u8; 8];
    account_data.extend_from_slice(&solana::bs58_decode("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"));
    account_data.extend_from_slice(&42u64.to_le_bytes());
    account_data.push(1);
    let (layout, fields) = idl::account(&program, &account_data).unwrap();
    assert_eq!(layout, "Vault");
    assert_eq!(fields[0].1, "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    assert_eq!(fields[1], ("balance".into(), "42".into()));
    assert_eq!(fields[2], ("frozen".into(), "true".into()));
}

#[test]
fn cached_idls_stay_out_of_the_history_list() {
    let mut app = loaded();
    app.entries = solana::entries("devnet");
    assert!(
        !app.entries.iter().any(|entry| entry.key.starts_with("idl-")),
        "IDL cache files are not browsable entities"
    );
}

/// The note bodies of a diagnosis, in rendered order.
fn note_texts(failure: &diagnose::Failure) -> Vec<String> {
    failure.notes.iter().map(|note| note.text.clone()).collect()
}

#[test]
fn unknown_codes_are_inferred_from_the_idl_flavor() {
    use idl::Flavor;

    let (headline, meaning, hint) = diagnose::infer_custom(6001, Some(Flavor::Anchor));
    assert!(headline.contains("Anchor user error #1"));
    assert!(meaning.contains("does not list"));
    assert!(hint.contains("older than the deployed program"));

    let (headline, _, hint) = diagnose::infer_custom(2006, Some(Flavor::Anchor));
    assert_eq!(headline, "ConstraintSeeds");
    assert!(hint.contains("framework"));

    let (headline, meaning, hint) = diagnose::infer_custom(3, Some(Flavor::Shank));
    assert!(headline.contains("program error #3"), "{headline}");
    assert!(meaning.contains("number their errors from 0"));
    assert!(hint.contains("4th entry"), "{hint}");

    let (headline, _, hint) = diagnose::infer_custom(7, Some(Flavor::Codama));
    assert!(headline.contains("program error #7"));
    assert!(hint.contains("8th entry"));

    let (headline, _, hint) = diagnose::infer_custom(3, None);
    assert!(headline.contains("custom program error 3"));
    assert!(hint.contains("number errors from 0"));
}

#[test]
fn custom_error_names_come_from_the_workspace_table() {
    let parsed = workspace::parse(
        r#"
        [errors.Vau1t11111111111111111111111111111111111]
        3 = "InsufficientLiquidity: the pool cannot cover this swap"
        9 = "PoolPaused"
        12 = ""
        "#,
    );
    let table = &parsed["errors.Vau1t11111111111111111111111111111111111"];
    assert_eq!(table["3"], "InsufficientLiquidity: the pool cannot cover this swap");
    assert_eq!(table["9"], "PoolPaused");
    assert_eq!(table["12"], "", "an empty stub is a placeholder, not a name");
}

#[test]
fn shank_idls_are_detected_and_decoded() {
    let shank = json!({
        "version": "1.0.0",
        "name": "mpl_token_metadata",
        "instructions": [{
            "name": "CreateMetadataAccount",
            "accounts": [
                {"name": "metadata", "isMut": true, "isSigner": false},
                {"name": "payer", "isMut": true, "isSigner": true}
            ],
            "args": [{"name": "lamports", "type": "u64"}],
            "discriminant": {"type": "u8", "value": 33}
        }],
        "accounts": [{"name": "Metadata", "type": {"kind": "struct", "fields": [
            {"name": "updateAuthority", "type": "publicKey"},
            {"name": "primarySaleHappened", "type": "bool"}
        ]}}],
        "errors": [{"code": 0, "name": "InstructionUnpackError", "msg": "Instruction unpack error"}],
        "metadata": {"origin": "shank", "address": "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s"}
    });

    let program = idl::parse(&shank);
    assert_eq!(program.flavor, idl::Flavor::Shank);
    assert_eq!(
        program.address.as_deref(),
        Some("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s")
    );
    assert_eq!(
        program.instructions[0].discriminator,
        vec![33],
        "shank uses a single-byte discriminant"
    );
    assert!(program.instructions[0].accounts[1].signer);
    assert!(program.instructions[0].accounts[0].writable);

    let mut data = vec![33u8];
    data.extend_from_slice(&4_200_000u64.to_le_bytes());
    let decoded = idl::instruction(&program, &data).unwrap();
    assert_eq!(decoded.name, "CreateMetadataAccount");
    assert_eq!(decoded.args[0], ("lamports".into(), "4200000".into()));

    let (name, message) = idl::error(&program, 0).unwrap();
    assert_eq!(name, "InstructionUnpackError");
    assert_eq!(message, "Instruction unpack error");
}

#[test]
fn codama_idls_are_detected_and_decoded() {
    let codama = json!({
        "kind": "rootNode",
        "standard": "codama",
        "program": {
            "kind": "programNode",
            "name": "vault",
            "publicKey": "Vau1t11111111111111111111111111111111111",
            "accounts": [{
                "kind": "accountNode",
                "name": "vault",
                "data": {"kind": "structTypeNode", "fields": [
                    {"kind": "structFieldTypeNode", "name": "discriminator", "type": {"kind": "numberTypeNode", "format": "u8"},
                     "defaultValue": {"kind": "numberValueNode", "number": 7}, "defaultValueStrategy": "omitted"},
                    {"kind": "structFieldTypeNode", "name": "owner", "type": {"kind": "publicKeyTypeNode"}},
                    {"kind": "structFieldTypeNode", "name": "balance", "type": {"kind": "numberTypeNode", "format": "u64"}}
                ]},
                "discriminators": [{"kind": "fieldDiscriminatorNode", "name": "discriminator", "offset": 0}]
            }],
            "instructions": [{
                "kind": "instructionNode",
                "name": "withdraw",
                "accounts": [
                    {"kind": "instructionAccountNode", "name": "vault", "isWritable": true, "isSigner": false},
                    {"kind": "instructionAccountNode", "name": "authority", "isWritable": false, "isSigner": true}
                ],
                "arguments": [
                    {"kind": "instructionArgumentNode", "name": "discriminator", "type": {"kind": "numberTypeNode", "format": "u8"},
                     "defaultValue": {"kind": "numberValueNode", "number": 3}, "defaultValueStrategy": "omitted"},
                    {"kind": "instructionArgumentNode", "name": "amount", "type": {"kind": "numberTypeNode", "format": "u64"}},
                    {"kind": "instructionArgumentNode", "name": "memo", "type": {"kind": "sizePrefixTypeNode", "type": {"kind": "stringTypeNode"}}}
                ],
                "discriminators": [{"kind": "fieldDiscriminatorNode", "name": "discriminator", "offset": 0}]
            }],
            "errors": [{"kind": "errorNode", "name": "vaultLocked", "code": 6001, "message": "the vault is locked"}]
        }
    });

    let program = idl::parse(&codama);
    assert_eq!(program.flavor, idl::Flavor::Codama);
    assert_eq!(
        program.address.as_deref(),
        Some("Vau1t11111111111111111111111111111111111")
    );
    assert_eq!(program.instructions[0].discriminator, vec![3]);
    assert!(program.instructions[0].accounts[1].signer);
    assert!(!program.instructions[0].accounts[1].writable);

    let mut data = vec![3u8];
    data.extend_from_slice(&999u64.to_le_bytes());
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(b"ok");
    let decoded = idl::instruction(&program, &data).unwrap();
    assert_eq!(decoded.name, "withdraw");
    assert_eq!(decoded.args[0], ("amount".into(), "999".into()));
    assert_eq!(decoded.args[1], ("memo".into(), "ok".into()));
    assert_eq!(
        decoded.specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["vault", "authority"]
    );

    let mut account_data = vec![7u8];
    account_data.extend_from_slice(&solana::bs58_decode("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"));
    account_data.extend_from_slice(&55u64.to_le_bytes());
    let (layout, fields) = idl::account(&program, &account_data).unwrap();
    assert_eq!(layout, "vault");
    assert_eq!(fields[1], ("balance".into(), "55".into()));

    let (name, message) = idl::error(&program, 6001).unwrap();
    assert_eq!(name, "vaultLocked");
    assert_eq!(message, "the vault is locked");
}

#[test]
fn idl_errors_replace_the_generic_anchor_headline() {
    let mut tx = solana::parse("s", "devnet", &fixture());
    let idl = json!({"errors": [{"code": 6001, "name": "VaultLocked", "msg": "the vault is locked"}]});
    let failure = tx.failure.as_mut().unwrap();
    failure.code = diagnose::Code::Custom(6001);
    failure.headline = "Anchor user error #1".into();

    let (name, message) = idl::error(&idl::parse(&idl), 6001).unwrap();
    assert_eq!(name, "VaultLocked");
    assert_eq!(message, "the vault is locked");
}

#[test]
fn instruction_errors_decode_into_a_diagnosis() {
    use diagnose::Code;

    let anchor = json!({"InstructionError": [5, {"Custom": 6001}]});
    let nodes = solana::parse("s", "devnet", &fixture()).nodes;
    let failure = diagnose::failure(&anchor, &nodes, &[], 1000, 200_000).unwrap();
    assert_eq!(failure.code, Code::Custom(6001));
    assert_eq!(failure.instruction, Some(5));
    assert_eq!(failure.headline, "Anchor user error #1");
    assert!(
        failure.label().ends_with(": Anchor user error #1"),
        "{}",
        failure.label()
    );
    assert!(note_texts(&failure).iter().any(|t| t.contains("error enum")));

    let seeds = json!({"InstructionError": [0, {"Custom": 2006}]});
    let failure = diagnose::failure(&seeds, &nodes, &[], 1000, 200_000).unwrap();
    assert!(failure.headline.contains("ConstraintSeeds"));
    assert_eq!(failure.label(), "error[anchor.2006]: ConstraintSeeds");
    assert!(failure.meaning.contains("seeds constraint"));
    assert!(note_texts(&failure).iter().any(|t| t.contains("re-derive the PDA")));

    let signature = json!({"InstructionError": [0, "MissingRequiredSignature"]});
    let failure = diagnose::failure(&signature, &nodes, &[], 1000, 200_000).unwrap();
    assert_eq!(failure.code, Code::Named("MissingRequiredSignature".into()));
    assert!(failure.meaning.contains("must sign"));
    assert!(note_texts(&failure).iter().any(|t| t.contains("no signature from")));

    assert!(diagnose::failure(&Value::Null, &nodes, &[], 0, 0).is_none());
}

#[test]
fn validation_flags_signer_writable_address_and_pda_mismatches() {
    use validate::Level;

    let tx = solana::parse("4vK9mP", "devnet", &constraint_fixture());
    let node = &tx.nodes[0];
    let specs = json!([
        {"name": "authority", "signer": true, "writable": true},
        {"name": "vault", "writable": true, "pda": {"seeds": [
            {"kind": "const", "value": [118, 97, 117, 108, 116]},
            {"kind": "account", "path": "authority"}
        ]}}
    ]);
    let specs = idl::account_specs(&specs);

    let findings = validate::declared_findings(&specs, node);
    let pda = findings
        .iter()
        .find(|f| f.message.contains("PDA"))
        .expect("the vault PDA mismatch is caught");
    assert_eq!(pda.index, 1);
    assert_eq!(pda.role, "vault");
    assert_eq!(pda.level, Level::Error);
    assert_eq!(pda.actual.as_deref(), Some("Vau1t111"));
    assert!(pda.expected.as_deref().is_some_and(|key| key.len() > 32));

    let partial = json!([{"name": "authority", "signer": true, "writable": true}]);
    assert!(
        validate::declared_findings(&idl::account_specs(&partial), node).is_empty(),
        "optional accounts shift indices, so mismatched counts are not checked positionally"
    );

    let fixed = json!([{"name": "authority"}, {"name": "token_program", "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"}]);
    let findings = validate::declared_findings(&idl::account_specs(&fixed), node);
    let address = findings.iter().find(|f| f.message.contains("address")).unwrap();
    assert_eq!(
        address.expected.as_deref(),
        Some("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
    );
    assert_eq!(address.actual.as_deref(), Some("Vau1t111"));
}

#[test]
fn validation_flags_token_and_rent_problems_from_account_state() {
    use std::collections::HashMap;

    let tx = solana::parse("4vK9mP", "devnet", &constraint_fixture());
    let node = &tx.nodes[0];
    let mut data = vec![0u8; 165];
    data[0..32].copy_from_slice(&solana::bs58_decode("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"));
    data[32..64].copy_from_slice(&solana::bs58_decode("BdHB1RVLPXjs1yvH1Nt1rMW4nNzBjSuRtsvBh5zm8BBA"));

    let mut state = HashMap::new();
    state.insert(
        "Vau1t111".to_string(),
        validate::State {
            owner: validate::TOKEN.to_string(),
            lamports: 100,
            data,
        },
    );

    let roles = vec!["mint".to_string(), "associated_token".to_string()];
    let findings = validate::state_findings(node, &roles, &state, |_| Some(2_039_280));
    let ata = findings
        .iter()
        .find(|f| f.message.contains("associated token account"))
        .expect("a role named associated_* must be the canonical ATA");
    assert_eq!(ata.level, validate::Level::Error);
    assert!(ata.expected.as_deref().is_some_and(|key| key.len() > 32));

    let quiet = validate::state_findings(node, &[], &state, |_| Some(2_039_280));
    assert!(
        quiet.iter().all(|f| f.message.contains("rent-exempt")),
        "an undeclared token account is not second-guessed"
    );
    let rent = findings.iter().find(|f| f.message.contains("rent-exempt")).unwrap();
    assert_eq!(rent.level, validate::Level::Warn);
    assert_eq!(rent.expected.as_deref(), Some("0.002039280 SOL"));
    assert_eq!(rent.actual.as_deref(), Some("0.000000100 SOL"));

    let empty: HashMap<String, validate::State> = HashMap::new();
    assert!(validate::state_findings(node, &[], &empty, |_| None).is_empty());
}

#[test]
fn anchor_logs_yield_account_source_and_expected_values() {
    let logs: Vec<String> = [
        "Program Vau1t11111111111111111111111111111111111 invoke [1]",
        "Program log: Instruction: Withdraw",
        "Program log: AnchorError caused by account: vault. Error Code: ConstraintSeeds. Error Number: 2006. Error Message: A seeds constraint was violated.",
        "Program log: Left:",
        "Program log: 3sPq1aBxGv2kkR7ZLczdKJ2w7xkGk4trQJcM3pQWvNnT",
        "Program log: Right:",
        "Program log: 7xKX9mPaRvQw1bNcTzWd4rYfG6hJ8uL2sE5vA3dC9kFq",
        "Program Vau1t11111111111111111111111111111111111 failed: custom program error: 0x7d6",
    ]
    .map(String::from)
    .to_vec();

    let parsed = diagnose::anchor_log(&logs);
    assert_eq!(parsed.account.as_deref(), Some("vault"));
    assert_eq!(parsed.name.as_deref(), Some("ConstraintSeeds"));
    assert_eq!(parsed.number, Some(2006));
    assert_eq!(parsed.message.as_deref(), Some("A seeds constraint was violated"));
    assert_eq!(
        parsed.actual.as_deref(),
        Some("3sPq1aBxGv2kkR7ZLczdKJ2w7xkGk4trQJcM3pQWvNnT")
    );
    assert_eq!(
        parsed.expected.as_deref(),
        Some("7xKX9mPaRvQw1bNcTzWd4rYfG6hJ8uL2sE5vA3dC9kFq")
    );

    let thrown = ["Program log: AnchorError thrown in programs/vault/src/lib.rs:88. Error Code: Unauthorized. Error Number: 6000. Error Message: not the owner."]
        .map(String::from)
        .to_vec();
    let parsed = diagnose::anchor_log(&thrown);
    assert_eq!(parsed.source.as_deref(), Some("programs/vault/src/lib.rs:88"));
    assert_eq!(parsed.name.as_deref(), Some("Unauthorized"));
    assert_eq!(parsed.number, Some(6000));
}

#[test]
fn a_constraint_failure_renders_expected_against_actual() {
    let mut app = App::new("devnet".into());
    app.tx = Some(solana::parse("4vK9mP", "devnet", &constraint_fixture()));
    app.kind = solana::Kind::Tx;
    app.goto(Screen::Overview);

    let failure = app.tx().unwrap().failure.as_ref().unwrap();
    assert!(failure.headline.contains("ConstraintSeeds"));
    assert_eq!(failure.account.as_deref(), Some("vault"));
    assert_eq!(failure.source.as_deref(), Some("programs/vault/src/lib.rs:88"));

    let screen = render(&mut app, 120, 34);
    assert!(screen.contains("account"));
    assert!(screen.contains("vault"));
    assert!(screen.contains("expected"));
    assert!(screen.contains("7xKX9mPaRvQw1bNcTzWd4rYfG6hJ8uL2sE5vA3dC9kFq"));
    assert!(screen.contains("actual"));
    assert!(screen.contains("3sPq1aBxGv2kkR7ZLczdKJ2w7xkGk4trQJcM3pQWvNnT"));
    assert!(screen.contains("programs/vault/src/lib.rs:88"));
    assert!(screen.contains("failed the constraint"));
}

#[test]
fn logs_nest_under_the_invocation_that_emitted_them() {
    let tx = solana::parse("4vK9mP", "devnet", &fixture());
    let frames = tx.log_frames();
    let depth_of = |needle: &str| {
        let index = tx.logs.iter().position(|line| line.contains(needle)).unwrap();
        frames[index].map(|node| tx.nodes[node].depth)
    };

    assert_eq!(depth_of("Instruction: Withdraw"), Some(0));
    assert_eq!(depth_of("Instruction: TransferChecked"), Some(1));
    assert_eq!(
        depth_of("consumed 8420"),
        Some(1),
        "inner frame owns its own meter line"
    );

    let mut app = loaded();
    app.goto(Screen::Logs);
    let screen = render(&mut app, 120, 34);
    assert!(screen.contains("1.1"), "log rows carry the frame path");
}

#[test]
fn workspace_config_parses_sections_and_labels() {
    let parsed = workspace::parse(
        r#"
        # a comment
        idl_dir = "idls"

        [idl]
        Vau1t11111111111111111111111111111111111 = "idls/vault.json"

        [programs]
        Vau1t11111111111111111111111111111111111 = "Vault"   # trailing comment

        [accounts]
        Auth1111 = "treasury"
        "#,
    );

    assert_eq!(parsed[""]["idl_dir"], "idls");
    assert_eq!(
        parsed["idl"]["Vau1t11111111111111111111111111111111111"],
        "idls/vault.json"
    );
    assert_eq!(parsed["programs"]["Vau1t11111111111111111111111111111111111"], "Vault");
    assert_eq!(parsed["accounts"]["Auth1111"], "treasury");
    assert!(workspace::parse("").is_empty());
}

#[test]
fn heuristics_suggest_a_fix_per_failure_class() {
    let nodes = solana::parse("s", "devnet", &fixture()).nodes;
    let fix = |err: Value, logs: Vec<String>, cu, limit| {
        diagnose::failure(&err, &nodes, &logs, cu, limit)
            .map(|f| note_texts(&f).join(" | "))
            .unwrap_or_default()
    };

    let blockhash = fix(json!("BlockhashNotFound"), vec![], 0, 0);
    assert!(blockhash.contains("fetch a fresh one and re-sign"));

    let compute = fix(
        json!({"InstructionError": [0, "ProgramFailedToComplete"]}),
        vec![
            "Program Vau1t11111111111111111111111111111111111 consumed 200000 of 200000 compute units".into(),
            "Program failed to complete: exceeded CUs meter at BPF instruction".into(),
        ],
        200_000,
        200_000,
    );
    assert!(compute.contains("ran out of compute: 200000 of 200000"));
    assert!(compute.contains("SetComputeUnitLimit"));

    let rent = fix(
        json!({"InstructionError": [0, {"Custom": 0}]}),
        vec!["Program log: Transfer: insufficient lamports 890880, need 2039280".into()],
        0,
        0,
    );
    assert!(
        rent.contains("0.001148400 more SOL"),
        "shows the exact shortfall: {rent}"
    );

    let in_use = fix(
        json!({"InstructionError": [0, {"Custom": 0}]}),
        vec!["Program log: Allocate: account Vau1t111 already in use".into()],
        0,
        0,
    );
    assert!(in_use.contains("init_if_needed"));

    let discriminator = fix(json!({"InstructionError": [0, {"Custom": 101}]}), vec![], 0, 0);
    assert!(discriminator.contains("does not recognise this instruction"));

    let panic = fix(
        json!({"InstructionError": [0, "ProgramFailedToComplete"]}),
        vec!["Program log: panicked at 'attempt to subtract with overflow'".into()],
        0,
        0,
    );
    assert!(panic.contains("check unwraps and arithmetic"));
}

#[test]
fn token_program_errors_use_the_token_table() {
    let nodes = solana::parse("s", "devnet", &fixture()).nodes;
    let failure = diagnose::failure(&json!({"InstructionError": [0, {"Custom": 1}]}), &nodes, &[], 0, 0).unwrap();
    assert_eq!(failure.headline, "InsufficientFunds");
    assert_eq!(failure.label(), "error[token.1]: InsufficientFunds");
    assert!(failure.meaning.contains("not hold enough tokens"));
}

#[test]
fn instruction_names_come_from_program_logs() {
    let tx = solana::parse("s", "devnet", &fixture());
    assert_eq!(tx.nodes[0].name.as_deref(), Some("Withdraw"));
    assert_eq!(tx.nodes[1].name.as_deref(), Some("TransferChecked"));
    let screen = render(&mut loaded(), 120, 34);
    assert!(screen.contains("Vau1…1111 · Withdraw"));
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
    assert!(screen.contains("Vau1…1111 · Withdraw  →  Token Program · TransferChecked"));
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
    assert!(screen.contains("DIAGNOSIS"));
    assert!(screen.contains("instruction 1.1"));
    assert!(screen.contains("Vau1…1111 · Withdraw  →  Token Program · TransferChecked"));
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
fn a_serialized_transaction_decodes_into_accounts_and_instructions() {
    // the transaction from a recorded sendTransaction, legacy format
    let request: Value = serde_json::from_str(crate::suite::tap::SEND_REQUEST).expect("json");
    let bytes = solana::base64_decode(request["params"][0].as_str().expect("base64"));
    let message = solana::decode_message(&bytes).expect("decoded");
    assert!(message.accounts_complete);
    assert_eq!(
        message.json["accountKeys"][0],
        "4AvpokDPwAV5MBwY4VahxddcDF9AHJHCDbpbUdfUpjWa"
    );
    assert_eq!(
        message.json["accountKeys"][2],
        "2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5"
    );
    assert_eq!(message.json["header"]["numRequiredSignatures"], 1);
    assert_eq!(message.json["instructions"][0]["accounts"], json!([1, 0]));

    // and it feeds the normal parse, so the payer reads as the payer
    let tx = solana::parse(
        "t",
        "tests",
        &json!({"result": {"transaction": {"message": message.json}, "meta": {"err": Value::Null}}}),
    );
    assert_eq!(tx.accounts.len(), 3);
    assert_eq!(tx.accounts[0].role(), "signer, writable");

    // A v0 message naming a lookup table it does not carry: the static keys
    // are real, but an instruction index may point past them, so the account
    // mapping is marked short rather than quietly presented as whole.
    let mut v0: Vec<u8> = vec![1];
    v0.extend(std::iter::repeat_n(0u8, 64));
    v0.push(0x80);
    v0.extend([1, 0, 1]);
    v0.push(2);
    v0.extend(std::iter::repeat_n(7u8, 32));
    v0.extend(std::iter::repeat_n(8u8, 32));
    v0.extend(std::iter::repeat_n(9u8, 32));
    v0.extend([1, 1, 1, 0, 0]); // one instruction: program 1, one account, no data
    v0.push(1); // one lookup table
    v0.extend(std::iter::repeat_n(5u8, 32));
    v0.extend([1, 0]); // one writable index
    v0.push(0); // no readonly indexes
    let message = solana::decode_message(&v0).expect("decoded");
    assert!(!message.accounts_complete, "a loaded address we cannot see");
    assert_eq!(message.json["accountKeys"].as_array().map(Vec::len), Some(2));

    assert!(solana::decode_message(b"nowhere near a transaction").is_none());
}

#[test]
fn the_overview_names_the_payer_and_what_it_held() {
    // a real transaction: preBalances[0] is what the chain recorded
    let mut app = loaded();
    app.goto(Screen::Overview);
    let screen = render(&mut app, 120, 34);
    assert!(screen.contains("payer"), "{screen}");
    assert!(screen.contains("held 1.0000 SOL"), "{screen}");

    // one that never landed: a live snapshot, said to be one
    let mut app = App::new("tests".into());
    app.tx = Some(solana::parse(
        "t",
        "tests",
        &json!({
            "result": {
                "meta": {
                    "err": {"InstructionError": [0, {"Custom": 6000}]},
                    "logMessages": [
                        "Program Vau1t11111111111111111111111111111111111 invoke [1]",
                        "Program log: Instruction: Deposit",
                        "Program Vau1t11111111111111111111111111111111111 failed: custom program error: 0x1770",
                    ],
                },
                "transaction": {"message": {
                    "accountKeys": ["4AvpokDPwAV5MBwY4VahxddcDF9AHJHCDbpbUdfUpjWa"],
                    "instructions": [],
                    "header": {"numRequiredSignatures": 1, "numReadonlySignedAccounts": 0, "numReadonlyUnsignedAccounts": 0},
                }},
            },
            "patp": {"payerBalance": 2_500_000_000u64, "accountsComplete": true},
        }),
    ));
    app.kind = solana::Kind::Tx;
    app.goto(Screen::Overview);
    let screen = render(&mut app, 120, 34);
    assert!(
        screen.contains("4AvpokDPwAV5MBwY4VahxddcDF9AHJHCDbpbUdfUpjWa"),
        "{screen}"
    );
    assert!(screen.contains("holds 2.5000 SOL now (live)"), "{screen}");
}

#[test]
fn the_cache_round_trips_and_evicts() {
    let value = serde_json::json!({"result": {"slot": 7}});
    solana::store("test-cache", "roundtrip", &value).unwrap();
    assert_eq!(solana::load("test-cache", "roundtrip"), Some(value));
    solana::evict("test-cache", "roundtrip");
    assert_eq!(solana::load("test-cache", "roundtrip"), None);
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

/// A finding standing in for one validate.rs would produce.
fn finding(kind: validate::FindingKind, index: usize, role: &str, level: validate::Level) -> validate::Finding {
    validate::Finding {
        index,
        role: role.to_string(),
        key: format!("Key{index}"),
        kind,
        level,
        message: "something is wrong".to_string(),
        expected: Some(format!("want{index}")),
        actual: Some(format!("got{index}")),
    }
}

fn seeds_failure() -> diagnose::Failure {
    let nodes = solana::parse("s", "devnet", &fixture()).nodes;
    diagnose::failure(&json!({"InstructionError": [0, {"Custom": 2006}]}), &nodes, &[], 0, 0).unwrap()
}

#[test]
fn anchor_tsv_is_wellformed() {
    let rows: Vec<diagnose::Known> = diagnose::anchor_rows().collect();
    assert_eq!(rows.len(), 77, "anchor-lang 0.32.2 declares 77 error codes");

    for known in &rows {
        assert!(!known.name.is_empty(), "{} has no name", known.code);
        assert!(!known.message.is_empty(), "{} has no message", known.code);
    }
    let codes: Vec<u32> = rows.iter().map(|known| known.code).collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(codes, sorted, "codes must be ascending with no duplicates");

    // One row per documented range, so a half-finished regeneration is caught.
    for code in [103, 1002, 1500, 2039, 2506, 3017, 4102, 5000] {
        assert!(codes.contains(&code), "missing code {code}");
    }
    assert_eq!(
        diagnose::anchor_rows().find(|known| known.code == 2006).unwrap().class,
        Some(diagnose::Class::Seeds)
    );
}

#[test]
fn every_class_has_tokens_and_help() {
    for &class in diagnose::Class::ALL {
        assert!(!class.tokens().is_empty(), "{class:?} has no vocabulary");
        assert!(!class.help().is_empty(), "{class:?} has no help");
        assert_eq!(diagnose::Class::from_name(&format!("{class:?}")), Some(class));
    }
}

#[test]
fn class_match_is_deterministic_and_conservative() {
    use diagnose::Class::*;

    // Real SPL Token / Token-2022 / DeFi error names. None means the matcher
    // must stay silent: no diagnosis beats a confidently wrong fix.
    let cases: &[(&str, Option<diagnose::Class>)] = &[
        ("OwnerMismatch", Some(WrongOwner)),
        ("InvalidOwner", Some(WrongOwner)),
        ("InvalidMint", Some(Mint)),
        ("MintMismatch", Some(Mint)),
        ("UninitializedState", Some(Uninitialized)),
        ("AlreadyInUse", Some(Already)),
        ("NotRentExempt", Some(Rent)),
        ("AccountFrozen", Some(Paused)),
        ("Overflow", Some(Overflow)),
        ("InvalidInstruction", Some(Instruction)),
        ("InvalidNumberOfProvidedSigners", Some(NotSigner)),
        ("InsufficientFunds", Some(Insufficient)),
        ("SlippageToleranceExceeded", Some(Slippage)),
        ("StaleOracle", Some(Expired)),
        ("Unauthorized", Some(Unauthorized)),
        ("SeedsMismatch", Some(Seeds)),
        ("InvalidPda", Some(Seeds)),
        ("AccountNotWritable", Some(NotWritable)),
        // `already` outscores `init`, so the opposite condition is not suggested.
        ("AlreadyInitialized", Some(Already)),
        // `balance` is not an Insufficient trigger, or this would false-match.
        ("NonNativeHasBalance", None),
        // Mint and Decimals tie, and a tie is ambiguous rather than a coin flip.
        ("MintDecimalsMismatch", None),
        ("FixedSupply", None),
        ("NativeNotSupported", None),
        ("InvalidState", None),
        ("Zork", None),
    ];
    for (name, want) in cases {
        assert_eq!(diagnose::classify_name(name), *want, "{name}");
    }
}

#[test]
fn notes_dedupe_by_identity_keeping_the_highest_tier() {
    use diagnose::{DiagnosticId, Note, Tier};

    let mut notes = vec![
        Note::new(DiagnosticId::Log("same"), Tier::Help, "help wording".into()),
        Note::new(DiagnosticId::Log("same"), Tier::Fix, "fix wording".into()),
        Note::new(DiagnosticId::Log("other"), Tier::Note, "other".into()),
    ];
    diagnose::finalize(&mut notes);

    assert_eq!(notes.len(), 2, "one id, one note");
    let same = notes.iter().find(|n| n.id == DiagnosticId::Log("same")).unwrap();
    assert_eq!(same.tier, Tier::Fix, "the more specific tier survives");

    // Idempotent, so calling it at every collection boundary is safe.
    let before: Vec<String> = notes.iter().map(|n| n.id.to_string()).collect();
    diagnose::finalize(&mut notes);
    assert_eq!(before, notes.iter().map(|n| n.id.to_string()).collect::<Vec<_>>());
}

#[test]
fn two_missing_signers_both_survive_deduplication() {
    use validate::{FindingKind, Level};

    let mut failure = seeds_failure();
    let findings = [
        finding(FindingKind::NotSigner, 1, "payer", Level::Error),
        finding(FindingKind::NotSigner, 4, "authority", Level::Error),
    ];
    diagnose::add_findings(&mut failure, 0, &findings);

    let signers = failure
        .notes
        .iter()
        .filter(|note| note.text.contains("payer") || note.text.contains("authority"))
        .count();
    assert_eq!(signers, 2, "two accounts, two notes — identity includes the slot");
}

#[test]
fn the_same_code_from_two_programs_is_two_identities() {
    use diagnose::DiagnosticId;

    let one = DiagnosticId::Program {
        program: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".into(),
        code: 6001,
    };
    let two = DiagnosticId::Program {
        program: "Vau1t11111111111111111111111111111111111".into(),
        code: 6001,
    };
    assert_ne!(one, two, "custom codes index each program's own enum");
    assert_eq!(one.to_string(), "token.6001");
    assert_eq!(two.to_string(), "custom.6001");
}

#[test]
fn findings_order_does_not_change_the_diagnosis() {
    use validate::{FindingKind, Level};

    let forward = [
        finding(FindingKind::PdaMismatch, 2, "vault", Level::Error),
        finding(FindingKind::BelowRent, 3, "payer", Level::Warn),
        finding(FindingKind::NotSigner, 1, "authority", Level::Error),
    ];
    let mut reversed = forward.clone();
    reversed.reverse();

    let render = |findings: &[validate::Finding]| {
        let mut failure = seeds_failure();
        diagnose::add_findings(&mut failure, 0, findings);
        failure
            .notes
            .iter()
            .map(|note| format!("{:?} {} {}", note.tier, note.id, note.text))
            .collect::<Vec<_>>()
    };
    assert_eq!(render(&forward), render(&reversed));
}

#[test]
fn evidence_promotes_inferred_advice_to_a_fix() {
    use validate::{FindingKind, Level};

    // A seeds failure corroborated by the derived PDA differing.
    let mut failure = seeds_failure();
    failure.notes.push(diagnose::Note::maybe(
        diagnose::DiagnosticId::Log("class.inferred"),
        diagnose::Tier::Help,
        "inferred advice".into(),
    ));
    diagnose::add_findings(
        &mut failure,
        0,
        &[finding(FindingKind::PdaMismatch, 2, "vault", Level::Error)],
    );

    let inferred = failure
        .notes
        .iter()
        .find(|note| note.id == diagnose::DiagnosticId::Log("class.inferred"))
        .unwrap();
    assert_eq!(
        inferred.confidence,
        diagnose::Confidence::Certain,
        "on-chain evidence turns the guess into a fact"
    );
    let fix = failure
        .notes
        .iter()
        .find(|note| matches!(note.id, diagnose::DiagnosticId::Finding { .. }))
        .unwrap();
    assert_eq!(fix.tier, diagnose::Tier::Fix);
    assert_eq!(fix.expected.as_deref(), Some("want2"));

    // A guess from some other source is not corroborated by this finding, so
    // one piece of evidence must not stamp "certain" on unrelated advice.
    let mut mixed = seeds_failure();
    mixed.notes.push(diagnose::Note::maybe(
        diagnose::DiagnosticId::Log("class.inferred"),
        diagnose::Tier::Help,
        "advice the finding confirms".into(),
    ));
    mixed.notes.push(diagnose::Note::maybe(
        diagnose::DiagnosticId::Log("some.other.guess"),
        diagnose::Tier::Help,
        "advice the finding says nothing about".into(),
    ));
    diagnose::add_findings(
        &mut mixed,
        0,
        &[finding(FindingKind::PdaMismatch, 2, "vault", Level::Error)],
    );

    let by_id = |id| {
        mixed
            .notes
            .iter()
            .find(|note| note.id == diagnose::DiagnosticId::Log(id))
            .unwrap()
            .confidence
    };
    assert_eq!(by_id("class.inferred"), diagnose::Confidence::Certain);
    assert_eq!(by_id("some.other.guess"), diagnose::Confidence::Likely);

    // Uncorroborated, the same advice stays hedged.
    let mut alone = seeds_failure();
    alone.notes.push(diagnose::Note::maybe(
        diagnose::DiagnosticId::Log("class.inferred"),
        diagnose::Tier::Help,
        "inferred advice".into(),
    ));
    diagnose::add_findings(
        &mut alone,
        0,
        &[finding(FindingKind::BelowRent, 3, "payer", Level::Warn)],
    );
    let still = alone
        .notes
        .iter()
        .find(|note| note.id == diagnose::DiagnosticId::Log("class.inferred"))
        .unwrap();
    assert_eq!(still.confidence, diagnose::Confidence::Likely);
}

#[test]
fn the_diagnosis_renders_in_a_fixed_order() {
    let nodes = solana::parse("s", "devnet", &fixture()).nodes;
    let logs = vec![
        "Program log: panicked at 'attempt to subtract with overflow'".to_string(),
        "Program log: Transfer: insufficient lamports 890880, need 2039280".to_string(),
    ];
    let failure = diagnose::failure(
        &json!({"InstructionError": [0, "ProgramFailedToComplete"]}),
        &nodes,
        &logs,
        200_000,
        200_000,
    )
    .unwrap();

    let order: Vec<String> = failure
        .notes
        .iter()
        .map(|note| format!("{:?}/{:?}/{}", note.tier, note.confidence, note.id))
        .collect();
    assert_eq!(
        order,
        vec![
            "Help/Certain/program.panic".to_string(),
            "Help/Certain/rent.shortfall".to_string(),
        ],
        "tier then id, independent of which branch pushed first"
    );
}

#[test]
fn a_narrow_terminal_still_shows_the_whole_diagnosis() {
    let mut app = App::new("devnet".into());
    app.tx = Some(solana::parse("4vK9mP", "devnet", &constraint_fixture()));
    app.kind = solana::Kind::Tx;
    app.goto(Screen::Overview);

    // Width 60 forces notes to wrap; the panel is sized from the wrapped row
    // count, so nothing is cut off.
    let narrow = render(&mut app, 60, 40);
    assert!(narrow.contains("error[anchor.2006]"), "{narrow}");
    assert!(narrow.contains("re-derive"), "help survives wrapping:\n{narrow}");

    let wide = render(&mut app, 160, 40);
    assert!(wide.contains("7xKX9mPaRvQw1bNcTzWd4rYfG6hJ8uL2sE5vA3dC9kFq"));
}

#[test]
fn inferred_advice_renders_as_a_guess_not_an_instruction() {
    let mut app = App::new("devnet".into());
    app.tx = Some(solana::parse("4vK9mP", "devnet", &constraint_fixture()));
    app.kind = solana::Kind::Tx;
    let failure = app.tx.as_mut().unwrap().failure.as_mut().unwrap();
    failure.notes.push(diagnose::Note::maybe(
        diagnose::DiagnosticId::Log("class.inferred"),
        diagnose::Tier::Help,
        "inferred from the name `InvalidOwner` — check which program owns the account you passed".into(),
    ));
    diagnose::finalize(&mut failure.notes);
    app.goto(Screen::Overview);

    let screen = render(&mut app, 140, 34);
    assert!(
        screen.contains("maybe — inferred from the name"),
        "a guess has to read as a guess:\n{screen}"
    );
    // The certain advice on the same diagnosis carries no hedge.
    assert!(screen.contains("= help: re-derive the PDA"), "{screen}");
}

#[test]
fn uncased_error_names_terminate() {
    // word_end used to return `start` for alphanumerics that are neither upper
    // nor lower case, so tokenize looped forever pushing empty strings. Reachable
    // from any IDL or patp.toml error name.
    for name in ["中", "错误", "エラー", "שגיאה", "Ошибка", "Ошибка2"] {
        assert_eq!(diagnose::classify_name(name), None, "{name}");
    }
    // Mixed scripts still tokenize the part we understand.
    assert_eq!(diagnose::classify_name("中Owner"), Some(diagnose::Class::WrongOwner));
}

#[test]
fn cpi_pda_authorities_are_not_reported_as_missing_signers() {
    use crate::chain::solana::{Account, Node, Status};

    let spec = |name: &str| idl::AccountSpec {
        name: name.to_string(),
        signer: true,
        writable: false,
        address: None,
        seeds: None,
    };
    // A PDA is off-curve, so it can never be a transaction signer, but a
    // program can sign for it with invoke_signed. Both keys are verified by
    // idl::off_curve.
    let pda = "3sPq1aBxGv2kkR7ZLczdKJ2w7xkGk4trQJcM3pQWvNnT";
    let keypair = "7xKX9mPaRvQw1bNcTzWd4rYfG6hJ8uL2sE5vA3dC9kFq";
    let node = |depth: usize, key: &str| Node {
        program: "Prog1111111111111111111111111111111111111".to_string(),
        path: "1.0".to_string(),
        label: String::new(),
        depth,
        status: Status::Failed,
        error: None,
        cu: 0,
        self_cu: 0,
        accounts: vec![Account {
            key: key.to_string(),
            signer: false,
            writable: false,
        }],
        data: String::new(),
        name: None,
        children: 0,
        logs: 0..0,
    };

    let specs = [spec("authority")];
    assert!(
        validate::declared_findings(&specs, &node(1, pda)).is_empty(),
        "a CPI can sign for its own PDA"
    );
    assert!(
        !validate::declared_findings(&specs, &node(0, pda)).is_empty(),
        "at depth 0 there is no caller to sign for it"
    );
    assert!(
        !validate::declared_findings(&specs, &node(1, keypair)).is_empty(),
        "an on-curve key is a real keypair that had to sign"
    );
}
