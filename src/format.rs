//! Human-friendly number, money and time formatting.

pub fn tokens(n: u64) -> String {
    let f = n as f64;
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        trim(f / 1e3, "K")
    } else if n < 1_000_000_000 {
        trim(f / 1e6, "M")
    } else {
        trim(f / 1e9, "B")
    }
}

fn trim(v: f64, unit: &str) -> String {
    if v >= 100.0 {
        format!("{v:.0}{unit}")
    } else if v >= 10.0 {
        format!("{v:.1}{unit}")
    } else {
        format!("{v:.2}{unit}")
    }
}

pub fn cost(c: f64) -> String {
    if c <= 0.0 {
        "$0.00".into()
    } else if c < 0.01 {
        "<$0.01".into()
    } else if c < 1_000.0 {
        format!("${c:.2}")
    } else if c < 100_000.0 {
        format!("${}", thousands(c.round() as u64))
    } else {
        format!("${:.1}K", c / 1e3)
    }
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Compact duration, e.g. `45s`, `12m`, `2h 05m`, `3d 4h`.
pub fn duration(ms: i64) -> String {
    let s = (ms.max(0) / 1000) as u64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d {}h", s / 86_400, (s % 86_400) / 3600)
    }
}

/// Coarse "time ago" used in tables: `now`, `5m`, `3h`, `2d`, `5w`.
pub fn ago(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else if s < 86_400 * 14 {
        format!("{}d", s / 86_400)
    } else {
        format!("{}w", s / (86_400 * 7))
    }
}

/// Shorten model ids for display: `claude-opus-4-5-20251101` -> `opus-4-5`.
pub fn short_model(m: &str) -> String {
    let mut s = m.strip_prefix("claude-").unwrap_or(m).to_string();
    if let Some(idx) = s.rfind('-') {
        let tail = &s[idx + 1..];
        if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) {
            s.truncate(idx);
        }
    }
    s
}

/// Format a time with a user-supplied strftime pattern. `None` for an empty
/// pattern; an invalid one falls back to `%H:%M:%S` rather than panicking.
pub fn clock(t: chrono::DateTime<chrono::Local>, pattern: &str) -> Option<String> {
    use chrono::format::{Item, StrftimeItems};
    if pattern.trim().is_empty() {
        return None;
    }
    let valid = !StrftimeItems::new(pattern).any(|i| matches!(i, Item::Error));
    Some(t.format(if valid { pattern } else { "%H:%M:%S" }).to_string())
}

/// Truncate to `width` display columns, adding an ellipsis.
pub fn truncate(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_234), "1.23K");
        assert_eq!(tokens(987_200), "987K");
        assert_eq!(tokens(45_200_000), "45.2M");
        assert_eq!(tokens(3_100_000_000), "3.10B");
        assert_eq!(cost(0.0), "$0.00");
        assert_eq!(cost(0.004), "<$0.01");
        assert_eq!(cost(12.345), "$12.35");
        assert_eq!(cost(12_345.0), "$12,345");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(duration(7_980_000), "2h 13m");
        assert_eq!(ago(30_000), "now");
        assert_eq!(ago(3 * 3_600_000), "3h");
        assert_eq!(short_model("claude-opus-4-5-20251101"), "opus-4-5");
        assert_eq!(short_model("gpt-5.1-codex"), "gpt-5.1-codex");
        assert_eq!(truncate("abcdef", 4), "abc…");
        let t = chrono::TimeZone::with_ymd_and_hms(&chrono::Local, 2026, 1, 2, 15, 4, 5).unwrap();
        assert_eq!(clock(t, "%H:%M").as_deref(), Some("15:04"));
        assert_eq!(clock(t, "%I:%M %p").as_deref(), Some("03:04 PM"));
        assert_eq!(clock(t, "%Q"), Some("15:04:05".into()), "invalid falls back");
        assert_eq!(clock(t, ""), None);
    }
}
