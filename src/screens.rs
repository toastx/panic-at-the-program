use ratatui::prelude::*;
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Click, Mode, Screen};
use crate::solana::{Status, Tx};
use crate::ui::*;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)]).split(f.area());
    render_topbar(f, rows[0], app);

    let wide = rows[1].width >= 108;
    let detail = wide && matches!(app.screen, Screen::Overview | Screen::Execution) && app.tx.is_some();
    let _ = wide;
    let columns = Layout::horizontal([
        Constraint::Length(if rows[1].width >= 90 { 22 } else { 0 }),
        Constraint::Min(30),
        Constraint::Length(if detail { 38 } else { 0 }),
    ])
    .split(rows[1]);

    if columns[0].width > 0 {
        render_nav(f, columns[0], app);
    }

    let body = columns[1];

    match app.screen {
        Screen::Overview => overview(f, body, app),
        Screen::Execution => execution(f, body, app),
        Screen::Accounts => accounts(f, body, app),
        Screen::StateDiff => state_diff(f, body, app),
        Screen::Logs => logs(f, body, app),
        Screen::Raw => raw(f, body, app),
        Screen::Info => account_info(f, body, app),
        Screen::Data => account_data(f, body, app),
    }

    if detail {
        detail_panel(f, columns[2], app);
    }
    footer(f, rows[2], app);

    if app.cluster_menu {
        render_cluster_menu(f, app);
    }
    if matches!(app.mode, Mode::Help) {
        help(f, f.area());
    }
}

fn footer(f: &mut Frame, area: Rect, app: &App) {
    let line = match &app.mode {
        Mode::Search => Line::from(vec![
            Span::styled(" /", Style::new().fg(ACCENT).bold()),
            Span::raw(app.search.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
            Span::styled(format!("  {} match(es)  Enter keep · Esc clear", app.match_count()), muted()),
        ]),
        Mode::Open(input) => Line::from(vec![
            Span::styled(" signature › ", Style::new().fg(ACCENT).bold()),
            Span::raw(input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]),
        _ if !app.message.is_empty() => Line::styled(format!(" {}", app.message), Style::new().fg(BAD)),
        _ if !app.search.is_empty() => Line::from(vec![
            Span::styled(" filter ", Style::new().fg(ACCENT).bold()),
            Span::raw(app.search.clone()),
            Span::styled(
                format!("  {} match(es)   n next · Esc clear", app.match_count()),
                muted(),
            ),
        ]),
        _ => Line::from(vec![
            Span::styled(" ↑↓", Style::new().fg(ACCENT)),
            Span::styled(" navigate  ", muted()),
            Span::styled("←→", Style::new().fg(ACCENT)),
            Span::styled(" collapse  ", muted()),
            Span::styled("Enter", Style::new().fg(ACCENT)),
            Span::styled(
                if app.focus_nav { " open  " } else { " inspect account  " },
                muted(),
            ),
            Span::styled("Esc", Style::new().fg(ACCENT)),
            Span::styled(" back  ", muted()),
            Span::styled("/", Style::new().fg(ACCENT)),
            Span::styled(" search  ", muted()),
            Span::styled("o", Style::new().fg(ACCENT)),
            Span::styled(" load  ", muted()),
            Span::styled("q", Style::new().fg(ACCENT)),
            Span::styled(" quit", muted()),
        ]),
    };
    f.render_widget(Paragraph::new(line), area);
}

fn require_tx<'a>(f: &mut Frame, area: Rect, app: &'a App, title: &str) -> Option<(&'a Tx, Rect)> {
    let block = panel(title, !app.focus_nav);
    let inner = block.inner(area);
    f.render_widget(block, area);
    match app.tx() {
        Some(tx) => Some((tx, inner)),
        None => {
            f.render_widget(wrapped("no transaction loaded — press o", muted()), inner);
            None
        }
    }
}

fn overview(f: &mut Frame, area: Rect, app: &mut App) {
    let Some(tx) = app.tx() else {
        let _ = require_tx(f, area, app, "OVERVIEW");
        return;
    };

    let rows = Layout::vertical([Constraint::Length(4), Constraint::Length(4), Constraint::Min(6)]).split(area);

    let banner_color = status_color(tx.status);
    let banner = panel("RESULT", false).border_style(Style::new().fg(banner_color));
    let banner_inner = banner.inner(rows[0]);
    f.render_widget(banner, rows[0]);
    let headline = match tx.status {
        Status::Ok => "Transaction Succeeded".to_string(),
        Status::Failed => "Transaction Failed".to_string(),
    };
    let detail = match (&tx.error, tx.failed_node.and_then(|i| tx.nodes.get(i))) {
        (Some(error), Some(node)) => format!("Instruction {} · {} · {}", node.path, node.label, error),
        (Some(error), None) => error.clone(),
        _ => format!("{} instructions executed", tx.nodes.iter().filter(|n| n.depth == 0).count()),
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    match tx.status {
                        Status::Ok => "✓  ",
                        Status::Failed => "✕  ",
                    },
                    Style::new().fg(banner_color).bold(),
                ),
                Span::styled(headline, Style::new().fg(banner_color).bold()),
            ]),
            Line::styled(format!("   {detail}"), muted()),
        ])
        .wrap(Wrap { trim: true }),
        banner_inner,
    );

    let tiles = [
        (tx.nodes.iter().filter(|n| n.depth == 0).count().to_string(), "Instructions"),
        (tx.accounts.len().to_string(), "Accounts"),
        (format!("{:.1}k", tx.cu as f64 / 1000.0), "Compute units"),
        (format!("{:.6}", tx.fee as f64 / 1e9), "Fee in SOL"),
    ];
    let tile_areas = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(rows[1]);
    for (area, (value, label)) in tile_areas.iter().zip(tiles) {
        let block = panel("", false);
        let inner = block.inner(*area);
        f.render_widget(block, *area);
        f.render_widget(
            Paragraph::new(vec![
                Line::styled(value, Style::new().fg(ACCENT).bold()),
                Line::styled(label, muted()),
            ]),
            inner,
        );
    }

    let selected = app.selected_node().unwrap_or(0);
    render_compute_chart(f, rows[2], tx, selected, !app.focus_nav);
}

