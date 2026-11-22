//! The name of this machine.
//!
//! Every usage record carries the machine it was made on, so that usage
//! pulled from other machines (see [`crate::remote`]) can be told apart.

/// The configured `host`, otherwise the system hostname in lower case
/// without a `.local` / `.lan` suffix.
pub fn local(configured: Option<&str>) -> String {
    match configured.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => name.to_string(),
        None => normalize(&gethostname::gethostname().to_string_lossy()),
    }
}

pub fn normalize(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let short = [".local", ".lan", ".home"].iter().find_map(|s| lower.strip_suffix(s)).unwrap_or(&lower);
    if short.is_empty() {
        "local".into()
    } else {
        short.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(normalize("Workshop-Mac.local"), "workshop-mac");
        assert_eq!(normalize("box.example.com"), "box.example.com");
        assert_eq!(normalize(""), "local");
        assert_eq!(local(Some(" desk ")), "desk");
        assert!(!local(Some("")).is_empty());
    }
}
