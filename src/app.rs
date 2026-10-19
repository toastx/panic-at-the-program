use std::collections::HashSet;

use ratatui::layout::Rect;
use ratatui::widgets::ListState;

use crate::solana::{self, AccountInfo, Entry, Kind, Tx};

pub const CLUSTERS: [&str; 4] = ["mainnet", "devnet", "testnet", "localnet"];

#[derive(Clone, PartialEq)]
pub enum Click {
    Workspace(Screen),
    Entry(usize),
    Back,
    ClusterMenu,
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
    Overview,
    Execution,
    Invocation,
    Accounts,
    Flow,
    StateDiff,
    Logs,
    Info,
    Data,
    Raw,
}

impl Screen {
    pub const TX: [Screen; 7] = [
        Screen::Overview,
        Screen::Execution,
        Screen::Accounts,
        Screen::Flow,
        Screen::StateDiff,
        Screen::Logs,
        Screen::Raw,
    ];
    pub const ACCOUNT: [Screen; 3] = [Screen::Info, Screen::Data, Screen::Raw];

    pub fn title(&self) -> &'static str {
        match self {
            Screen::Overview => "Overview",
            Screen::Execution => "Execution",
            Screen::Invocation => "Instruction",
            Screen::Accounts => "Accounts",
            Screen::Flow => "Value Flow",
            Screen::StateDiff => "State Diff",
            Screen::Logs => "Logs",
            Screen::Info => "Info",
            Screen::Data => "Data",
            Screen::Raw => "Raw",
        }
    }

    pub fn for_kind(kind: Kind) -> &'static [Screen] {
        match kind {
            Kind::Tx => &Screen::TX,
            Kind::Account => &Screen::ACCOUNT,
        }
    }

    pub fn hub(kind: Kind) -> Screen {
        match kind {
            Kind::Tx => Screen::Overview,
            Kind::Account => Screen::Info,
        }
    }
}

pub enum Mode {
    Normal,
    Search,
    Open(String),
    Help,
}

pub struct Location {
    pub kind: Kind,
    pub key: String,
    pub screen: Screen,
}

pub struct App {
    pub cluster: String,
    pub tx: Option<Tx>,
    pub screen: Screen,
    pub kind: Kind,
    pub entries: Vec<Entry>,
    pub entry_index: usize,
    pub node_index: Option<usize>,
    pub section_columns: usize,
    pub stack: Vec<Location>,
    pub focus_nav: bool,
    pub list: ListState,
    pub collapsed: HashSet<usize>,

    pub mode: Mode,
    pub search: String,
    pub message: String,
    pub scroll: usize,
    pub quit: bool,
    pub account: Option<AccountInfo>,
    pub cluster_button: Rect,
    pub cluster_menu: bool,
    pub cluster_index: usize,
    pub hits: Vec<Hit>,
}