fn execution(f: &mut Frame, area: Rect, app: &mut App) {
    if app.tx().is_none() {
        let _ = require_tx(f, area, app, "EXECUTION FLOW");
        return;
    }
    let chart_height = (app.tx().map_or(0, |t| t.nodes.len()) + 4).min(12) as u16;
    let rows = Layout::vertical([Constraint::Min(5), Constraint::Length(chart_height)]).split(area);

    let visible = app.visible_nodes();
    let searching = !app.search.is_empty();
    let matched: Vec<bool> = visible.iter().map(|&i| app.node_matches(i)).collect();
    let tx = app.tx.as_ref().unwrap();
    let prefixes = tree_prefixes(&tx.nodes);
    let selected_index = app.list.selected().unwrap_or(0);

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(row, &i)| {
            let node = &tx.nodes[i];
            let faded = searching && !matched[row];
            let collapsed = app.collapsed.contains(&i);
            let marker = match (node.children > 0, collapsed) {
                (true, true) => "▸ ",
                (true, false) => "▾ ",
                _ => "  ",
            };
            let (glyph, color) = match node.status {
                Status::Ok => ("✓", OK),
                Status::Failed => ("✕", BAD),
            };
            let is_failure = tx.failed_node == Some(i);
            let label_style = match (is_failure, node.depth) {
                (true, _) => Style::new().fg(BAD).bold(),
                (false, 0) => Style::new().fg(Color::White).bold(),
                _ => Style::new().fg(Color::Rgb(176, 180, 210)),
            };
            let mut spans: Vec<Span> = vec![
                Span::styled(format!("{:>5} ", node.path), muted()),
                Span::styled(prefixes[i].clone(), Style::new().fg(PANEL)),
                Span::styled(marker, muted()),
                Span::styled(glyph, Style::new().fg(color)),
                Span::raw(" "),
                Span::styled(node.label.clone(), label_style),
            ];
            if collapsed && node.children > 0 {
                spans.push(Span::styled(format!(" (+{})", node.children), muted()));
            }
            spans.push(Span::styled(
                format!("  {} CU", thousands(node.self_cu as u64)),
                Style::new().fg(if node.self_cu > 0 { Color::Rgb(96, 165, 250) } else { MUTED }),
            ));
            if is_failure {
                if let Some(error) = &node.error {
                    spans.push(Span::styled(format!("  ⚠ {error}"), Style::new().fg(BAD)));
                }
            }
            if faded {
                spans = spans
                    .into_iter()
                    .map(|span| Span::styled(span.content, Style::new().fg(PANEL)))
                    .collect();
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let block = panel("EXECUTION FLOW", !app.focus_nav);
    let inner = block.inner(rows[0]);
    f.render_widget(block, rows[0]);
    let mut state = app.list.clone();
    state.select(Some(selected_index.min(visible.len().saturating_sub(1))));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 38, 58))),
        inner,
        &mut state,
    );

    let selected_node = visible
        .get(selected_index.min(visible.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0);
    render_compute_chart(f, rows[1], tx, selected_node, false);
    clickable_rows(app, inner, visible.len());
}

