use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap};

use crate::app::{App, Click, Mode, Screen};
use crate::solana::{Status, Tx};
use crate::ui::*;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)]).split(f.area());
    render_topbar(f, rows[0], app);

    let wide = rows[1].width >= 108;
    let detail = wide && app.screen == Screen::Execution && app.tx.is_some();
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
        Screen::Invocation => invocation(f, body, app),
        Screen::Accounts => accounts(f, body, app),
        Screen::Flow => flow(f, body, app),
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
                match (app.focus_nav, app.screen) {
                    (true, _) => " open  ",
                    (_, Screen::Overview | Screen::Info) => " open section  ",
                    (_, Screen::Execution) => " inspect instruction  ",
                    (_, Screen::Accounts | Screen::Invocation) => " inspect account  ",
                    _ => " select  ",
                },
                muted(),
            ),
            Span::styled("Esc/b", Style::new().fg(ACCENT)),
            Span::styled(" back  ", muted()),
            Span::styled("[ ]", Style::new().fg(ACCENT)),
            Span::styled(" network  ", muted()),
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

    let path = tx.failure_path();
    let mut movers: Vec<&crate::solana::BalanceChange> =
        tx.balances.iter().filter(|b| b.before != b.after).collect();
    movers.sort_by(|a, b| {
        (b.after - b.before)
            .abs()
            .total_cmp(&(a.after - a.before).abs())
    });
    movers.truncate(3);
    let path_height = if path.is_empty() {
        0
    } else {
        tx.failure.as_ref().map_or(0, |f| {
            f.checks.len() as u16
                + [&f.account, &f.expected, &f.actual, &f.source]
                    .iter()
                    .filter(|value| value.is_some())
                    .count() as u16
        }) + 5
    };
    let movers_height = if movers.is_empty() { 0 } else { movers.len() as u16 + 3 };

    let rows = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(path_height),
        Constraint::Length(movers_height),
        Constraint::Min(0),
    ])
    .split(area);

    let banner_color = status_color(tx.status);
    let banner = panel("RESULT", false).border_style(Style::new().fg(banner_color));
    let banner_inner = banner.inner(rows[0]);
    f.render_widget(banner, rows[0]);
    let headline = match tx.status {
        Status::Ok => "Transaction Succeeded".to_string(),
        Status::Failed => "Transaction Failed".to_string(),
    };
    let detail = match (&tx.failure, &tx.error, tx.failed_node.and_then(|i| tx.nodes.get(i))) {
        (Some(failure), _, Some(node)) => {
            format!("Instruction {} · {} · {}", node.path, node_label(node), failure.headline)
        }
        (_, Some(error), Some(node)) => format!("Instruction {} · {} · {}", node.path, node_label(node), error),
        (_, Some(error), None) => error.clone(),
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

    if path_height > 0 {
        let block = panel("DIAGNOSIS", false)
            .border_style(Style::new().fg(BAD))
            .padding(Padding::horizontal(1));
        let inner = block.inner(rows[2]);
        f.render_widget(block, rows[2]);

        let mut lines = Vec::new();
        if let Some(failure) = &tx.failure {
            lines.push(Line::from(vec![
                Span::styled("✕  ", Style::new().fg(BAD).bold()),
                Span::styled(failure.headline.clone(), Style::new().fg(BAD).bold()),
            ]));
            if !failure.meaning.is_empty() {
                lines.push(Line::styled(format!("   {}", failure.meaning), muted()));
            }
        }
        if let Some(&failed) = path.last() {
            let node = &tx.nodes[failed];
            lines.push(Line::from(vec![
                Span::styled("   instruction ", muted()),
                Span::styled(node.path.clone(), Style::new().fg(Color::White).bold()),
                Span::styled("  ", muted()),
                Span::styled(call_path(tx, failed).join("  →  "), Style::new().fg(Color::Rgb(233, 164, 170))),
            ]));
        }
        if let Some(failure) = &tx.failure {
            for (label, value) in [
                ("account", failure.account.as_ref()),
                ("expected", failure.expected.as_ref()),
                ("actual", failure.actual.as_ref()),
                ("source", failure.source.as_ref()),
            ] {
                let Some(value) = value else { continue };
                lines.push(Line::from(vec![
                    Span::styled(format!("   {label:<9}"), muted()),
                    Span::styled(
                        value.clone(),
                        match label {
                            "expected" => Style::new().fg(OK),
                            "actual" => Style::new().fg(BAD),
                            _ => Style::new().fg(Color::Rgb(176, 180, 210)),
                        },
                    ),
                ]));
            }
            lines.extend(failure.checks.iter().map(|check| {
                Line::from(vec![
                    Span::styled("   → ", Style::new().fg(ACCENT)),
                    Span::styled(check.clone(), Style::new().fg(Color::Rgb(176, 180, 210))),
                ])
            }));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    if movers_height > 0 {
        let block = panel("BALANCE CHANGES", false).padding(Padding::horizontal(1));
        let inner = block.inner(rows[3]);
        f.render_widget(block, rows[3]);
        let mut lines = vec![Line::styled(
            format!(
                "{:<14}{:<10}{:>16}{:>16}{:>16}",
                "ACCOUNT", "ASSET", "PRE", "POST", "CHANGE"
            ),
            muted(),
        )];
        lines.extend(movers.iter().map(|change| {
            let delta = change.after - change.before;
            Line::from(vec![
                Span::raw(format!("{:<14}", short(&change.key))),
                Span::styled(format!("{:<10}", change.label), muted()),
                Span::styled(format!("{:>16.6}", change.before), muted()),
                Span::raw(format!("{:>16.6}", change.after)),
                Span::styled(
                    format!("{delta:>+16.6}"),
                    Style::new().fg(if delta < 0.0 { BAD } else { OK }).bold(),
                ),
            ])
        }));
        f.render_widget(Paragraph::new(lines), inner);
    }

    render_sections(f, rows[4], app);
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
    let path = app.failure_path();
    let selected_index = app.selected_row();
    let selected_node = visible
        .get(selected_index.min(visible.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0);
    let tracing = path.contains(&selected_node);
    let tx = app.tx.as_ref().unwrap();
    let prefixes = tree_prefixes(&tx.nodes);

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(row, &i)| {
            let node = &tx.nodes[i];
            let faded = searching && !matched[row];
            let collapsed = app.collapsed.contains(&i);
            let is_failure = tx.failed_node == Some(i);
            let on_path = path.contains(&i);
            let marker = match (node.children > 0, collapsed) {
                (true, true) => "▸ ",
                (true, false) => "▾ ",
                _ => "  ",
            };
            let (glyph, glyph_color) = match (is_failure, on_path, node.status) {
                (true, _, _) => ("✕", BAD),
                (_, true, _) => ("↯", BAD),
                (_, _, Status::Ok) => ("✓", OK),
                _ => ("✕", BAD),
            };
            let connector_style = if on_path {
                Style::new().fg(if tracing { BAD } else { Color::Rgb(128, 72, 78) })
            } else {
                Style::new().fg(PANEL)
            };
            let label_style = match (is_failure, on_path, node.depth) {
                (true, _, _) => Style::new().fg(BAD).bold(),
                (_, true, _) => Style::new().fg(Color::Rgb(233, 164, 170)).bold(),
                (_, _, 0) => Style::new().fg(Color::White).bold(),
                _ => Style::new().fg(Color::Rgb(176, 180, 210)),
            };
            let mut spans: Vec<Span> = vec![
                Span::styled(format!("{:>5} ", node.path), muted()),
                Span::styled(prefixes[i].clone(), connector_style),
                Span::styled(marker, muted()),
                Span::styled(glyph, Style::new().fg(glyph_color)),
                Span::raw(" "),
                Span::styled(node_label(node), label_style),
            ];
            if collapsed && node.children > 0 {
                spans.push(Span::styled(format!(" (+{})", node.children), muted()));
            }
            spans.push(Span::styled(
                format!("  {} CU", thousands(node.self_cu as u64)),
                Style::new().fg(if node.self_cu > 0 { Color::Rgb(96, 165, 250) } else { MUTED }),
            ));
            if is_failure {
                let message = tx
                    .failure
                    .as_ref()
                    .filter(|failure| failure.node == Some(i))
                    .map(|failure| failure.headline.clone())
                    .or_else(|| node.error.clone());
                if let Some(message) = message {
                    spans.push(Span::styled(format!("  ⚠ {message}"), Style::new().fg(BAD).bold()));
                }
            } else if on_path {
                spans.push(Span::styled("  propagated", Style::new().fg(Color::Rgb(128, 72, 78))));
            }
            if faded || (tracing && !on_path) {
                spans = spans
                    .into_iter()
                    .map(|span| Span::styled(span.content, Style::new().fg(PANEL)))
                    .collect();
            }
            let item = ListItem::new(Line::from(spans));
            if is_failure {
                item.style(Style::new().bg(Color::Rgb(58, 26, 32)))
            } else {
                item
            }
        })
        .collect();

    let block = panel("EXECUTION FLOW", !app.focus_nav);
    let inner = block.inner(rows[0]);
    f.render_widget(block, rows[0]);
    let mut state = app.list.clone();
    state.select(Some(selected_index.min(visible.len().saturating_sub(1))));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(48, 48, 72))),
        inner,
        &mut state,
    );

    render_compute_chart(f, rows[1], tx, selected_node, false);
    clickable_rows(app, inner, visible.len());
}

