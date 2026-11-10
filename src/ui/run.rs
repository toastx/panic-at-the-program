use std::time::Instant;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Paragraph, Wrap};

use super::widgets::*;

/// How a `panic test` run ended.
pub enum Outcome {
    /// Failed transactions found, newest last.
    Open { cluster: &'static str, keys: Vec<String> },
    /// The suite ran but left nothing to diagnose; lines say why.
    Nothing(Vec<String>),
    /// We could not run the suite at all.
    Error(String),
}

pub struct Step {
    pub text: String,
    pub notes: Vec<String>,
}

/// State of the `panic test` screen: what has happened so far and the suite's output.
pub struct Run {
    pub framework: String,
    pub verbose: bool,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub steps: Vec<Step>,
    pub logs: Vec<String>,
    /// Lines scrolled up from the bottom of the output; 0 follows the tail.
    pub scroll: usize,
    pub passed: usize,
    pub failed: usize,
    pub outcome: Option<Outcome>,
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Run {
    pub fn new(framework: String, verbose: bool) -> Self {
        Run {
            framework,
            verbose,
            started: Instant::now(),
            finished: None,
            steps: Vec::new(),
            logs: Vec::new(),
            scroll: 0,
            passed: 0,
            failed: 0,
            outcome: None,
        }
    }

    pub fn step(&mut self, text: String) {
        self.steps.push(Step {
            text,
            notes: Vec::new(),
        });
    }

    pub fn note(&mut self, text: String) {
        match self.steps.last_mut() {
            Some(step) => step.notes.push(text),
            None => self.step(text),
        }
    }

    pub fn log(&mut self, line: &str) {
        let line = clean(line);
        // ponytail: counts from mocha (✔, "1) name") and cargo test ("... ok"/"... FAILED")
        // line shapes; a stray "2) " in program output can over-count failures.
        let text = line.trim();
        if text.starts_with('✔') || text.starts_with('✓') || (text.starts_with("test ") && text.ends_with(" ok")) {
            self.passed += 1;
        } else if text.starts_with("test ") && text.ends_with(" FAILED") {
            self.failed += 1;
        } else if let Some(n) = text.split_once(") ").and_then(|(n, _)| n.parse::<usize>().ok()) {
            // mocha numbers failures as it goes, then repeats them in its summary.
            self.failed = self.failed.max(n);
        }
        self.logs.push(line);
        // Scrolled up: stay on the same lines while new ones arrive below.
        if self.scroll > 0 {
            self.scroll += 1;
        }
    }

    pub fn finish(&mut self, outcome: Outcome) {
        self.outcome = Some(outcome);
        self.finished = Some(Instant::now());
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.logs.len().saturating_sub(1);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    pub fn shows_logs(&self) -> bool {
        self.verbose || matches!(self.outcome, Some(Outcome::Nothing(_) | Outcome::Error(_)))
    }

    fn elapsed(&self) -> f64 {
        let end = self.finished.unwrap_or_else(Instant::now);
        (end - self.started).as_secs_f64()
    }

    fn status(&self) -> (&'static str, Color) {
        match &self.outcome {
            None => {
                let frame = (self.started.elapsed().as_millis() / 80) as usize % SPINNER.len();
                (SPINNER[frame], theme().accent)
            }
            Some(Outcome::Error(_)) => ("✕", theme().bad),
            Some(_) => ("✓", theme().ok),
        }
    }
}

/// Strips ANSI escapes and control characters, expands tabs: the terminal is ours now.
fn clean(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            // CSI: ESC [ params final-letter.
            '\x1b' => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

pub fn draw(f: &mut Frame, run: &Run) {
    f.render_widget(Block::new().style(base()), f.area());
    let result = result_lines(run);
    let step_rows = run.steps.iter().map(|s| 1 + s.notes.len()).sum::<usize>() as u16 + 3;
    let rows = Layout::vertical([
        Constraint::Length(1),
        if run.shows_logs() {
            Constraint::Length(step_rows)
        } else {
            Constraint::Min(step_rows)
        },
        Constraint::Min(if run.shows_logs() { 3 } else { 0 }),
        Constraint::Length(if result.is_empty() { 0 } else { result.len() as u16 + 2 }),
        Constraint::Length(1),
    ])
    .split(f.area());

    topbar(f, rows[0], run);
    steps(f, rows[1], run);
    if run.shows_logs() {
        output(f, rows[2], run);
    }
    if !result.is_empty() {
        let color = match &run.outcome {
            Some(Outcome::Open { .. } | Outcome::Error(_)) => theme().bad,
            _ => theme().panel,
        };
        let block = panel("RESULT", false).border_style(Style::new().fg(color));
        let inner = block.inner(rows[3]);
        f.render_widget(block, rows[3]);
        f.render_widget(Paragraph::new(result).wrap(Wrap { trim: true }), inner);
    }
    footer(f, rows[4], run);
}

fn topbar(f: &mut Frame, area: Rect, run: &Run) {
    let (glyph, color) = run.status();
    let mut spans = vec![
        Span::styled(" PANIC ", Style::new().fg(theme().accent).bold()),
        Span::styled("│", muted()),
        Span::raw(format!(" test · {} ", run.framework)),
        Span::styled("│ ", muted()),
        Span::styled(glyph, Style::new().fg(color).bold()),
        Span::raw(format!(" {:.1}s  ", run.elapsed())),
        chip(&format!("✓ {} passed", run.passed), theme().ok),
        Span::raw(" "),
        chip(&format!("✕ {} failed", run.failed), theme().bad),
        Span::raw("  "),
    ];
    // Pass/fail split of the tests seen so far, in whole cells.
    let total = run.passed + run.failed;
    if let Some(green) = (run.passed * 20 + total / 2).checked_div(total) {
        let width = 20;
        spans.push(Span::styled("█".repeat(green), Style::new().fg(theme().ok)));
        spans.push(Span::styled("█".repeat(width - green), Style::new().fg(theme().bad)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn steps(f: &mut Frame, area: Rect, run: &Run) {
    let block = panel("TEST RUN", run.outcome.is_none());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let last = run.steps.len().saturating_sub(1);
    let mut lines = Vec::new();
    for (i, step) in run.steps.iter().enumerate() {
        let (glyph, color) = if i == last { run.status() } else { ("✓", theme().ok) };
        lines.push(Line::from(vec![
            Span::styled(format!(" {glyph} "), Style::new().fg(color).bold()),
            Span::styled(step.text.clone(), Style::new().fg(theme().strong)),
        ]));
        lines.extend(
            step.notes
                .iter()
                .map(|note| Line::styled(format!("   └ {note}"), muted())),
        );
    }
    // Without the output panel, a ticker of the latest line shows it is alive.
    if run.outcome.is_none()
        && !run.shows_logs()
        && let Some(latest) = run.logs.iter().rev().find(|l| !l.trim().is_empty())
    {
        lines.push(Line::styled(
            format!("   › {}", latest.trim()),
            Style::new().fg(theme().dim),
        ));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn output(f: &mut Frame, area: Rect, run: &Run) {
    let title = match run.scroll {
        0 => "SUITE OUTPUT".to_string(),
        n => format!("SUITE OUTPUT ↑{n}"),
    };
    let block = panel(&title, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let height = inner.height as usize;
    let end = run.logs.len().saturating_sub(run.scroll);
    let start = end.saturating_sub(height);
    let lines: Vec<Line> = run.logs[start..end].iter().map(|l| Line::raw(l.as_str())).collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn result_lines(run: &Run) -> Vec<Line<'static>> {
    match &run.outcome {
        None => Vec::new(),
        Some(Outcome::Open { cluster, keys }) => vec![
            Line::styled(
                format!("✕ {} failed transaction(s) on {cluster}", keys.len()),
                Style::new().fg(theme().bad).bold(),
            ),
            Line::from(vec![
                Span::styled("press ", muted()),
                Span::styled("Enter", Style::new().fg(theme().accent).bold()),
                Span::styled(" to open the latest in panic", muted()),
            ]),
        ],
        Some(Outcome::Nothing(text)) => text
            .iter()
            .enumerate()
            .map(|(i, line)| match i {
                0 => Line::styled(line.clone(), Style::new().fg(theme().strong).bold()),
                _ => Line::styled(line.clone(), muted()),
            })
            .collect(),
        Some(Outcome::Error(err)) => vec![Line::styled(err.clone(), Style::new().fg(theme().bad).bold())],
    }
}

fn footer(f: &mut Frame, area: Rect, run: &Run) {
    let key = |k: &'static str| Span::styled(k, Style::new().fg(theme().accent));
    let mut spans = Vec::new();
    if matches!(run.outcome, Some(Outcome::Open { .. })) {
        spans.extend([Span::raw(" "), key("Enter"), Span::styled(" open panic  ", muted())]);
    }
    if run.shows_logs() {
        spans.extend([Span::raw(" "), key("↑↓ PgUp PgDn"), Span::styled(" scroll  ", muted())]);
        spans.extend([key("G"), Span::styled(" follow  ", muted())]);
    }
    spans.extend([
        Span::raw(" "),
        key("q"),
        Span::styled(if run.outcome.is_none() { " abort" } else { " quit" }, muted()),
    ]);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