fn detail_panel(f: &mut Frame, area: Rect, app: &App) {
    let Some(tx) = app.tx() else { return };
    let Some(index) = app.selected_node() else { return };
    let node = &tx.nodes[index];

    let block = panel("SELECTED INSTRUCTION", false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let share = node.cu as f64 / tx.cu.max(1) as f64;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{} ", node.path), Style::new().fg(ACCENT).bold()),
            Span::styled(node.label.clone(), Style::new().fg(Color::White).bold()),
            Span::raw("  "),
            chip(
                match node.status {
                    Status::Ok => "Success",
                    Status::Failed => "Failed",
                },
                status_color(node.status),
            ),
        ]),
        Line::styled(
            format!("depth {} · {} inner call(s)", node.depth, node.children),
            muted(),
        ),
        Line::raw(""),
        Line::styled("COMPUTE UNITS", muted()),
        Line::from(vec![
            Span::styled(format!("{} self", thousands(node.self_cu as u64)), Style::new().fg(ACCENT).bold()),
            Span::styled(
                format!(
                    "  ·  {} inclusive  ·  {:.1}% of tx",
                    thousands(node.cu as u64),
                    share * 100.0
                ),
                muted(),
            ),
        ]),
        Line::from(bar(
            (share * (inner.width.saturating_sub(2)) as f64) as usize,
            inner.width.saturating_sub(2) as usize,
            status_color(node.status),
        )),
        Line::raw(""),
        Line::styled("PROGRAM ID", muted()),
        Line::raw(node.program.clone()),
        Line::raw(""),
    ];

    if let Some(error) = &node.error {
        lines.push(Line::styled("ERROR", Style::new().fg(BAD)));
        lines.push(Line::styled(error.clone(), Style::new().fg(BAD).bold()));
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled(format!("ACCOUNTS ({})", node.accounts.len()), muted()));

    let room = inner.height as usize;
    for account in node.accounts.iter().take(room.saturating_sub(lines.len() + 4)) {
        lines.push(Line::from(vec![
            Span::styled("● ", Style::new().fg(if account.writable { ACCENT } else { MUTED })),
            Span::raw(format!("{:<16}", short(&account.key))),
            Span::styled(account.role(), muted()),
        ]));
    }

    if !node.data.is_empty() {
        lines.push(Line::raw(""));
        match crate::solana::decode_instruction(&node.program, &node.data) {
            Some((_, fields)) => {
                lines.push(Line::styled("DATA — parsed", muted()));
                lines.extend(fields.into_iter().map(|(key, value)| {
                    Line::from(vec![
                        Span::styled(format!("{key:<13}"), Style::new().fg(Color::Rgb(96, 165, 250))),
                        Span::raw(value),
                    ])
                }));
            }
            None => lines.push(Line::styled("DATA — no decoder", muted())),
        }
        lines.push(Line::styled("raw (base58)", muted()));
        lines.push(Line::raw(node.data.clone()));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn accounts(f: &mut Frame, area: Rect, app: &mut App) {
    let Some((tx, inner)) = require_tx(f, area, app, "ACCOUNTS") else { return };
    let rows = app.account_rows();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|&i| (i, &tx.accounts[i]))
        .map(|(i, account)| {
            let touches = tx
                .nodes
                .iter()
                .filter(|n| n.accounts.iter().any(|a| a.key == account.key))
                .map(|n| n.path.clone())
                .collect::<Vec<_>>()
                .join(" ");
            ListItem::new(Line::from(vec![
                Span::styled(format!("{i:>3}  "), muted()),
                Span::styled("● ", Style::new().fg(if account.writable { ACCENT } else { MUTED })),
                Span::raw(format!("{:<46}", account.key)),
                Span::styled(format!("{:<18}", account.role()), muted()),
                Span::styled(format!("ix {touches}"), Style::new().fg(Color::Rgb(96, 165, 250))),
            ]))
        })
        .collect();
    let keys: Vec<String> = rows.iter().map(|&i| tx.accounts[i].key.clone()).collect();
    for (row, key) in keys.into_iter().take(inner.height as usize).enumerate() {
        app.push_hit(
            Rect { x: inner.x, y: inner.y + row as u16, width: inner.width, height: 1 },
            Click::Inspect(key),
        );
    }
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 38, 58))),
        inner,
        &mut app.list,
    );
}