fn invocation(f: &mut Frame, area: Rect, app: &mut App) {
    let Some(index) = app.node_index.filter(|i| app.tx().is_some_and(|tx| *i < tx.nodes.len())) else {
        let _ = require_tx(f, area, app, "INSTRUCTION");
        f.render_widget(
            wrapped("  select an invocation in the Execution tree and press Enter", muted()),
            area,
        );
        return;
    };
    let path = app.failure_path();
    let selected_row = app.selected_row();
    let tx = app.tx.as_ref().unwrap();
    let node = &tx.nodes[index];
    let failed = node.status == Status::Failed;

    let title = format!("INSTRUCTION {}", node.path);
    let block = panel(&title, !app.focus_nav)
        .border_style(Style::new().fg(if failed { BAD } else { PANEL }))
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let error_height = if failed {
        3 + tx.failure.as_ref().filter(|f| f.node == Some(index)).map_or(0, |f| f.checks.len() as u16)
    } else {
        0
    };
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(5),
        Constraint::Length(error_height),
        Constraint::Min(4),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(node_label(node), Style::new().fg(Color::White).bold()),
            Span::styled(format!("   {} inner call(s)   depth {}", node.children, node.depth), muted()),
            Span::raw("   "),
            chip(if failed { "✕ failed" } else { "✓ ok" }, status_color(node.status)),
        ])),
        rows[0],
    );

    let mut facts = vec![
        Line::from(vec![
            Span::styled("Call path   ", muted()),
            Span::styled(
                call_path(tx, index).join("  →  "),
                Style::new().fg(if path.contains(&index) { BAD } else { Color::Rgb(176, 180, 210) }),
            ),
        ]),
        Line::from(vec![
            Span::styled("Compute     ", muted()),
            Span::styled(format!("{:>9}", thousands(node.self_cu as u64)), Style::new().fg(ACCENT).bold()),
            Span::styled(" self", muted()),
            Span::styled(format!("{:>12}", thousands(node.cu as u64)), Style::new().fg(Color::Rgb(176, 180, 210))),
            Span::styled(" inclusive", muted()),
            Span::styled(
                format!("   {:.1}% of transaction", node.cu as f64 / tx.cu.max(1) as f64 * 100.0),
                muted(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Program     ", muted()),
            Span::raw(node.program.clone()),
        ]),
    ];
    let decoded = crate::idl::idl(&tx.cluster, &node.program)
        .and_then(|idl| crate::idl::instruction(&idl, &crate::solana::bs58_decode(&node.data)));
    let findings = crate::validate::check(&tx.cluster, tx, index);
    if let Some(decoded) = &decoded {
        facts.push(Line::from(vec![
            Span::styled("Data        ", muted()),
            Span::styled(decoded.name.clone(), Style::new().fg(Color::Rgb(96, 165, 250)).bold()),
            Span::styled(
                decoded
                    .args
                    .iter()
                    .map(|(k, v)| format!("   {k} {}", short(v)))
                    .collect::<String>(),
                Style::new().fg(Color::Rgb(176, 180, 210)),
            ),
        ]));
    } else {
    match crate::solana::decode_instruction(&node.program, &node.data) {
        Some((name, fields)) => facts.push(Line::from(vec![
            Span::styled("Data        ", muted()),
            Span::styled(name, Style::new().fg(Color::Rgb(96, 165, 250)).bold()),
            Span::styled(
                fields.iter().skip(1).map(|(k, v)| format!("   {k} {v}")).collect::<String>(),
                Style::new().fg(Color::Rgb(176, 180, 210)),
            ),
        ])),
        None if !node.data.is_empty() => facts.push(Line::from(vec![
            Span::styled("Data        ", muted()),
            Span::styled(format!("{}  (base58, no decoder)", short(&node.data)), muted()),
        ])),
        None => {}
    }
    }
    f.render_widget(Paragraph::new(facts), rows[1]);

    if failed {
        let diagnosed = tx
            .failure
            .as_ref()
            .filter(|failure| failure.node == Some(index));
        let headline = match diagnosed {
            Some(failure) => failure.headline.clone(),
            None => node.error.clone().or_else(|| tx.error.clone()).unwrap_or_default(),
        };
        let mut lines = vec![Line::from(vec![
            Span::styled("⚠  ", Style::new().fg(BAD).bold()),
            Span::styled(headline, Style::new().fg(BAD).bold()),
        ])];
        if let Some(failure) = diagnosed {
            if !failure.meaning.is_empty() {
                lines.push(Line::styled(format!("   {}", failure.meaning), muted()));
            }
            lines.extend(failure.checks.iter().map(|check| {
                Line::from(vec![
                    Span::styled("   → ", Style::new().fg(ACCENT)),
                    Span::styled(check.clone(), Style::new().fg(Color::Rgb(176, 180, 210))),
                ])
            }));
        }
        f.render_widget(Paragraph::new(lines), rows[2]);
    }

    if !findings.is_empty() {
        let lines: Vec<Line> = findings
            .iter()
            .take(rows[3].height.saturating_sub(1) as usize)
            .map(|finding| {
                let (glyph, color) = match finding.level {
                    crate::validate::Level::Error => ("✕", BAD),
                    crate::validate::Level::Warn => ("⚠", Color::Rgb(234, 179, 8)),
                };
                let mut spans = vec![
                    Span::styled(format!("{glyph} "), Style::new().fg(color)),
                    Span::styled(
                        format!("{:<20.19} ", if finding.role.is_empty() { short(&finding.key) } else { finding.role.clone() }),
                        Style::new().fg(Color::Rgb(96, 165, 250)),
                    ),
                    Span::styled(finding.message.clone(), Style::new().fg(Color::Rgb(176, 180, 210))),
                ];
                if let (Some(expected), Some(actual)) = (&finding.expected, &finding.actual) {
                    spans.push(Span::styled(format!("  expected {}", short(expected)), Style::new().fg(OK)));
                    spans.push(Span::styled(format!("  got {}", short(actual)), Style::new().fg(BAD)));
                }
                Line::from(spans)
            })
            .collect();
        let block = panel("VALIDATION", false).padding(Padding::horizontal(1));
        let inner = block.inner(rows[3]);
        f.render_widget(block, rows[3]);
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let split = rows[3].width >= 96;
    let panes = if split {
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(rows[3])
    } else {
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[3])
    };

    let accounts_title = format!("ACCOUNTS ({})", node.accounts.len());
    let accounts_block = panel(&accounts_title, false).padding(Padding::horizontal(1));
    let accounts_inner = accounts_block.inner(panes[0]);
    f.render_widget(accounts_block, panes[0]);
    let roles = decoded.map(|d| d.accounts).unwrap_or_default();
    let items: Vec<ListItem> = node
        .accounts
        .iter()
        .enumerate()
        .map(|(i, account)| {
            let flagged = findings.iter().find(|finding| finding.index == i);
            let (glyph, glyph_style) = match flagged.map(|f| f.level) {
                Some(crate::validate::Level::Error) => ("✕ ", Style::new().fg(BAD)),
                Some(crate::validate::Level::Warn) => ("⚠ ", Style::new().fg(Color::Rgb(234, 179, 8))),
                None => ("● ", Style::new().fg(if account.writable { ACCENT } else { MUTED })),
            };
            ListItem::new(Line::from(vec![
                Span::styled(glyph, glyph_style),
                Span::styled(
                    format!("{:<24.23} ", roles.get(i).cloned().unwrap_or_default()),
                    Style::new().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw(format!("{:<16}", short(&account.key))),
                Span::styled(account.role(), muted()),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(selected_row.min(items.len().saturating_sub(1))));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 38, 58)).bold()),
        accounts_inner,
        &mut state,
    );

    let logs_block = panel("LOGS", false).padding(Padding::horizontal(1));
    let logs_inner = logs_block.inner(panes[1]);
    f.render_widget(logs_block, panes[1]);
    let log_lines: Vec<Line> = tx.logs[node.logs.clone()]
        .iter()
        .rev()
        .take(logs_inner.height as usize)
        .rev()
        .map(|line| {
            let style = if line.contains("failed:") || line.contains("Error") {
                Style::new().fg(BAD)
            } else if line.starts_with("Program log:") {
                Style::new().fg(Color::Rgb(176, 180, 210))
            } else {
                muted()
            };
            Line::styled(compact_keys(line), style)
        })
        .collect();
    f.render_widget(Paragraph::new(log_lines), logs_inner);
    clickable_rows(app, accounts_inner, node.accounts.len());
}

fn render_sections(f: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 5 {
        return;
    }
    let sections = app.sections();
    if sections.is_empty() {
        return;
    }
    let selected = app.selected_row().min(sections.len() - 1);
    let columns = match area.width {
        w if w >= 84 => 3,
        w if w >= 56 => 2,
        _ => 1,
    }
    .min(sections.len());
    app.section_columns = columns;

    let card_height = 4u16;
    let rows = sections.len().div_ceil(columns);
    let visible_rows = ((area.height / card_height) as usize).clamp(1, rows);
    let selected_row = selected / columns;
    let first_row = selected_row.saturating_sub(visible_rows - 1).min(rows - visible_rows);
    let row_areas = Layout::vertical(vec![Constraint::Length(card_height); visible_rows])
        .split(Rect { height: visible_rows as u16 * card_height, ..area });

    let mut hits: Vec<(Rect, usize)> = Vec::new();
    for (offset, row_area) in row_areas.iter().enumerate() {
        let row = first_row + offset;
        let cells = Layout::horizontal(vec![Constraint::Ratio(1, columns as u32); columns]).split(*row_area);
        for (column, cell) in cells.iter().enumerate() {
            let index = row * columns + column;
            let Some((screen, headline, detail)) = sections.get(index) else { continue };
            let active = index == selected;
            let block = panel(screen.title(), active)
                .border_style(Style::new().fg(if active { ACCENT } else { PANEL }))
                .padding(Padding::horizontal(1));
            let inner = block.inner(*cell);
            f.render_widget(block, *cell);
            f.render_widget(
                Paragraph::new(vec![
                    Line::styled(
                        headline.clone(),
                        if active {
                            Style::new().fg(ACCENT).bold()
                        } else {
                            Style::new().fg(Color::White)
                        },
                    ),
                    Line::styled(detail.clone(), muted()),
                ]),
                inner,
            );
            hits.push((*cell, index));
        }
    }
    for (cell, index) in hits {
        app.push_hit(cell, Click::Row(index));
    }
}

fn flow(f: &mut Frame, area: Rect, app: &mut App) {
    let selected = app.selected_row();
    let Some((tx, inner)) = require_tx(f, area, app, "VALUE FLOW") else { return };
    let transfers = crate::solana::transfers(tx);
    if transfers.is_empty() {
        f.render_widget(
            wrapped(
                "no decodable transfers — this transaction moves value through programs without a known instruction layout",
                muted(),
            ),
            inner,
        );
        return;
    }

    let box_width = (inner.width.saturating_sub(16) / 3).clamp(12, 26);
    let middle_width = box_width + 4;
    let gap = inner
        .width
        .saturating_sub(box_width * 2 + middle_width)
        .saturating_div(2)
        .clamp(3, 14);
    let box_height = 4u16;
    let rows_per = box_height + 1;
    let capacity = (((inner.height + 1) / rows_per) as usize).max(1);
    let overflow = transfers.len() > capacity;
    let capacity = if overflow { capacity.saturating_sub(1).max(1) } else { capacity };
    let start = selected.saturating_sub(capacity.saturating_sub(1)).min(transfers.len() - 1);
    let mut y = inner.y;
    let mut hits: Vec<(Rect, usize)> = Vec::new();

    for (row, transfer) in transfers.iter().enumerate().skip(start).take(capacity) {
        if y + box_height > inner.y + inner.height {
            break;
        }
        let active = row == selected;
        let node = &tx.nodes[transfer.node];
        let failed = node.status == Status::Failed;
        let accent = if failed { BAD } else if active { ACCENT } else { PANEL };
        let text = if active { Style::new().fg(Color::White).bold() } else { Style::new().fg(Color::Rgb(176, 180, 210)) };

        let left = Rect { x: inner.x, y, width: box_width, height: box_height };
        let middle = Rect { x: left.x + box_width + gap, y, width: middle_width, height: box_height };
        let right = Rect { x: middle.x + middle_width + gap, y, width: box_width, height: box_height };

        let amount = format!("{:.6} {}", transfer.amount, transfer.unit);
        draw_box(f, left, &short(&transfer.from), &format!("-{amount}"), accent, text, BAD);
        draw_box(f, middle, &node_label(node), &format!("{}  ix {}", transfer.kind, node.path), accent, text, accent);
        if right.x + right.width <= inner.x + inner.width {
            draw_box(f, right, &short(&transfer.to), &format!("+{amount}"), accent, text, OK);
        }

        let arrow_y = y + box_height / 2;
        let label = amount.clone();
        for (start, end) in [(left.x + left.width, middle.x), (middle.x + middle.width, right.x)] {
            if end <= start || end > inner.x + inner.width {
                continue;
            }
            let width = end - start;
            let line = format!("{}▶", "─".repeat(width.saturating_sub(1) as usize));
            f.render_widget(
                Paragraph::new(Line::styled(line, Style::new().fg(accent))),
                Rect { x: start, y: arrow_y, width, height: 1 },
            );
        }
        let label_x = left.x + left.width + 1;
        if label_x + label.chars().count() as u16 <= middle.x {
            f.render_widget(
                Paragraph::new(Line::styled(label, if active { Style::new().fg(ACCENT).bold() } else { muted() })),
                Rect { x: label_x, y, width: middle.x - label_x, height: 1 },
            );
        }

        hits.push((Rect { x: inner.x, y, width: inner.width, height: box_height }, row));
        y += rows_per;
    }
    for (area, row) in hits {
        app.push_hit(area, Click::Row(row));
    }

    if overflow {
        f.render_widget(
            Paragraph::new(Line::styled(
                format!(
                    "  showing {}–{} of {} transfers",
                    start + 1,
                    (start + capacity).min(transfers.len()),
                    transfers.len()
                ),
                muted(),
            )),
            Rect { x: inner.x, y: inner.y + inner.height - 1, width: inner.width, height: 1 },
        );
    }
}

fn draw_box(
    f: &mut Frame,
    area: Rect,
    title: &str,
    detail: &str,
    border: Color,
    text: Style,
    detail_color: Color,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(border));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(vec![
            Line::styled(title.to_string(), text),
            Line::styled(detail.to_string(), Style::new().fg(detail_color)),
        ]),
        inner,
    );
}

