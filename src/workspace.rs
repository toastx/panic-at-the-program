use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Config {
    pub root: PathBuf,
    sections: HashMap<String, HashMap<String, String>>,
}

static CONFIG: OnceLock<Option<Config>> = OnceLock::new();

pub fn config() -> Option<&'static Config> {
    CONFIG.get_or_init(load).as_ref()
}

fn load() -> Option<Config> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        for name in ["solwork.toml", ".solwork"] {
            let path = dir.join(name);
            if path.is_file() {
                let text = std::fs::read_to_string(&path).ok()?;
                return Some(Config { root: dir, sections: parse(&text) });
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub fn parse(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        sections
            .entry(current.clone())
            .or_default()
            .insert(key.trim().trim_matches('"').to_string(), value.to_string());
    }
    sections
}

pub fn value(section: &str, key: &str) -> Option<String> {
    config()?.sections.get(section)?.get(key).cloned()
}

pub fn label(section: &str, key: &str) -> Option<String> {
    value(section, key)
}

pub fn root() -> Option<PathBuf> {
    Some(config()?.root.clone())
}

pub fn idl_dir() -> PathBuf {
    let base = root().unwrap_or_default();
    match value("", "idl_dir") {
        Some(dir) => base.join(dir),
        None => base.join("target/idl"),
    }
}

pub fn idl_path(program: &str) -> Option<PathBuf> {
    Some(root()?.join(value("idl", program)?))
}
