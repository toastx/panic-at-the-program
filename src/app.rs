use std::collections::HashSet;

use ratatui::layout::Rect;
use ratatui::widgets::ListState;

use crate::solana::{self, AccountInfo, Summary, Tx};

pub const CLUSTERS: [&str; 4] = ["mainnet", "devnet", "testnet", "localnet"];

#[derive(Clone, PartialEq)]
pub enum Click {
    Workspace(Screen),
    Cluster(&'static str),
    Explorer,
    Row(usize),
    Inspect(String),
}

pub struct Hit {
    pub area: Rect,
    pub click: Click,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    History,
    Overview,
    Execution,
    Accounts,
    StateDiff,
    Logs,
    Raw,
    Account,
}

impl Screen {
    pub const ALL: [Screen; 8] = [
        Screen::History,
        Screen::Overview,
        Screen::Execution,
        Screen::Accounts,
        Screen::StateDiff,
        Screen::Logs,
        Screen::Raw,
        Screen::Account,
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
            Screen::Account => "Account Info",
        }
    }
}

pub enum Mode {
    Normal,
    Search,
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
    pub search: String,
    pub message: String,
    pub scroll: usize,
    pub quit: bool,
    pub account: Option<AccountInfo>,
    pub hits: Vec<Hit>,
}

impl App {
    pub fn new(cluster: String) -> Self {
        App {
            history: solana::summaries(&cluster),
            cluster,
            tx: None,
            screen: Screen::History,
            nav_index: 0,
            focus_nav: false,
            list: ListState::default().with_selected(Some(0)),
            collapsed: HashSet::new(),
            mode: Mode::Normal,
            search: String::new(),
            message: String::new(),
            scroll: 0,
            quit: false,
            account: None,
            hits: Vec::new(),
        }
    }

    pub fn open(&mut self, input: &str) {
        if solana::is_signature(input) {
            self.open_transaction(input);
        } else {
            self.open_account(input);
        }
    }

    pub fn open_transaction(&mut self, signature: &str) {
        match solana::fetch(&self.cluster, signature) {
            Ok(value) => {
                self.tx = Some(solana::parse(signature, &self.cluster, &value));
                self.collapsed.clear();
                self.refresh_history();
                self.message.clear();
                self.goto(Screen::Overview);
            }
            Err(err) => self.message = format!("{signature}: {err}"),
        }
    }

    pub fn open_account(&mut self, pubkey: &str) {
        match solana::fetch_account(&self.cluster, pubkey) {
            Ok(value) => {
                self.account = Some(solana::parse_account(pubkey, &value));
                self.message.clear();
                self.goto(Screen::Account);
            }
            Err(err) => self.message = format!("{pubkey}: {err}"),
        }
    }

    pub fn refresh_history(&mut self) {
        self.history = solana::summaries(&self.cluster);
    }

    pub fn set_cluster(&mut self, cluster: &str) {
        if self.cluster == cluster {
            return;
        }
        self.cluster = cluster.to_string();
        self.message.clear();
        self.refresh_history();
        if let Some(signature) = self.tx().map(|tx| tx.signature.clone()) {
            self.open_transaction(&signature);
        }
        if let Some(key) = self.account.as_ref().map(|a| a.key.clone()) {
            self.open_account(&key);
        }
    }

    pub fn explorer_target(&self) -> Option<(String, String)> {
        match self.screen {
            Screen::Account => self
                .account
                .as_ref()
                .map(|a| ("address".to_string(), a.key.clone())),
            _ => self.tx().map(|tx| ("tx".to_string(), tx.signature.clone())),
        }
    }

    pub fn open_in_explorer(&mut self) {
        let Some((kind, key)) = self.explorer_target() else {
            self.message = "nothing loaded to open in explorer".to_string();
            return;
        };
        let url = solana::explorer_url(&self.cluster, &kind, &key);
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        match std::process::Command::new(opener).arg(&url).spawn() {
            Ok(_) => self.message = format!("opened {url}"),
            Err(err) => self.message = format!("{err}: {url}"),
        }
    }

