mod solana;

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph};

use solana::{Node, Status, Summary, Tx};

struct App {
    cluster: String,
    tx: Option<Tx>,
    trace: ListState,
    history: Vec<Summary>,
    history_state: ListState,
    message: String,
}

impl App {
    fn open(&mut self, signature: &str) {
        self.message = format!("loading {signature}…");
        match solana::fetch(&self.cluster, signature) {
            Ok(value) => {
                self.tx = Some(solana::parse(signature, &self.cluster, &value));
                self.trace.select(Some(0));
                self.history = solana::summaries();
                self.message.clear();
            }
            Err(err) => self.message = err.to_string(),
        }
    }
}

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let signature = args.next();
    let cluster = args.next().unwrap_or_else(|| "devnet".to_string());

    let mut app = App {
        cluster,
        tx: None,
        trace: ListState::default().with_selected(Some(0)),
        history: solana::summaries(),
        history_state: ListState::default().with_selected(Some(0)),
        message: String::new(),
    };
    if let Some(signature) = &signature {
        app.open(signature);
    }

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    loop {
        term.draw(|f| draw(f, &mut app))?;

        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let list = if app.tx.is_some() { &mut app.trace } else { &mut app.history_state };
        match key.code {
            KeyCode::Char('q') => break,
            KeyCode::Down | KeyCode::Char('j') => list.select_next(),
            KeyCode::Up | KeyCode::Char('k') => list.select_previous(),
            KeyCode::Char('h') | KeyCode::Esc => {
                app.tx = None;
                app.history = solana::summaries();
            }
            KeyCode::Enter => {
                if app.tx.is_none() {
                    if let Some(signature) = app
                        .history_state
                        .selected()
                        .and_then(|i| app.history.get(i))
                        .map(|s| s.signature.clone())
                    {
                        app.open(&signature);
                    }
                }
            }
            _ => {}
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

fn mark(status: Status) -> Span<'static> {
    match status {
        Status::Ok => Span::styled("✓", Style::new().fg(Color::Green)),
        Status::Failed => Span::styled("✕", Style::new().fg(Color::Red)),
    }
}

fn short(signature: &str) -> String {
    if signature.len() > 12 {
        format!("{}…{}", &signature[..4], &signature[signature.len() - 4..])
    } else {
        signature.to_string()
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    match &app.tx {
        Some(tx) if !tx.nodes.is_empty() => draw_execution(f, tx, &mut app.trace, &app.message),
        _ => draw_history(f, app),
    }
}

fn draw_history(f: &mut Frame, app: &mut App) {
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(3), Constraint::Length(1)]).split(f.area());

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" SOLWORK", Style::new().fg(Color::Cyan).bold()),
            Span::raw(format!("    Cluster: {}    CACHED QUERIES: {}", app.cluster, app.history.len())),
        ])),
        rows[0],
    );

    let items: Vec<ListItem> = app
        .history
        .iter()
        .map(|s| {
            ListItem::new(Line::from(vec![
                mark(s.status),
                Span::raw(format!(" {:<16}", short(&s.signature))),
                Span::styled(format!("{:>3} ix   ", s.instructions), dim()),
                Span::raw(format!("{:>7.1}k CU", s.cu as f64 / 1000.0)),
            ]))
        })
        .collect();
    let items = if items.is_empty() {
        vec![ListItem::new(Span::styled("no cached transactions — run: solwork <signature> [cluster]", dim()))]
    } else {
        items
    };
    f.render_stateful_widget(
        List::new(items)
            .block(Block::new().borders(Borders::ALL).title(" HISTORY "))
            .highlight_style(Style::new().bg(Color::Rgb(40, 40, 60)).bold()),
        rows[1],
        &mut app.history_state,
    );

    let footer = if app.message.is_empty() {
        Paragraph::new(" ↑↓ Navigate   Enter Open   q Quit").style(dim())
    } else {
        Paragraph::new(format!(" {}", app.message)).style(Style::new().fg(Color::Red))
    };
    f.render_widget(footer, rows[2]);
}

fn draw_execution(f: &mut Frame, tx: &Tx, sel: &mut ListState, message: &str) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(tx.nodes.len().min(8) as u16 + 2),
        Constraint::Length(1),
    ])
    .split(f.area());

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" SOLWORK", Style::new().fg(Color::Cyan).bold()),
            Span::raw(format!("    Cluster: {}     TX: {}", tx.cluster, short(&tx.signature))),
        ])),
        rows[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" STATUS  "),
            match tx.status {
                Status::Failed => Span::styled("Failed", Style::new().fg(Color::Red).bold()),
                Status::Ok => Span::styled("Success", Style::new().fg(Color::Green).bold()),
            },
            Span::styled("     INSTRUCTIONS  ", dim()),
            Span::raw(tx.nodes.iter().filter(|n| n.depth == 0).count().to_string()),
            Span::styled("     CU USED  ", dim()),
            Span::raw(format!("{:.1}k", tx.cu as f64 / 1000.0)),
        ])),
        rows[1],
    );

    let mid = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).split(rows[2]);
    render_trace(f, mid[0], &tx.nodes, sel);
    render_detail(f, mid[1], &tx.nodes[sel.selected().unwrap_or(0).min(tx.nodes.len() - 1)], tx.cu);
    render_compute(f, rows[3], &tx.nodes);

    let footer = if message.is_empty() {
        Paragraph::new(" ↑↓ Navigate   h History   q Quit").style(dim())
    } else {
        Paragraph::new(format!(" {message}")).style(Style::new().fg(Color::Red))
    };
    f.render_widget(footer, rows[4]);
}