impl App {
    pub fn new(cluster: String) -> Self {
        App {
            entries: solana::entries(&cluster),
            cluster,
            tx: None,
            screen: Screen::Overview,
            kind: Kind::Tx,
            entry_index: 0,
            node_index: None,
            section_columns: 3,
            stack: Vec::new(),
            focus_nav: false,
            list: ListState::default(),
            collapsed: HashSet::new(),
            mode: Mode::Normal,
            search: String::new(),
            message: String::new(),
            scroll: 0,
            quit: false,
            account: None,
            cluster_button: Rect::new(0, 0, 0, 0),
            cluster_menu: false,
            cluster_index: 0,
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

    fn push_location(&mut self, next_key: &str) {
        let Some(key) = self.loaded_key() else { return };
        if key == next_key {
            return;
        }
        self.stack.push(Location { kind: self.kind, key, screen: self.screen });
    }

    pub fn drill_into_account(&mut self, pubkey: &str) {
        self.push_location(pubkey);
        self.load_account(pubkey);
    }

    fn pop_location(&mut self) -> bool {
        let Some(location) = self.stack.pop() else { return false };
        let in_memory = match location.kind {
            Kind::Tx => self.tx().is_some_and(|tx| tx.signature == location.key),
            Kind::Account => self.account.as_ref().is_some_and(|a| a.key == location.key),
        };
        if in_memory {
            self.kind = location.kind;
            self.message.clear();
        } else {
            match location.kind {
                Kind::Tx => self.load_transaction(&location.key),
                Kind::Account => self.load_account(&location.key),
            }
        }
        if self.message.is_empty() {
            self.goto(location.screen);
        }
        true
    }

    pub fn open_transaction(&mut self, signature: &str) {
        self.stack.clear();
        self.load_transaction(signature);
    }

    pub fn open_account(&mut self, pubkey: &str) {
        self.stack.clear();
        self.load_account(pubkey);
    }

    fn load_transaction(&mut self, signature: &str) {
        match solana::fetch(&self.cluster, signature) {
            Ok(value) => {
                self.tx = Some(solana::parse(signature, &self.cluster, &value));
                self.collapsed.clear();
                self.kind = Kind::Tx;
                self.focus_nav = false;
                self.node_index = self.tx().and_then(|tx| tx.failed_node);
                self.goto(Screen::Overview);
                self.focus_failure();
                self.refresh_history();
                self.message.clear();
            }
            Err(err) => self.message = format!("{signature}: {err}"),
        }
    }

    fn load_account(&mut self, pubkey: &str) {
        match solana::fetch_account(&self.cluster, pubkey) {
            Ok(value) => {
                self.account = Some(solana::parse_account(pubkey, &value));
                self.kind = Kind::Account;
                self.focus_nav = false;
                self.goto(Screen::Info);
                self.refresh_history();
                self.message.clear();
            }
            Err(err) => self.message = format!("{pubkey}: {err}"),
        }
    }

    pub fn refresh_history(&mut self) {
        self.entries = solana::entries(&self.cluster);
        if let Some(index) = self.loaded_key().and_then(|key| {
            self.entries.iter().position(|e| e.key == key)
        }) {
            self.entry_index = index;
        }
    }

    pub fn loaded_key(&self) -> Option<String> {
        match self.kind {
            Kind::Tx => self.tx().map(|tx| tx.signature.clone()),
            Kind::Account => self.account.as_ref().map(|a| a.key.clone()),
        }
    }

    pub fn step_cluster(&mut self, delta: isize) {
        let current = CLUSTERS.iter().position(|c| *c == self.cluster).unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(CLUSTERS.len() as isize) as usize;
        self.set_cluster(CLUSTERS[next]);
    }

    pub fn toggle_cluster_menu(&mut self) {
        self.cluster_menu = !self.cluster_menu;
        self.cluster_index = CLUSTERS.iter().position(|c| *c == self.cluster).unwrap_or(0);
    }

    pub fn set_cluster(&mut self, cluster: &str) {
        self.cluster_menu = false;
        if self.cluster == cluster {
            return;
        }
        self.cluster = cluster.to_string();
        self.message.clear();
        self.refresh_history();
        match self.kind {
            Kind::Tx => {
                if let Some(signature) = self.tx().map(|tx| tx.signature.clone()) {
                    self.load_transaction(&signature);
                }
            }
            Kind::Account => {
                if let Some(key) = self.account.as_ref().map(|a| a.key.clone()) {
                    self.load_account(&key);
                }
            }
        }
    }

    pub fn explorer_target(&self) -> Option<(String, String)> {
        match self.kind {
            Kind::Account => self.account.as_ref().map(|a| ("address".to_string(), a.key.clone())),
            Kind::Tx => self.tx().map(|tx| ("tx".to_string(), tx.signature.clone())),
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
                self.focus_nav = true;
            }
            Some(Click::Back) => self.back_to_entities(),
            Some(Click::ClusterMenu) => self.toggle_cluster_menu(),
            Some(Click::Cluster(cluster)) => self.set_cluster(cluster),
            Some(Click::Explorer) => self.open_in_explorer(),
            Some(Click::Row(index)) => {
                self.focus_nav = false;
                self.list.select(Some(index));
                if self.screen == Screen::hub(self.kind) {
                    self.open_section();
                }
            }
            Some(Click::Entry(index)) => {
                self.entry_index = index;
                self.open_entry(index);
            }
            Some(Click::Inspect(key)) => self.drill_into_account(&key),
            None => self.cluster_menu = false,
        }
    }

    pub fn push_hit(&mut self, area: Rect, click: Click) {
        self.hits.push(Hit { area, click });
    }

    pub fn goto(&mut self, screen: Screen) {
        let keep_node = screen == Screen::Execution && self.screen == Screen::Invocation;
        self.screen = screen;
        if !keep_node {
            self.list.select(None);
        }
        self.scroll = 0;
    }

    pub fn back(&mut self) -> bool {
        let hub = Screen::hub(self.kind);
        if self.screen == Screen::Invocation {
            self.goto(Screen::Execution);
            return true;
        }
        if self.screen != hub {
            self.goto(hub);
            return true;
        }
        self.pop_location()
    }

    pub fn sections(&self) -> Vec<(Screen, String, String)> {
        let hub = Screen::hub(self.kind);
        self.screens()
            .iter()
            .filter(|screen| **screen != hub)
            .map(|screen| {
                let (headline, detail) = self.section_summary(*screen);
                (*screen, headline, detail)
            })
            .collect()
    }

    fn section_summary(&self, screen: Screen) -> (String, String) {
        if self.kind == Kind::Account {
            let Some(account) = self.account.as_ref() else { return (String::new(), String::new()) };
            return match screen {
                Screen::Data => (
                    format!("{} B", account.data.len()),
                    match solana::decode_account(account) {
                        Some((layout, fields)) => format!("{layout}, {} fields", fields.len()),
                        None => "no decoder".to_string(),
                    },
                ),
                Screen::Raw => ("JSON".to_string(), "cached response".to_string()),
                _ => (String::new(), String::new()),
            };
        }
        let Some(tx) = self.tx() else { return (String::new(), String::new()) };
        match screen {
            Screen::Execution => (
                format!("{} calls", tx.nodes.len()),
                match tx.failed_node.map(|i| tx.nodes[i].path.clone()) {
                    Some(path) => format!("failed at {path}"),
                    None => format!("{} top level", tx.nodes.iter().filter(|n| n.depth == 0).count()),
                },
            ),
            Screen::Accounts => (
                format!("{}", tx.accounts.len()),
                format!(
                    "{} writable · {} signer",
                    tx.accounts.iter().filter(|a| a.writable).count(),
                    tx.accounts.iter().filter(|a| a.signer).count()
                ),
            ),
            Screen::Flow => {
                let transfers = solana::transfers(tx);
                match transfers.first() {
                    Some(first) => (
                        format!("{} transfers", transfers.len()),
                        format!("{} {}", first.kind, first.unit),
                    ),
                    None => ("none".to_string(), "no decodable transfers".to_string()),
                }
            }
            Screen::StateDiff => {
                let net: f64 = tx
                    .balances
                    .iter()
                    .filter(|b| b.label == "SOL")
                    .map(|b| b.after - b.before)
                    .sum();
                (
                    format!("{} changes", tx.balances.len()),
                    format!("net {net:+.6} SOL"),
                )
            }
            Screen::Logs => (
                format!("{} lines", tx.logs.len()),
                format!(
                    "{} program invocations",
                    tx.logs.iter().filter(|l| l.contains(" invoke [")).count()
                ),
            ),
            Screen::Raw => ("JSON".to_string(), "cached response".to_string()),
            _ => (String::new(), String::new()),
        }
    }

    pub fn open_section(&mut self) {
        let sections = self.sections();
        if let Some(screen) = sections.get(self.selected_row()).map(|(screen, _, _)| *screen) {
            self.goto(screen);
        }
    }

    pub fn back_to_entities(&mut self) {
        self.focus_nav = true;
        self.stack.clear();
    }

    pub fn screens(&self) -> &'static [Screen] {
        Screen::for_kind(self.kind)
    }

