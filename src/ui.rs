use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{App, CLUSTERS, Click, Sidebar};
use crate::solana::{Kind, Node, Status, Tx};

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

pub fn render_topbar(f: &mut Frame, area: Rect, app: &mut App) {
    let mut spans = vec![Span::styled(" SOLWORK ", Style::new().fg(ACCENT).bold()), Span::styled("│", muted())];

    let x = area.x + spans.iter().map(|s| s.width() as u16).sum::<u16>();
    let label = format!(" ● {} {} ", app.cluster, if app.cluster_menu { "▴" } else { "▾" });
    let width = label.chars().count() as u16;
    spans.push(Span::styled(label, Style::new().fg(OK).bg(Color::Rgb(32, 33, 48)).bold()));
    app.cluster_button = Rect { x, y: area.y, width, height: 1 };
    app.push_hit(app.cluster_button, Click::ClusterMenu);
    spans.push(Span::styled("│", muted()));

    match (app.kind, app.tx.as_ref(), app.account.as_ref()) {
        (Kind::Tx, Some(tx), _) => {
            let pct = tx.cu as f64 / tx.cu_limit.max(1) as f64;
            spans.extend([
                Span::raw(format!(" {} ", short(&tx.signature))),
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
        }
        (Kind::Account, _, Some(account)) => {
            spans.extend([
                Span::raw(format!(" {} ", short(&account.key))),
                chip(
                    if account.executable { "program" } else { "account" },
                    Color::Rgb(96, 165, 250),
                ),
                Span::styled("  Balance ", muted()),
                Span::raw(format!("{:.9} SOL", account.lamports as f64 / 1e9)),
                Span::styled("  Owner ", muted()),
                Span::raw(account.owner_name()),
                Span::styled("  Size ", muted()),
                Span::raw(format!("{} B", thousands(account.data.len() as u64))),
            ]);
        }
        _ => spans.push(Span::styled(
            " nothing loaded — press o or pick a cached entry",
            muted(),
        )),
    }

    let button = " ↗ Explorer ";
    let width = button.chars().count() as u16;
    if area.width > width + 4 {
        let button_area = Rect { x: area.x + area.width - width, y: area.y, width, height: 1 };
        f.render_widget(
            Paragraph::new(Span::styled(button, Style::new().fg(ACCENT).bg(Color::Rgb(32, 33, 48)).bold())),
            button_area,
        );
        app.push_hit(button_area, Click::Explorer);
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { width: area.width.saturating_sub(width + 1), ..area },
    );
}

pub fn render_cluster_menu(f: &mut Frame, app: &mut App) {
    let anchor = app.cluster_button;
    let width = 16.min(f.area().width);
    let area = Rect {
        x: anchor.x.min(f.area().width.saturating_sub(width)),
        y: anchor.y + 1,
        width,
        height: (CLUSTERS.len() as u16 + 2).min(f.area().height.saturating_sub(anchor.y + 1)),
    };
    f.render_widget(Clear, area);
    let block = panel("NETWORK", true);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = CLUSTERS
        .iter()
        .enumerate()
        .map(|(i, cluster)| {
            let selected = i == app.cluster_index;
            let active = app.cluster == *cluster;
            Line::from(vec![
                Span::styled(if active { " ● " } else { "   " }, Style::new().fg(OK)),
                Span::styled(
                    format!("{cluster:<11}"),
                    if selected {
                        Style::new().fg(ACCENT).bg(Color::Rgb(38, 38, 58)).bold()
                    } else {
                        Style::new()
                    },
                ),
            ])
        })
        .collect();

    for (i, cluster) in CLUSTERS.iter().enumerate() {
        let y = inner.y + i as u16;
        if y < inner.y + inner.height {
            app.push_hit(Rect { x: inner.x, y, width: inner.width, height: 1 }, Click::Cluster(cluster));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

pub fn render_nav(f: &mut Frame, area: Rect, app: &mut App) {
    match app.sidebar {
        Sidebar::Entities => render_entities(f, area, app),
        Sidebar::Screens => render_screens(f, area, app),
    }
}

fn nav_help(f: &mut Frame, area: Rect, keys: &[(&str, &str)]) {
    f.render_widget(
        Paragraph::new(
            keys.iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(format!("{k:>5} "), Style::new().fg(ACCENT)),
                        Span::styled(*v, muted()),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
        area,
    );
}

fn render_entities(f: &mut Frame, area: Rect, app: &mut App) {
    let block = panel("HISTORY", app.focus_nav);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let help_height = if inner.height >= 14 { 5 } else { 0 };
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(help_height)]).split(inner);

    if app.entries.is_empty() {
        f.render_widget(
            Paragraph::new("nothing cached\n\npress o and paste a\nsignature or address")
                .style(muted())
                .wrap(Wrap { trim: true }),
            rows[0],
        );
    }

    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|entry| {
            let (tag, color) = match entry.kind {
                Kind::Tx => ("tx:", Color::Rgb(96, 165, 250)),
                Kind::Account => ("acc:", ACCENT),
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    match entry.status {
                        Some(status) => {
                            if status == crate::solana::Status::Ok {
                                "✓ "
                            } else {
                                "✕ "
                            }
                        }
                        None => "· ",
                    },
                    Style::new().fg(entry.status.map_or(MUTED, status_color)),
                ),
                Span::styled(tag, Style::new().fg(color)),
                Span::raw(short(&entry.key)),
            ]))
        })
        .collect();

    for row in 0..items.len().min(rows[0].height as usize) {
        app.push_hit(
            Rect { x: rows[0].x, y: rows[0].y + row as u16, width: rows[0].width, height: 1 },
            Click::Entry(row),
        );
    }

    let mut state = ListState::default().with_selected(Some(app.entry_index));
    f.render_stateful_widget(
        List::new(items).highlight_style(if app.focus_nav {
            Style::new().bg(Color::Rgb(38, 38, 58)).bold()
        } else {
            Style::new().bold()
        }),
        rows[0],
        &mut state,
    );

    if help_height > 0 {
        nav_help(
            f,
            rows[1],
            &[("Enter", "open"), ("o", "load"), ("c", "cluster"), ("?", "help")],
        );
    }
}

