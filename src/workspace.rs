use std::path::PathBuf;

pub fn root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("solwork.toml").is_file() || dir.join(".solwork").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub fn idl_dir() -> PathBuf {
    root().unwrap_or_default().join("target/idl")
}

pub fn idl_path(_program: &str) -> Option<PathBuf> {
    None
}
