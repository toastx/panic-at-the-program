use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{App, Screen};
use crate::solana::{Node, Status, Tx};

pub const ACCENT: Color = Color::Rgb(139, 122, 255);
pub const OK: Color = Color::Rgb(74, 222, 128);
pub const BAD: Color = Color::Rgb(248, 113, 113);
pub const MUTED: Color = Color::Rgb(110, 114, 140);
pub const PANEL: Color = Color::Rgb(58, 60, 82);

pub fn muted() -> Style {
    Style::new().fg(MUTED)
}

pub fn panel(title: &str, focused: bool) -> Block<'_> {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if focused { ACCENT } else { PANEL }));
    if title.is_empty() {
        return block;
    }
    block.title(Span::styled(
        format!(" {title} "),
        Style::new().fg(if focused { ACCENT } else { MUTED }).bold(),
    ))
}

pub fn chip(text: &str, color: Color) -> Span<'static> {
    Span::styled(format!(" {text} "), Style::new().fg(color).bg(Color::Rgb(32, 33, 48)).bold())
}

pub fn short(key: &str) -> String {
    if key.len() > 12 {
        format!("{}…{}", &key[..4], &key[key.len() - 4..])
    } else {
        key.to_string()
    }
}

pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn status_color(status: Status) -> Color {
    match status {
        Status::Ok => OK,
        Status::Failed => BAD,
    }
}

pub fn bar(filled: usize, width: usize, color: Color) -> Vec<Span<'static>> {
    let filled = filled.min(width);
    vec![
        Span::styled("█".repeat(filled), Style::new().fg(color)),
        Span::styled("░".repeat(width - filled), Style::new().fg(PANEL)),
    ]
}

pub fn tree_prefixes(nodes: &[Node]) -> Vec<String> {
    let mut last_at: Vec<bool> = Vec::new();
    let mut out = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.iter().enumerate() {
        let is_last = !nodes[i + 1..]
            .iter()
            .take_while(|n| n.depth >= node.depth)
            .any(|n| n.depth == node.depth);
        last_at.truncate(node.depth);
        while last_at.len() < node.depth {
            last_at.push(true);
        }
        let mut prefix = String::new();
        for ancestor_last in &last_at {
            prefix.push_str(if *ancestor_last { "   " } else { "│  " });
        }
        if node.depth > 0 {
            prefix.push_str(if is_last { "└─ " } else { "├─ " });
        }
        last_at.push(is_last);
        out.push(prefix);
    }
    out
}

pub fn render_topbar(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled(" SOLWORK ", Style::new().fg(ACCENT).bold()),
        Span::styled("│ ", muted()),
        Span::styled(format!("● {} ", app.cluster), Style::new().fg(OK)),
    ];
    if let Some(tx) = &app.tx {
        let pct = tx.cu as f64 / tx.cu_limit.max(1) as f64;
        spans.extend([
            Span::styled("│ ", muted()),
            Span::raw(format!("{} ", short(&tx.signature))),
            chip(
                match tx.status {
                    Status::Ok => "✓ Success",
                    Status::Failed => "✕ Failed",
                },
                status_color(tx.status),
            ),
            Span::styled("  Slot ", muted()),
            Span::raw(thousands(tx.slot)),
            Span::styled("  Fee ", muted()),
            Span::raw(format!("{:.9} SOL", tx.fee as f64 / 1e9)),
            Span::styled("  CU ", muted()),
            Span::raw(format!(
                "{}/{} ({:.0}%) ",
                thousands(tx.cu as u64),
                thousands(tx.cu_limit as u64),
                pct * 100.0
            )),
        ]);
        spans.extend(bar((pct * 10.0) as usize, 10, ACCENT));
    } else {
        spans.push(Span::styled("│ no transaction loaded — press o", muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn render_nav(f: &mut Frame, area: Rect, app: &mut App) {
    let block = panel("WORKSPACE", app.focus_nav);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let help_height = if inner.height >= 16 { 8 } else { 0 };
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(help_height)]).split(inner);
    let items: Vec<ListItem> = Screen::ALL
        .iter()
        .map(|s| {
            ListItem::new(Line::from(vec![
                Span::raw(if *s == app.screen { "▸ " } else { "  " }),
                Span::raw(s.title()),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.nav_index));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().fg(ACCENT).bold()),
        rows[0],
        &mut state,
    );

    let keys = [
        ("↑↓", "Navigate"),
        ("←→", "Collapse"),
        ("Tab", "Panel"),
        ("/", "Search"),
        ("o", "Open tx"),
        ("?", "Help"),
    ];
    if help_height == 0 {
        return;
    }
    f.render_widget(
        Paragraph::new(
            keys.iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(format!("{k:>4} "), Style::new().fg(ACCENT)),
                        Span::styled(*v, muted()),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
        rows[1],
    );
}

pub fn render_compute_chart(f: &mut Frame, area: Rect, tx: &Tx, selected: usize, focused: bool) {
    let block = panel("COMPUTE UNITS BY INSTRUCTION (SELF COST)", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 {
        return;
    }

    let label_width = 22usize;
    let value_width = 9usize;
    let bar_width = (inner.width as usize).saturating_sub(label_width + value_width + 2);
    let max = tx.nodes.iter().map(|n| n.self_cu).max().unwrap_or(1).max(1);
    let rows = (inner.height as usize).saturating_sub(2);

    let mut lines: Vec<Line> = Vec::new();
    for (i, node) in tx.nodes.iter().enumerate().take(rows) {
        let label = format!("{}. {}", node.path, node.label);
        let color = if i == selected {
            ACCENT
        } else if node.status == Status::Failed {
            BAD
        } else {
            Color::Rgb(96, 165, 250)
        };
        let filled = (node.self_cu as usize * bar_width) / max as usize;
        let mut spans = vec![Span::styled(
            format!("{label:<label_width$.label_width$}"),
            if i == selected { Style::new().fg(ACCENT).bold() } else { Style::new() },
        )];
        spans.push(Span::styled("█".repeat(filled.max(usize::from(node.self_cu > 0))), Style::new().fg(color)));
        spans.push(Span::raw(format!(" {}", thousands(node.self_cu as u64))));
        lines.push(Line::from(spans));
    }

    let axis_ticks = 4;
    let mut axis = format!("{:<label_width$}", "");
    for t in 0..=axis_ticks {
        let value = max as usize * t / axis_ticks;
        let tick = format!("{}k", value / 1000);
        let position = label_width + bar_width * t / axis_ticks;
        while axis.chars().count() < position.saturating_sub(tick.len() / 2) {
            axis.push(' ');
        }
        axis.push_str(&tick);
    }
    lines.push(Line::styled(axis, muted()));
    lines.push(Line::from(vec![
        Span::styled(format!("{:<label_width$}", "TOTAL (inclusive)"), muted()),
        Span::styled(
            format!("{} of {} CU limit", thousands(tx.cu as u64), thousands(tx.cu_limit as u64)),
            Style::new().fg(ACCENT).bold(),
        ),
    ]));

    f.render_widget(Paragraph::new(lines), inner);
}

pub fn wrapped(text: &str, style: Style) -> Paragraph<'static> {
    Paragraph::new(text.to_string()).style(style).wrap(Wrap { trim: true })
}