fn render_screens(f: &mut Frame, area: Rect, app: &mut App) {
    let title = match app.loaded_key() {
        Some(key) => short(&key),
        None => "—".to_string(),
    };
    let block = panel(&title, app.focus_nav);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let help_height = if inner.height >= 14 { 5 } else { 0 };
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(1), Constraint::Length(help_height)])
        .split(inner);

    let back = Rect { x: rows[0].x, y: rows[0].y, width: rows[0].width, height: 1 };
    f.render_widget(
        Paragraph::new(Line::styled("← History", Style::new().fg(ACCENT))),
        back,
    );
    app.push_hit(back, Click::Back);

    let screens = app.screens();
    let items: Vec<ListItem> = screens
        .iter()
        .map(|screen| ListItem::new(Line::raw(format!("  {}", screen.title()))))
        .collect();

    for (i, screen) in screens.iter().enumerate() {
        let y = rows[1].y + i as u16;
        if y < rows[1].y + rows[1].height {
            app.push_hit(Rect { x: rows[1].x, y, width: rows[1].width, height: 1 }, Click::Workspace(*screen));
        }
    }

    let mut state = ListState::default().with_selected(Some(app.screen_index()));
    f.render_stateful_widget(
        List::new(items).highlight_style(if app.focus_nav {
            Style::new().fg(ACCENT).bg(Color::Rgb(38, 38, 58)).bold()
        } else {
            Style::new().fg(ACCENT).bold()
        }),
        rows[1],
        &mut state,
    );

    if help_height > 0 {
        nav_help(
            f,
            rows[2],
            &[("Enter", "inspect"), ("Esc", "history"), ("x", "explorer"), ("?", "help")],
        );
    }
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