fn state_diff(f: &mut Frame, area: Rect, app: &mut App) {
    let Some((tx, inner)) = require_tx(f, area, app, "STATE DIFF — BALANCE CHANGES") else { return };
    if tx.balances.is_empty() {
        f.render_widget(wrapped("no balance changes recorded", muted()), inner);
        return;
    }
    let rows = app.balance_rows();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|&i| &tx.balances[i])
        .map(|change| {
            let delta = change.after - change.before;
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:<16}", short(&change.key))),
                Span::styled(format!("{:<16}", change.label), muted()),
                Span::raw(format!("{:>16.6} → {:>16.6}  ", change.before, change.after)),
                Span::styled(
                    format!("{delta:>+16.6}"),
                    Style::new().fg(if delta < 0.0 { BAD } else { OK }),
                ),
            ]))
        })
        .collect();
    clickable_rows(app, inner, items.len());
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 38, 58))),
        inner,
        &mut app.list,
    );
}

fn logs(f: &mut Frame, area: Rect, app: &mut App) {
    let rows = app.log_rows();
    let Some((tx, inner)) = require_tx(f, area, app, "PROGRAM LOGS") else { return };
    let items: Vec<ListItem> = rows
        .iter()
        .map(|&i| &tx.logs[i])
        .map(|line| {
            let style = if line.contains("failed") {
                Style::new().fg(BAD)
            } else if line.contains("invoke") {
                Style::new().fg(ACCENT)
            } else if line.contains("success") {
                Style::new().fg(OK)
            } else {
                muted()
            };
            ListItem::new(Line::styled(line.clone(), style))
        })
        .collect();
    clickable_rows(app, inner, items.len());
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 38, 58))),
        inner,
        &mut app.list,
    );
}

fn raw(f: &mut Frame, area: Rect, app: &mut App) {
    let block = panel("RAW RPC RESPONSE", !app.focus_nav);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let cache_key = match app.kind {
        crate::solana::Kind::Tx => app.tx().map(|tx| tx.signature.clone()),
        crate::solana::Kind::Account => app.account.as_ref().map(|a| format!("account-{}", a.key)),
    };
    let Some(cache_key) = cache_key else {
        f.render_widget(wrapped("nothing loaded — press o", muted()), inner);
        return;
    };
    let text = crate::solana::load(&app.cluster, &cache_key)
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| "cache entry unavailable".to_string());
    f.render_widget(
        Paragraph::new(text).style(muted()).scroll((app.scroll as u16, 0)),
        inner,
    );
}

fn clickable_rows(app: &mut App, area: Rect, count: usize) {
    for row in 0..count.min(area.height as usize) {
        app.push_hit(
            Rect { x: area.x, y: area.y + row as u16, width: area.width, height: 1 },
            Click::Row(row),
        );
    }
}

fn require_account<'a>(f: &mut Frame, area: Rect, app: &'a App, title: &str) -> Option<(&'a crate::solana::AccountInfo, Rect)> {
    let block = panel(title, !app.focus_nav);
    let inner = block.inner(area);
    f.render_widget(block, area);
    match &app.account {
        Some(account) => Some((account, inner)),
        None => {
            f.render_widget(
                wrapped("no account loaded — press o and paste an address", muted()),
                inner,
            );
            None
        }
    }
}

