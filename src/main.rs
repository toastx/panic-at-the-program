use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use crossterm::execute;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph};

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Ok,
    Failed,
}

struct Node {
    depth: usize,
    label: &'static str,
    program: &'static str,
    cu: u32,
    status: Status,
    accounts: &'static [(&'static str, Status)],
}

fn trace() -> Vec<Node> {
    vec![
        Node {
            depth: 0,
            label: "Vault::withdraw",
            program: "Vault Program",
            cu: 42_800,
            status: Status::Failed,
            accounts: &[("Vault", Status::Ok), ("Authority", Status::Failed)],
        },
        Node {
            depth: 1,
            label: "Validate authority",
            program: "Vault Program",
            cu: 4_200,
            status: Status::Ok,
            accounts: &[("Authority", Status::Ok)],
        },
        Node {
            depth: 1,
            label: "Verify vault",
            program: "Vault Program",
            cu: 6_100,
            status: Status::Ok,
            accounts: &[("Vault", Status::Ok), ("Vault PDA", Status::Ok)],
        },
        Node {
            depth: 1,
            label: "Token::transfer",
            program: "Token Program",
            cu: 8_420,
            status: Status::Failed,
            accounts: &[
                ("Vault", Status::Ok),
                ("Token account", Status::Ok),
                ("Authority", Status::Failed),
            ],
        },
        Node {
            depth: 2,
            label: "Authority check",
            program: "Token Program",
            cu: 320,
            status: Status::Failed,
            accounts: &[("Authority", Status::Failed)],
        },
    ]
}

fn mark(status: Status) -> Span<'static> {
    match status {
        Status::Ok => Span::styled("✓", Style::new().fg(Color::Green)),
        Status::Failed => Span::styled("✕", Style::new().fg(Color::Red)),
    }
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    let nodes = trace();
    let mut sel = ListState::default().with_selected(Some(0));

    loop {
        term.draw(|f| draw(f, &nodes, &mut sel))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => sel.select_next(),
                KeyCode::Up | KeyCode::Char('k') => sel.select_previous(),
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()
}

fn draw(f: &mut Frame, nodes: &[Node], sel: &mut ListState) {
    let dim = Style::new().fg(Color::DarkGray);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(nodes.len() as u16 + 2),
        Constraint::Length(1),
    ])
    .split(f.area());

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" SOLWORK", Style::new().fg(Color::Cyan).bold()),
            Span::raw("    Cluster: Devnet     TX: 4vK...9mP"),
        ])),
        rows[0],
    );

    let failed = nodes.iter().any(|n| n.status == Status::Failed);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" STATUS  "),
            if failed {
                Span::styled("Failed", Style::new().fg(Color::Red).bold())
            } else {
                Span::styled("Success", Style::new().fg(Color::Green).bold())
            },
            Span::styled("     INSTRUCTIONS  ", dim),
            Span::raw(nodes.len().to_string()),
            Span::styled("     CU USED  ", dim),
            Span::raw(format!("{:.1}k", nodes[0].cu as f64 / 1000.0)),
        ])),
        rows[1],
    );

    let mid = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).split(rows[2]);
    render_trace(f, mid[0], nodes, sel);
    render_detail(f, mid[1], &nodes[sel.selected().unwrap_or(0)]);
    render_compute(f, rows[3], nodes);

    f.render_widget(
        Paragraph::new(" ↑↓ Navigate   Tab Change panel   q Quit").style(dim),
        rows[4],
    );
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
                Span::styled(prefix, Style::new().fg(Color::DarkGray)),
                Span::styled(n.label, Style::new().fg(color)),
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

fn render_detail(f: &mut Frame, area: Rect, node: &Node) {
    let block = Block::new().borders(Borders::ALL).title(" SELECTED ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let parts = Layout::vertical([Constraint::Length(3), Constraint::Length(2), Constraint::Min(1)]).split(inner);

    f.render_widget(
        Paragraph::new(vec![
            Line::styled("PROGRAM", Style::new().fg(Color::DarkGray)),
            Line::raw(node.program),
        ]),
        parts[0],
    );

    f.render_widget(
        Gauge::default()
            .label(format!("{} CU", node.cu))
            .ratio((node.cu as f64 / 42_800.0).min(1.0))
            .gauge_style(Style::new().fg(Color::Cyan)),
        Rect { height: 1, ..parts[1] },
    );

    let mut lines = vec![Line::styled("ACCOUNTS", Style::new().fg(Color::DarkGray))];
    lines.extend(node.accounts.iter().map(|(name, status)| {
        Line::from(vec![mark(*status), Span::raw(format!(" {name}"))])
    }));
    f.render_widget(Paragraph::new(lines), parts[2]);
}

fn render_compute(f: &mut Frame, area: Rect, nodes: &[Node]) {
    let max = nodes.iter().map(|n| n.cu).max().unwrap_or(1).max(1);
    let width = area.width.saturating_sub(28) as u32;
    let lines: Vec<Line> = nodes
        .iter()
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