fn render_trace(f: &mut Frame, area: Rect, nodes: &[Node], sel: &mut ListState) {
    let items: Vec<ListItem> = nodes
        .iter()
        .map(|n| {
            let prefix = if n.depth == 0 {
                "▼ ".to_string()
            } else {
                format!("{}└─ ", "   ".repeat(n.depth - 1))
            };
            let color = if n.status == Status::Failed { Color::Red } else { Color::Reset };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, dim()),
                Span::styled(n.label.clone(), Style::new().fg(color)),
            ]))
        })
        .collect();

    f.render_stateful_widget(
        List::new(items)
            .block(Block::new().borders(Borders::ALL).title(" EXECUTION TRACE "))
            .highlight_style(Style::new().bg(Color::Rgb(40, 40, 60)).bold()),
        area,
        sel,
    );
}

fn render_detail(f: &mut Frame, area: Rect, node: &Node, total_cu: u32) {
    let block = Block::new().borders(Borders::ALL).title(" SELECTED ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let parts = Layout::vertical([Constraint::Length(3), Constraint::Length(2), Constraint::Min(1)]).split(inner);

    f.render_widget(
        Paragraph::new(vec![
            Line::styled("PROGRAM", dim()),
            Line::raw(node.program.clone()),
        ]),
        parts[0],
    );

    f.render_widget(
        Gauge::default()
            .label(format!("{} CU", node.cu))
            .ratio((node.cu as f64 / total_cu.max(1) as f64).min(1.0))
            .gauge_style(Style::new().fg(Color::Cyan)),
        Rect { height: 1, ..parts[1] },
    );

    let mut lines = vec![Line::styled("ACCOUNTS", dim())];
    lines.extend(node.accounts.iter().map(|(key, status)| {
        Line::from(vec![mark(*status), Span::raw(format!(" {}", short(key)))])
    }));
    f.render_widget(Paragraph::new(lines), parts[2]);
}

fn render_compute(f: &mut Frame, area: Rect, nodes: &[Node]) {
    let max = nodes.iter().map(|n| n.cu).max().unwrap_or(1).max(1);
    let width = area.width.saturating_sub(28) as u32;
    let lines: Vec<Line> = nodes
        .iter()
        .take(8)
        .map(|n| {
            let filled = (n.cu * width / max) as usize;
            Line::from(vec![
                Span::raw(format!("{:<14.14}", n.label)),
                Span::styled("█".repeat(filled), Style::new().fg(Color::Cyan)),
                Span::raw(format!("  {:.1}k", n.cu as f64 / 1000.0)),
            ])
        })
        .collect();

    f.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(" COMPUTE BY INSTRUCTION ")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    fn fixture() -> serde_json::Value {
        json!({"result": {
            "meta": {
                "err": {"InstructionError": [0, "Custom"]},
                "computeUnitsConsumed": 12620,
                "innerInstructions": [{"index": 0, "instructions": [{"programIdIndex": 3, "accounts": [0, 2]}]}],
                "logMessages": [
                    "Program Vau1t11111111111111111111111111111111111 invoke [1]",
                    "Program log: withdraw",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [2]",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA consumed 8420 of 200000 compute units",
                    "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA failed: custom program error: 0x4",
                    "Program Vau1t11111111111111111111111111111111111 consumed 12620 of 200000 compute units",
                    "Program Vau1t11111111111111111111111111111111111 failed: custom program error: 0x4"
                ]
            },
            "transaction": {"message": {
                "accountKeys": ["Auth1111", "Vau1t111", "TokenAcct1111", "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"],
                "instructions": [{"programIdIndex": 3, "accounts": [0, 1, 2]}]
            }}
        }})
    }

    #[test]
    fn parses_cpi_tree_from_logs() {
        let tx = solana::parse("4vK9mP", "devnet", &fixture());
        assert_eq!(tx.nodes.len(), 2);
        assert_eq!(tx.nodes[0].depth, 0);
        assert_eq!(tx.nodes[1].depth, 1);
        assert_eq!(tx.nodes[1].label, "Token Program");
        assert_eq!(tx.nodes[1].cu, 8420);
        assert_eq!(tx.nodes[0].cu, 12620);
        assert!(tx.nodes.iter().all(|n| n.status == Status::Failed));
        assert_eq!(tx.nodes[1].accounts.len(), 2);
        assert_eq!(tx.status, Status::Failed);
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
        assert!(solana::load(signature).is_some());
    }

    #[test]
    fn renders_execution_screen() {
        let tx = solana::parse("4vK9mP", "devnet", &fixture());
        let mut sel = ListState::default().with_selected(Some(1));
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw_execution(f, &tx, &mut sel, "")).unwrap();

        let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Token Program"));
        assert!(text.contains("8420 CU"));
        assert!(text.contains("Failed"));
    }
}