fn compact_keys(line: &str) -> String {
    line.split(' ')
        .map(|token| if token.len() >= 32 { short(token) } else { token.to_string() })
        .collect::<Vec<_>>()
        .join(" ")
}

fn call_path(tx: &crate::solana::Tx, index: usize) -> Vec<String> {
    let mut chain = vec![index];
    let mut depth = tx.nodes[index].depth;
    for i in (0..index).rev() {
        if tx.nodes[i].depth < depth {
            chain.push(i);
            depth = tx.nodes[i].depth;
            if depth == 0 {
                break;
            }
        }
    }
    chain.reverse();
    chain.into_iter().map(|i| node_label(&tx.nodes[i])).collect()
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
        match (tx.failed_node, tx.failed_node.map(|i| i == index)) {
            (Some(failed), Some(false)) if app.failure_path().contains(&index) => Line::styled(
                format!("failure propagated from instruction {}", tx.nodes[failed].path),
                Style::new().fg(BAD),
            ),
            _ => Line::raw(""),
        },
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
    let mut state = app.list.clone();
    state.select(Some(app.selected_row()));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 38, 58))),
        inner,
        &mut state,
    );
}

fn state_diff(f: &mut Frame, area: Rect, app: &mut App) {
    let Some((tx, inner)) = require_tx(f, area, app, "STATE DIFF — BALANCE CHANGES") else { return };
    if tx.balances.is_empty() {
        f.render_widget(wrapped("no balance changes recorded", muted()), inner);
        return;
    }
    let rows = app.balance_rows();
    let header = Rect { height: 1, ..inner };
    let body = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    f.render_widget(
        Paragraph::new(Line::styled(
            format!(
                "{:<18}{:<14}{:>18}{:>18}{:>18}",
                "ACCOUNT", "ASSET", "PRE", "POST", "CHANGE"
            ),
            muted(),
        )),
        header,
    );
    let items: Vec<ListItem> = rows
        .iter()
        .map(|&i| &tx.balances[i])
        .map(|change| {
            let delta = change.after - change.before;
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:<18}", short(&change.key))),
                Span::styled(format!("{:<14}", change.label), muted()),
                Span::styled(format!("{:>18.6}", change.before), muted()),
                Span::raw(format!("{:>18.6}", change.after)),
                Span::styled(
                    format!("{delta:>+18.6}"),
                    Style::new().fg(if delta < 0.0 { BAD } else { OK }).bold(),
                ),
            ]))
        })
        .collect();
    clickable_rows(app, body, items.len());
    let mut state = app.list.clone();
    state.select(Some(app.selected_row()));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 38, 58))),
        body,
        &mut state,
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
    let mut state = app.list.clone();
    state.select(Some(app.selected_row()));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 38, 58))),
        inner,
        &mut state,
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