    pub fn screen_index(&self) -> usize {
        let screen = match self.screen {
            Screen::Invocation => Screen::Execution,
            other => other,
        };
        self.screens().iter().position(|s| *s == screen).unwrap_or(0)
    }

    pub fn cycle_screen(&mut self, delta: isize) {
        let screens = self.screens();
        let next = (self.screen_index() as isize + delta).rem_euclid(screens.len() as isize) as usize;
        self.goto(screens[next]);
    }

    pub fn open_entry(&mut self, index: usize) {
        let Some(entry) = self.entries.get(index) else { return };
        self.entry_index = index;
        let (kind, key) = (entry.kind, entry.key.clone());
        match kind {
            Kind::Tx => self.open_transaction(&key),
            Kind::Account => self.open_account(&key),
        }
    }

    pub fn parent_node(&self, index: usize) -> Option<usize> {
        let tx = self.tx()?;
        let depth = tx.nodes.get(index)?.depth;
        (0..index).rev().find(|&i| tx.nodes[i].depth < depth)
    }

    pub fn default_row(&self) -> usize {
        match (self.screen, self.tx()) {
            (Screen::Execution, Some(tx)) => tx
                .failed_node
                .and_then(|failed| self.visible_nodes().iter().position(|&i| i == failed))
                .unwrap_or(0),
            _ => 0,
        }
    }

