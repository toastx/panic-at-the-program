use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::state::{App, Click, clusters};
use crate::app::solana::{Kind, Node, Status, Tx};

/// Semantic colour slots; every UI colour comes from the active theme.
pub struct Theme {
    pub name: &'static str,
    /// Reset = leave the terminal's own background and text colour alone.
    pub bg: Color,
    pub text: Color,
    pub strong: Color,
    pub dim: Color,
    pub muted: Color,
    pub panel: Color,
    pub accent: Color,
    pub ok: Color,
    pub bad: Color,
    pub bad_soft: Color,
    pub bad_dim: Color,
    pub bad_bg: Color,
    pub info: Color,
    pub warn: Color,
    pub surface: Color,
    pub select: Color,
    pub highlight: Color,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

pub const THEMES: [Theme; 3] = [
    Theme {
        name: "dark",
        bg: Color::Reset,
        text: Color::Reset,
        strong: Color::White,
        dim: rgb(176, 180, 210),
        muted: rgb(110, 114, 140),
        panel: rgb(58, 60, 82),
        accent: rgb(139, 122, 255),
        ok: rgb(74, 222, 128),
        bad: rgb(248, 113, 113),
        bad_soft: rgb(233, 164, 170),
        bad_dim: rgb(128, 72, 78),
        bad_bg: rgb(58, 26, 32),
        info: rgb(96, 165, 250),
        warn: rgb(234, 179, 8),
        surface: rgb(32, 33, 48),
        select: rgb(38, 38, 58),
        highlight: rgb(48, 48, 72),
    },
    Theme {
        name: "light",
        bg: rgb(250, 250, 252),
        text: rgb(30, 32, 44),
        strong: rgb(12, 12, 20),
        dim: rgb(70, 74, 96),
        muted: rgb(120, 124, 145),
        panel: rgb(200, 203, 218),
        accent: rgb(98, 78, 230),
        ok: rgb(22, 140, 70),
        bad: rgb(205, 45, 55),
        bad_soft: rgb(175, 60, 72),
        bad_dim: rgb(215, 150, 155),
        bad_bg: rgb(253, 226, 228),
        info: rgb(37, 99, 200),
        warn: rgb(176, 115, 0),
        surface: rgb(234, 235, 243),
        select: rgb(224, 224, 240),
        highlight: rgb(212, 212, 234),
    },
    // Greyscale: state reads from glyphs (✓ ✕ ↯ ⚠) and brightness, not hue.
    Theme {
        name: "mono",
        bg: Color::Reset,
        text: Color::Reset,
        strong: rgb(245, 245, 245),
        dim: rgb(180, 180, 180),
        muted: rgb(120, 120, 120),
        panel: rgb(70, 70, 70),
        accent: rgb(225, 225, 225),
        ok: rgb(160, 160, 160),
        bad: rgb(255, 255, 255),
        bad_soft: rgb(215, 215, 215),
        bad_dim: rgb(105, 105, 105),
        bad_bg: rgb(60, 60, 60),
        info: rgb(190, 190, 190),
        warn: rgb(230, 230, 230),
        surface: rgb(36, 36, 36),
        select: rgb(50, 50, 50),
        highlight: rgb(66, 66, 66),
    },
];

// Global rather than threaded through every render fn: there is one terminal.
static THEME: AtomicUsize = AtomicUsize::new(0);

pub fn theme() -> &'static Theme {
    &THEMES[THEME.load(Ordering::Relaxed) % THEMES.len()]
}

/// Applies the theme saved in the global config, if any.
pub fn load_theme() {
    let saved = crate::workspace::global("ui")
        .into_iter()
        .find(|(key, _)| key == "theme");
    if let Some(index) = saved.and_then(|(_, name)| THEMES.iter().position(|t| t.name == name)) {
        THEME.store(index, Ordering::Relaxed);
    }
}

/// Switches to the next theme and remembers it.
pub fn cycle_theme() -> std::io::Result<()> {
    let next = (THEME.load(Ordering::Relaxed) + 1) % THEMES.len();
    THEME.store(next, Ordering::Relaxed);
    crate::workspace::set_global("ui", "theme", THEMES[next].name)
}

/// Base style for a themed surface: paints the background behind content.
pub fn base() -> Style {
    Style::new().bg(theme().bg).fg(theme().text)
}

pub fn muted() -> Style {
    Style::new().fg(theme().muted)
}

pub fn panel(title: &str, focused: bool) -> Block<'_> {
    let block = Block::new()
        .style(base())
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if focused { theme().accent } else { theme().panel }));
    if title.is_empty() {
        return block;
    }
    block.title(Span::styled(
        format!(" {title} "),
        Style::new()
            .fg(if focused { theme().accent } else { theme().muted })
            .bold(),
    ))
}

