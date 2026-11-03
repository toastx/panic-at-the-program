//! The `AnchorError` lines Anchor logs, and the `Left:`/`Right:` pair after a
//! failed `require_eq!`.

/// What one `AnchorError` log line, and the `Left:`/`Right:` pair after a
/// failed `require_eq!`, say about the failure.
#[derive(Default)]
pub struct Log {
    pub name: Option<String>,
    pub number: Option<u32>,
    pub message: Option<String>,
    pub account: Option<String>,
    pub source: Option<String>,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

/// One side of a failed comparison. Anchor logs a plain value inline,
/// `Left: 5`, but a pubkey on the line after a bare `Left:`.
fn compared(inline: &str, next: Option<&&str>) -> Option<String> {
    let inline = inline.trim();
    if inline.is_empty() {
        next.map(|next| next.trim().to_string())
    } else {
        Some(inline.to_string())
    }
}

fn field(line: &str, key: &str) -> Option<String> {
    let value = &line[line.find(key)? + key.len()..];
    let end = value.find(". ").unwrap_or(value.len());
    Some(value[..end].trim().trim_end_matches('.').to_string())
}

pub fn log(logs: &[String]) -> Log {
    let mut parsed = Log::default();
    let mut lines = logs
        .iter()
        .filter_map(|line| line.strip_prefix("Program log: "))
        .peekable();

    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("AnchorError ") {
            let head = rest.split("Error Code: ").next().unwrap_or_default();
            if let Some(account) = head.strip_prefix("caused by account: ") {
                parsed.account = Some(account.trim().trim_end_matches('.').to_string());
            }
            if let Some(source) = head.strip_prefix("thrown in ") {
                parsed.source = Some(source.trim().trim_end_matches('.').to_string());
            }
            parsed.name = field(rest, "Error Code: ").or(parsed.name);
            parsed.number = field(rest, "Error Number: ")
                .and_then(|v| v.parse().ok())
                .or(parsed.number);
            parsed.message = field(rest, "Error Message: ").or(parsed.message);
            continue;
        }
        if let Some(inline) = line.strip_prefix("Left:") {
            parsed.actual = compared(inline, lines.peek());
            continue;
        }
        if let Some(inline) = line.strip_prefix("Right:") {
            parsed.expected = compared(inline, lines.peek());
            continue;
        }
        if let Some(message) = line.strip_prefix("Error: ") {
            parsed.message.get_or_insert_with(|| message.trim().to_string());
        }
    }
    parsed
}
