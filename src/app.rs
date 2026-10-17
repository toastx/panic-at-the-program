use std::collections::HashSet;

use ratatui::widgets::ListState;

use crate::solana::{self, Summary, Tx};

#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    History,
    Overview,
    Execution,
    Accounts,
    StateDiff,
    Logs,
    Raw,
}

impl Screen {
    pub const ALL: [Screen; 7] = [
        Screen::History,
        Screen::Overview,
        Screen::Execution,
        Screen::Accounts,
        Screen::StateDiff,
        Screen::Logs,
        Screen::Raw,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Screen::History => "History",
            Screen::Overview => "Overview",
            Screen::Execution => "Execution",
            Screen::Accounts => "Accounts",
            Screen::StateDiff => "State Diff",
            Screen::Logs => "Logs",
            Screen::Raw => "Raw Data",
        }
    }
}

pub enum Mode {
    Normal,
    Search(String),
    Open(String),
    Help,
}

pub struct App {
    pub cluster: String,
    pub tx: Option<Tx>,
    pub screen: Screen,
    pub nav_index: usize,
    pub focus_nav: bool,
    pub list: ListState,
    pub collapsed: HashSet<usize>,
    pub history: Vec<Summary>,
    pub mode: Mode,
    pub message: String,
    pub scroll: usize,
    pub quit: bool,
}

impl App {
    pub fn new(cluster: String) -> Self {
        App {
            cluster,
            tx: None,
            screen: Screen::History,
            nav_index: 0,
            focus_nav: false,
            list: ListState::default().with_selected(Some(0)),
            collapsed: HashSet::new(),
            history: solana::summaries(),
            mode: Mode::Normal,
            message: String::new(),
            scroll: 0,
            quit: false,
        }
    }

    pub fn open(&mut self, signature: &str) {
        match solana::fetch(&self.cluster, signature) {
            Ok(value) => {
                self.tx = Some(solana::parse(signature, &self.cluster, &value));
                self.collapsed.clear();
                self.history = solana::summaries();
                self.message.clear();
                self.goto(Screen::Overview);
            }
            Err(err) => self.message = err.to_string(),
        }
    }

    pub fn goto(&mut self, screen: Screen) {
        self.screen = screen;
        self.nav_index = Screen::ALL.iter().position(|s| *s == screen).unwrap_or(0);
        self.list.select(Some(0));
        self.scroll = 0;
    }

    pub fn visible_nodes(&self) -> Vec<usize> {
        let Some(tx) = &self.tx else { return Vec::new() };
        let mut visible = Vec::new();
        let mut hidden_below: Option<usize> = None;
        for (i, node) in tx.nodes.iter().enumerate() {
            match hidden_below {
                Some(depth) if node.depth > depth => continue,
                _ => hidden_below = None,
            }
            visible.push(i);
            if self.collapsed.contains(&i) {
                hidden_below = Some(node.depth);
            }
        }
        visible
    }

    pub fn selected_node(&self) -> Option<usize> {
        let visible = self.visible_nodes();
        visible.get(self.list.selected().unwrap_or(0).min(visible.len().saturating_sub(1))).copied()
    }

    pub fn tx(&self) -> Option<&Tx> {
        self.tx.as_ref()
    }

    pub fn rows(&self) -> usize {
        match self.screen {
            Screen::History => self.history.len(),
            Screen::Execution | Screen::Overview => self.visible_nodes().len(),
            Screen::Accounts => self.tx().map_or(0, |t| t.accounts.len()),
            Screen::StateDiff => self.tx().map_or(0, |t| t.balances.len()),
            Screen::Logs => self.tx().map_or(0, |t| t.logs.len()),
            Screen::Raw => 0,
        }
    }
}