pub fn chip(text: &str, color: Color) -> Span<'static> {
    Span::styled(format!(" {text} "), Style::new().fg(color).bg(theme().surface).bold())
}

pub fn short(key: &str) -> String {
    if let Some(label) = crate::workspace::value("accounts", key) {
        return label;
    }
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
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn status_color(status: Status) -> Color {
    match status {
        Status::Ok => theme().ok,
        Status::Failed => theme().bad,
    }
}

/// A bar `cells` wide (fractional, drawn in eighths) on a `width`-cell track.
pub fn bar(cells: f64, width: usize, color: Color) -> Vec<Span<'static>> {
    let solid = smooth(cells.clamp(0.0, width as f64));
    let rest = width - solid.chars().count();
    vec![
        Span::styled(solid, Style::new().fg(color)),
        Span::styled("░".repeat(rest), Style::new().fg(theme().panel)),
    ]
}

/// `cells` worth of █, with the remainder as a partial block (▏▎▍▌▋▊▉).
pub fn smooth(cells: f64) -> String {
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = (cells.max(0.0) * 8.0).round() as usize;
    "█".repeat(eighths / 8) + PARTS[eighths % 8]
}

/// Diverging bar centred on a zero axis: losses grow left in red, gains right
/// in green. `ratio` is the change over the largest change, -1.0 ..= 1.0.
pub fn delta_bar(ratio: f64, width: usize) -> Vec<Span<'static>> {
    let half = width.saturating_sub(1) / 2;
    let cells = (ratio.abs().min(1.0) * half as f64).max(if ratio == 0.0 { 0.0 } else { 0.125 });
    let axis = Span::styled("│", muted());
    if ratio < 0.0 {
        // Partial blocks only fill from the left, so the left side rounds to whole cells.
        let n = (cells.ceil() as usize).min(half);
        vec![
            Span::raw(" ".repeat(half - n)),
            Span::styled("█".repeat(n), Style::new().fg(theme().bad)),
            axis,
        ]
    } else {
        vec![
            Span::raw(" ".repeat(half)),
            axis,
            Span::styled(smooth(cells), Style::new().fg(theme().ok)),
        ]
    }
}

pub fn tree_prefixes(nodes: &[Node]) -> Vec<String> {
    let mut last_at: Vec<bool> = Vec::new();
    let mut out = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.iter().enumerate() {
        let is_last = !nodes[i + 1..]
            .iter()
            .take_while(|n| n.depth >= node.depth)
            .any(|n| n.depth == node.depth);
        last_at.resize(node.depth, true);
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
    let mut spans = vec![
        Span::styled(" PANIC ", Style::new().fg(theme().accent).bold()),
        Span::styled("│", muted()),
    ];

    let x = area.x + spans.iter().map(|s| s.width() as u16).sum::<u16>();
    let label = format!(" ● {} {} ", app.cluster, if app.cluster_menu { "▴" } else { "▾" });
    let width = label.chars().count() as u16;
    spans.push(Span::styled(
        label,
        Style::new().fg(theme().ok).bg(theme().surface).bold(),
    ));
    app.cluster_button = Rect {
        x,
        y: area.y,
        width,
        height: 1,
    };
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
            spans.extend(bar(pct * 10.0 * app.load_anim(), 10, theme().accent));
        }
        (Kind::Account, _, Some(account)) => {
            spans.extend([
                Span::raw(format!(" {} ", short(&account.key))),
                chip(if account.executable { "program" } else { "account" }, theme().info),
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
        let button_area = Rect {
            x: area.x + area.width - width,
            y: area.y,
            width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                button,
                Style::new().fg(theme().accent).bg(theme().surface).bold(),
            )),
            button_area,
        );
        app.push_hit(button_area, Click::Explorer);
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            width: area.width.saturating_sub(width + 1),
            ..area
        },
    );
}

