use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct Config {
    pub root: PathBuf,
    sections: HashMap<String, HashMap<String, String>>,
}

thread_local! {
    static CONFIG: RefCell<Option<Option<Config>>> = const { RefCell::new(None) };
}

fn with_config<T>(read: impl FnOnce(Option<&Config>) -> T) -> T {
    CONFIG.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(load());
        }
        read(slot.as_ref().and_then(|config| config.as_ref()))
    })
}

pub fn reload() {
    CONFIG.with(|cell| *cell.borrow_mut() = None);
}

fn load() -> Option<Config> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        for name in ["patp.toml", ".patp"] {
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

pub fn project_root() -> Option<PathBuf> {
    if let Some(root) = root() {
        return Some(root);
    }
    let start = std::env::current_dir().ok()?;
    for marker in ["Anchor.toml", "Cargo.toml", ".git"] {
        let mut dir = start.clone();
        loop {
            if dir.join(marker).exists() {
                return Some(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    Some(start)
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
    with_config(|config| config?.sections.get(section)?.get(key).cloned())
}

pub fn label(section: &str, key: &str) -> Option<String> {
    value(section, key)
}

pub fn root() -> Option<PathBuf> {
    with_config(|config| Some(config?.root.clone()))
}

pub fn idl_dir() -> PathBuf {
    let base = project_root().unwrap_or_default();
    match value("", "idl_dir") {
        Some(dir) => base.join(dir),
        None => base.join("target/idl"),
    }
}

pub fn idl_path(program: &str) -> Option<PathBuf> {
    Some(root()?.join(value("idl", program)?))
}

pub fn append_entry(section: &str, key: &str, value_text: &str) -> std::io::Result<()> {
    let Some(root) = project_root() else { return Ok(()) };
    let path = ["patp.toml", ".patp"]
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| root.join("patp.toml"));

    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let header = format!("[{section}]");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let entry = format!("{key} = \"{value_text}\"");

    match lines.iter().position(|line| line.trim() == header) {
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|line| line.trim().starts_with('['))
                .map(|offset| start + 1 + offset)
                .unwrap_or(lines.len());
            if let Some(existing) = lines[start + 1..end]
                .iter()
                .position(|line| line.trim_start().starts_with(key))
            {
                lines[start + 1 + existing] = entry;
            } else {
                lines.insert(end, entry);
            }
        }
        None => {
            if !lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.push(entry);
        }
    }

    std::fs::write(&path, format!("{}\n", lines.join("\n")))?;
    reload();
    Ok(())
}

pub fn error_label(program: &str, code: u32) -> Option<(String, String)> {
    let text = value(&format!("errors.{program}"), &code.to_string())?;
    if text.trim().is_empty() {
        return None;
    }
    Some(match text.split_once(':') {
        Some((name, message)) => (name.trim().to_string(), message.trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    })
}

pub fn record_error_stub(program: &str, code: u32) {
    if value(&format!("errors.{program}"), &code.to_string()).is_some() {
        return;
    }
    let _ = append_entry(&format!("errors.{program}"), &code.to_string(), "");
}

pub fn record_flavor(program: &str, flavor: &str) {
    if value("idl.kind", program).as_deref() == Some(flavor) {
        return;
    }
    let _ = append_entry("idl.kind", program, flavor);
}