    pub fn selected_row(&self) -> usize {
        self.list.selected().unwrap_or_else(|| self.default_row())
    }

    pub fn focus_failure(&mut self) {
        let Some(tx) = self.tx() else { return };
        let Some(failed) = tx.failed_node else { return };
        for ancestor in tx.failure_path() {
            self.collapsed.remove(&ancestor);
        }
        if self.screen == Screen::Execution {
            let row = self.visible_nodes().iter().position(|&i| i == failed);
            self.list.select(row);
        }
    }

    pub fn failure_path(&self) -> Vec<usize> {
        self.tx().map(Tx::failure_path).unwrap_or_default()
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

    pub fn selected_account(&self) -> Option<String> {
        let row = self.selected_row();
        match self.screen {
            Screen::Accounts => {
                let index = *self.account_rows().get(row)?;
                self.tx().map(|tx| tx.accounts[index].key.clone())
            }
            Screen::Invocation => {
                let node = self.node_index?;
                let tx = self.tx()?;
                tx.nodes[node].accounts.get(row).map(|a| a.key.clone())
            }
            _ => None,
        }
    }

    pub fn selected_transfer_node(&self) -> Option<usize> {
        let transfers = solana::transfers(self.tx()?);
        transfers.get(self.selected_row()).map(|t| t.node)
    }

    pub fn inspect_invocation(&mut self) {
        let Some(node) = self.selected_node() else { return };
        self.node_index = Some(node);
        self.goto(Screen::Invocation);
    }

    pub fn selected_node(&self) -> Option<usize> {
        let visible = self.visible_nodes();
        visible.get(self.selected_row().min(visible.len().saturating_sub(1))).copied()
    }

    pub fn tx(&self) -> Option<&Tx> {
        self.tx.as_ref()
    }

    pub fn matches(&self, text: &str) -> bool {
        self.search.is_empty() || text.to_lowercase().contains(&self.search.to_lowercase())
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
            Screen::Overview | Screen::Info => self.sections().len(),
            Screen::Execution => self.visible_nodes().len(),
            Screen::Flow => self.tx().map(solana::transfers).unwrap_or_default().len(),
            Screen::Invocation => self
                .node_index
                .and_then(|i| self.tx().map(|tx| tx.nodes[i].accounts.len()))
                .unwrap_or(0),
            Screen::Accounts => self.account_rows().len(),
            Screen::StateDiff => self.balance_rows().len(),
            Screen::Logs => self.log_rows().len(),
            Screen::Raw | Screen::Data => 0,
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
        let start = self.selected_row();
        let hit = (1..=visible.len())
            .map(|offset| (start + offset) % visible.len().max(1))
            .find(|&i| visible.get(i).is_some_and(|&n| self.node_matches(n)));
        if let Some(i) = hit {
            self.list.select(Some(i));
        }
    }
}