pub fn render_cluster_menu(f: &mut Frame, app: &mut App) {
    let clusters = clusters();
    let anchor = app.cluster_button;
    let longest = clusters.iter().map(|c| c.chars().count()).max().unwrap_or(0) as u16;
    let width = (longest + 5).max(16).min(f.area().width);
    let area = Rect {
        x: anchor.x.min(f.area().width.saturating_sub(width)),
        y: anchor.y + 1,
        width,
        height: (clusters.len() as u16 + 2).min(f.area().height.saturating_sub(anchor.y + 1)),
    };
    f.render_widget(Clear, area);
    let block = panel("NETWORK", true);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = clusters
        .iter()
        .enumerate()
        .map(|(i, cluster)| {
            let selected = i == app.cluster_index;
            let active = app.cluster == *cluster;
            Line::from(vec![
                Span::styled(if active { " ● " } else { "   " }, Style::new().fg(theme().ok)),
                Span::styled(
                    format!("{cluster:<11}"),
                    if selected {
                        Style::new().fg(theme().accent).bg(theme().select).bold()
                    } else {
                        Style::new()
                    },
                ),
            ])
        })
        .collect();

    for (i, cluster) in clusters.into_iter().enumerate() {
        let y = inner.y + i as u16;
        if y < inner.y + inner.height {
            app.push_hit(
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
                Click::Cluster(cluster),
            );
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn nav_help(f: &mut Frame, area: Rect, keys: &[(&str, &str)]) {
    f.render_widget(
        Paragraph::new(
            keys.iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(format!("{k:>5} "), Style::new().fg(theme().accent)),
                        Span::styled(*v, muted()),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
        area,
    );
}

pub fn render_nav(f: &mut Frame, area: Rect, app: &mut App) {
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

    // A header row wherever the test run changes; `rows_to_entries` maps each
    // list row back to its entry, None for a header.
    let mut rows_to_entries = Vec::new();
    let mut items: Vec<ListItem> = Vec::new();
    let mut previous = None;
    for (index, entry) in app.entries.iter().enumerate() {
        if entry.run != previous {
            let label = entry
                .run
                .map_or_else(|| "earlier".to_string(), |run| format!("test run {run}"));
            items.push(ListItem::new(Line::styled(format!("▾ {label}"), muted())));
            rows_to_entries.push(None);
        }
        previous = entry.run;
        rows_to_entries.push(Some(index));
        items.push({
            // A test run's entries are keyed by their test name; anything else by an address.
            let name = match entry.run {
                Some(_) => entry
                    .key
                    .split_once('/')
                    .map_or(entry.key.clone(), |(_, test)| test.to_string()),
                None => short(&entry.key),
            };
            let (tag, color) = match entry.kind {
                Kind::Tx => ("tx:", theme().info),
                Kind::Account => ("acc:", theme().accent),
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    match entry.status {
                        Some(Status::Ok) => "✓ ",
                        Some(Status::Failed) => "✕ ",
                        None => "· ",
                    },
                    Style::new().fg(entry.status.map_or(theme().muted, status_color)),
                ),
                Span::styled(tag, Style::new().fg(color)),
                Span::raw(name),
            ]))
        });
    }

    let selected = rows_to_entries.iter().position(|row| *row == Some(app.entry_index));
    let mut state = ListState::default().with_selected(selected);
    f.render_stateful_widget(
        List::new(items).highlight_style(if app.focus_nav {
            Style::new().bg(theme().select).bold()
        } else {
            Style::new().bold()
        }),
        rows[0],
        &mut state,
    );

    // Hits after rendering, so they follow the list's scroll offset.
    let visible = rows_to_entries
        .iter()
        .skip(state.offset())
        .take(rows[0].height as usize);
    for (row, entry) in visible.enumerate() {
        if let Some(index) = entry {
            app.push_hit(
                Rect {
                    x: rows[0].x,
                    y: rows[0].y + row as u16,
                    width: rows[0].width,
                    height: 1,
                },
                Click::Entry(*index),
            );
        }
    }

    if help_height > 0 {
        nav_help(
            f,
            rows[1],
            &[
                ("Enter", "open"),
                ("Tab", "panel"),
                ("o", "load"),
                ("[ ]", "network"),
                ("?", "help"),
            ],
        );
    }
}

pub fn node_label(node: &Node) -> String {
    match &node.name {
        Some(name) => format!("{} · {name}", node.label),
        None => node.label.clone(),
    }
}

pub fn render_compute_chart(f: &mut Frame, area: Rect, tx: &Tx, selected: usize, focused: bool, progress: f64) {
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
            theme().accent
        } else if node.status == Status::Failed {
            theme().bad
        } else {
            theme().info
        };
        let cells = node.self_cu as f64 * bar_width as f64 / max as f64 * progress;
        let mut spans = vec![Span::styled(
            format!("{label:<label_width$.label_width$}"),
            if i == selected {
                Style::new().fg(theme().accent).bold()
            } else {
                Style::new()
            },
        )];
        // Any nonzero cost keeps at least a sliver, so it never reads as zero.
        let min = if node.self_cu > 0 { 0.125 } else { 0.0 };
        // Pad to the full track so the value sits in a fixed column while the bar grows.
        let solid = smooth(cells.max(min));
        let pad = bar_width.saturating_sub(solid.chars().count());
        spans.push(Span::styled(solid, Style::new().fg(color)));
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::raw(format!(" {:>value_width$}", thousands(node.self_cu as u64))));
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
            format!(
                "{} of {} CU limit",
                thousands(tx.cu as u64),
                thousands(tx.cu_limit as u64)
            ),
            Style::new().fg(theme().accent).bold(),
        ),
    ]));

    f.render_widget(Paragraph::new(lines), inner);
}

pub fn wrapped(text: &str, style: Style) -> Paragraph<'static> {
    Paragraph::new(text.to_string()).style(style).wrap(Wrap { trim: true })
}