fn decode_account_fields(
    cluster: &str,
    account: &crate::solana::AccountInfo,
) -> Option<(String, Vec<(String, String)>)> {
    if let Some((layout, fields)) = crate::solana::decode_account(account) {
        return Some((layout.to_string(), fields));
    }
    let idl = crate::idl::idl(cluster, &account.owner)?;
    crate::idl::account(&idl, &account.data)
}

fn account_info(f: &mut Frame, area: Rect, app: &mut App) {
    let Some((account, inner)) = require_account(f, area, app, "ACCOUNT") else { return };
    let decoded = decode_account_fields(&app.cluster, account);

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
            "PARSED — no decoder for this owner, see the Data section for raw bytes",
            muted(),
        )),
    }
    let height = lines.len() as u16;
    let rows = Layout::vertical([Constraint::Length(height), Constraint::Min(0)]).split(inner);
    f.render_widget(Paragraph::new(lines), rows[0]);
    render_sections(f, rows[1], app);
}

fn account_data(f: &mut Frame, area: Rect, app: &mut App) {
    let scroll = app.scroll;
    let Some((account, inner)) = require_account(f, area, app, "ACCOUNT DATA") else { return };
    if account.data.is_empty() {
        f.render_widget(wrapped("account holds no data", muted()), inner);
        return;
    }
    let decoded = decode_account_fields(&app.cluster, account);
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
        ("Esc / b", "back: instruction → tree → entity → history"),
        ("] / [", "next / previous network"),
        ("c", "network dropdown"),
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
