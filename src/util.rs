use std::time::{SystemTime, UNIX_EPOCH};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Terminal display width of a string (wide CJK chars count as 2).
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Width of a single char, with control characters treated as zero.
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Truncate to `max` display columns, appending an ellipsis when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    let limit = max.saturating_sub(1);
    for c in s.chars() {
        let w = char_width(c);
        if used + w > limit {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("page");
    }
    out.truncate(60);
    out
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Format a unix timestamp as `YYYY-MM-DD HH:MM` in UTC without extra deps.
pub fn format_ts(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let secs = ts % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

// Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Heuristic: does the input look like something we should fetch directly?
pub fn looks_like_url(input: &str) -> bool {
    let s = input.trim();
    if s.is_empty() {
        return false;
    }
    if s.starts_with("panzki:") || s.starts_with("file://") {
        return true;
    }
    if s.contains("://") {
        return true;
    }
    if s.starts_with("localhost") || s.starts_with("127.") || s.starts_with("[::1]") {
        return true;
    }
    if s.split_whitespace().count() > 1 {
        return false;
    }
    // Domain-ish: contains a dot and no spaces, or has a port/path on localhost.
    s.contains('.') && !s.starts_with('.') && !s.ends_with('.')
}

/// Turn free-form input into a fetchable URL, using `search` as a template
/// (`{}` is replaced with the percent-encoded query).
pub fn normalize_input(input: &str, search: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return String::new();
    }
    if looks_like_url(s) {
        if s.contains("://") || s.starts_with("panzki:") || s.starts_with("file://") {
            return s.to_string();
        }
        return format!("https://{s}");
    }
    let encoded: String =
        percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string();
    search.replace("{}", &encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_wide_text() {
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn detects_urls() {
        assert!(looks_like_url("example.com"));
        assert!(looks_like_url("https://example.com/a b"));
        assert!(!looks_like_url("rust programming"));
        assert!(!looks_like_url("hello"));
    }

    #[test]
    fn normalizes_input() {
        assert_eq!(
            normalize_input("example.com", "https://x/?q={}"),
            "https://example.com"
        );
        assert!(normalize_input("rust browser", "https://x/?q={}").starts_with("https://x/?q="));
    }

    #[test]
    fn formats_dates() {
        assert_eq!(format_ts(0), "1970-01-01 00:00");
        assert_eq!(format_ts(1_700_000_000), "2023-11-14 22:13");
    }
}