fn account_info(f: &mut Frame, area: Rect, app: &mut App) {
    let Some((account, inner)) = require_account(f, area, app, "ACCOUNT INFO") else { return };
    let decoded = crate::solana::decode_account(account);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(account.key.clone(), Style::new().fg(Color::White).bold()),
            Span::raw("  "),
            chip(if account.executable { "program" } else { "account" }, ACCENT),
            Span::raw("  "),
            chip(
                decoded.as_ref().map_or("unknown layout", |(name, _)| name),
                Color::Rgb(96, 165, 250),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("BALANCE   ", muted()),
            Span::styled(format!("{:.9} SOL", account.lamports as f64 / 1e9), Style::new().fg(ACCENT).bold()),
            Span::styled(format!("  ({} lamports)", thousands(account.lamports)), muted()),
        ]),
        Line::from(vec![
            Span::styled("OWNER     ", muted()),
            Span::raw(account.owner_name()),
            Span::styled(format!("  {}", account.owner), muted()),
        ]),
        Line::from(vec![
            Span::styled("SIZE      ", muted()),
            Span::raw(format!("{} bytes", thousands(account.data.len() as u64))),
            Span::styled("    RENT EPOCH  ", muted()),
            Span::raw(if account.rent_epoch == u64::MAX {
                "exempt".to_string()
            } else {
                account.rent_epoch.to_string()
            }),
        ]),
        Line::raw(""),
    ];

    match decoded {
        Some((name, fields)) => {
            lines.push(Line::styled(format!("PARSED — {name}"), muted()));
            lines.extend(fields.into_iter().map(|(key, value)| {
                Line::from(vec![
                    Span::styled(format!("{key:<18}"), Style::new().fg(Color::Rgb(96, 165, 250))),
                    Span::raw(value),
                ])
            }));
        }
        None => lines.push(Line::styled(
            "PARSED — no decoder for this owner, see the Data tab for raw bytes",
            muted(),
        )),
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn account_data(f: &mut Frame, area: Rect, app: &mut App) {
    let scroll = app.scroll;
    let Some((account, inner)) = require_account(f, area, app, "ACCOUNT DATA") else { return };
    if account.data.is_empty() {
        f.render_widget(wrapped("account holds no data", muted()), inner);
        return;
    }
    let decoded = crate::solana::decode_account(account);
    let parsed_height = decoded.as_ref().map_or(3, |(_, fields)| fields.len() as u16 + 3);
    let rows = Layout::vertical([Constraint::Length(parsed_height), Constraint::Min(3)]).split(inner);

    let mut parsed = vec![Line::styled(
        match &decoded {
            Some((name, _)) => format!("PARSED — {name}"),
            None => format!("PARSED — no decoder for owner {}", short(&account.owner)),
        },
        muted(),
    )];
    if let Some((_, fields)) = &decoded {
        parsed.extend(fields.iter().map(|(key, value)| {
            Line::from(vec![
                Span::styled(format!("{key:<18}"), Style::new().fg(Color::Rgb(96, 165, 250))),
                Span::raw(value.clone()),
            ])
        }));
    }
    f.render_widget(Paragraph::new(parsed), rows[0]);

    let dump = panel("RAW (hex)", false);
    let dump_inner = dump.inner(rows[1]);
    f.render_widget(dump, rows[1]);
    let lines: Vec<Line> = crate::solana::hex_dump(&account.data, scroll, dump_inner.height as usize)
        .into_iter()
        .map(|(offset, hex, ascii)| {
            Line::from(vec![
                Span::styled(format!("{offset}  "), muted()),
                Span::styled(format!("{hex:<50}"), Style::new().fg(Color::Rgb(96, 165, 250))),
                Span::styled(format!(" │{ascii}"), muted()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), dump_inner);
}

fn help(f: &mut Frame, area: Rect) {
    let width = 54.min(area.width);
    let height = 16.min(area.height);
    let popup = Rect {
        x: (area.width - width) / 2,
        y: (area.height - height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, popup);
    let block = panel("HELP", true);
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let keys = [
        ("↑ ↓ / j k", "move selection"),
        ("→ / ←", "expand / collapse CPI node"),
        ("Enter", "open entry · inspect account"),
        ("Esc", "leave panel / back to history"),
        ("Tab", "next screen for this entity"),
        ("/", "search current view"),
        ("o", "open a transaction signature"),
        ("r", "refetch, bypassing cache"),
        ("g / G", "jump to top / bottom"),
        ("? / Esc", "toggle help / cancel"),
        ("q", "quit"),
    ];
    f.render_widget(
        Paragraph::new(
            keys.iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(format!(" {k:<12}"), Style::new().fg(ACCENT).bold()),
                        Span::styled(*v, muted()),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
        inner,
    );
}