    pub fn click(&mut self, column: u16, row: u16) {
        let point = (column, row);
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|hit| {
                let a = hit.area;
                point.0 >= a.x && point.0 < a.x + a.width && point.1 >= a.y && point.1 < a.y + a.height
            })
            .map(|hit| hit.click.clone());
        match hit {
            Some(Click::Workspace(screen)) => {
                self.goto(screen);
                self.focus_nav = false;
            }
            Some(Click::Cluster(cluster)) => self.set_cluster(cluster),
            Some(Click::Explorer) => self.open_in_explorer(),
            Some(Click::Row(index)) => {
                self.focus_nav = false;
                self.list.select(Some(index));
                if self.screen == Screen::History {
                    if let Some(signature) = self
                        .history_rows()
                        .get(index)
                        .and_then(|&i| self.history.get(i))
                        .map(|s| s.signature.clone())
                    {
                        self.open_transaction(&signature);
                    }
                }
            }
            Some(Click::Inspect(key)) => self.open_account(&key),
            None => {}
        }
    }

    pub fn push_hit(&mut self, area: Rect, click: Click) {
        self.hits.push(Hit { area, click });
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

    pub fn matches(&self, text: &str) -> bool {
        self.search.is_empty() || text.to_lowercase().contains(&self.search.to_lowercase())
    }

    pub fn history_rows(&self) -> Vec<usize> {
        self.history
            .iter()
            .enumerate()
            .filter(|(_, s)| self.matches(&s.signature))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn account_rows(&self) -> Vec<usize> {
        self.tx().map_or(Vec::new(), |tx| {
            tx.accounts
                .iter()
                .enumerate()
                .filter(|(_, a)| self.matches(&a.key) || self.matches(a.role()))
                .map(|(i, _)| i)
                .collect()
        })
    }

    pub fn balance_rows(&self) -> Vec<usize> {
        self.tx().map_or(Vec::new(), |tx| {
            tx.balances
                .iter()
                .enumerate()
                .filter(|(_, b)| self.matches(&b.key) || self.matches(&b.label))
                .map(|(i, _)| i)
                .collect()
        })
    }

    pub fn log_rows(&self) -> Vec<usize> {
        self.tx().map_or(Vec::new(), |tx| {
            tx.logs
                .iter()
                .enumerate()
                .filter(|(_, line)| self.matches(line))
                .map(|(i, _)| i)
                .collect()
        })
    }

    pub fn node_matches(&self, index: usize) -> bool {
        self.tx()
            .map(|tx| &tx.nodes[index])
            .is_some_and(|n| self.matches(&n.label) || self.matches(&n.program) || self.matches(&n.path))
    }

    pub fn rows(&self) -> usize {
        match self.screen {
            Screen::History => self.history_rows().len(),
            Screen::Execution | Screen::Overview => self.visible_nodes().len(),
            Screen::Accounts => self.account_rows().len(),
            Screen::StateDiff => self.balance_rows().len(),
            Screen::Logs => self.log_rows().len(),
            Screen::Raw | Screen::Account => 0,
        }
    }

    pub fn match_count(&self) -> usize {
        match self.screen {
            Screen::Execution | Screen::Overview => {
                self.visible_nodes().iter().filter(|&&i| self.node_matches(i)).count()
            }
            _ => self.rows(),
        }
    }

    pub fn jump_to_match(&mut self) {
        if self.search.is_empty() {
            return;
        }
        let visible = self.visible_nodes();
        let start = self.list.selected().unwrap_or(0);
        let hit = (1..=visible.len())
            .map(|offset| (start + offset) % visible.len().max(1))
            .find(|&i| visible.get(i).is_some_and(|&n| self.node_matches(n)));
        if let Some(i) = hit {
            self.list.select(Some(i));
        }
    }
}
