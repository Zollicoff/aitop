//! Theme files.
//!
//! ```toml
//! name = "My theme"        # optional display name
//! base = "dracula"         # optional: start from another theme
//! background = "#282a36"   # "default" keeps the terminal's background
//! text = "#f8f8f2"
//! claude = "bright-red"    # ANSI names and 0-255 indexes work too
//! gradient = ["#50fa7b", "#f1fa8c", "#ff5555"]
//! ```
//!
//! Every colour is optional; anything left out comes from `base` (or the
//! default theme).

use anyhow::{anyhow, bail, Context, Result};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use super::Palette;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub background: Option<String>,
    pub text: Option<String>,
    pub bright: Option<String>,
    pub dim: Option<String>,
    pub faint: Option<String>,
    pub border: Option<String>,
    pub selection: Option<String>,
    pub good: Option<String>,
    pub warn: Option<String>,
    pub bad: Option<String>,
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub gradient: Option<[String; 3]>,
}

impl ThemeFile {
    pub fn parse(text: &str) -> Result<ThemeFile> {
        Ok(toml::from_str(text)?)
    }

    /// The palette this file describes, on top of `base`.
    pub fn apply(&self, base: Palette) -> Result<Palette> {
        let mut p = base;
        let fields: [(&str, &Option<String>, &mut Color); 12] = [
            ("background", &self.background, &mut p.background),
            ("text", &self.text, &mut p.text),
            ("bright", &self.bright, &mut p.bright),
            ("dim", &self.dim, &mut p.dim),
            ("faint", &self.faint, &mut p.faint),
            ("border", &self.border, &mut p.border),
            ("selection", &self.selection, &mut p.selection),
            ("good", &self.good, &mut p.good),
            ("warn", &self.warn, &mut p.warn),
            ("bad", &self.bad, &mut p.bad),
            ("claude", &self.claude, &mut p.claude),
            ("codex", &self.codex, &mut p.codex),
        ];
        for (key, value, slot) in fields {
            if let Some(v) = value {
                *slot = parse_color(v).with_context(|| format!("`{key}`"))?;
            }
        }
        if let Some(stops) = &self.gradient {
            for (slot, v) in p.gradient.iter_mut().zip(stops) {
                *slot = parse_color(v).context("`gradient`")?;
            }
        }
        Ok(p)
    }

    /// A complete file for `palette`, for `aitop themes show`.
    pub fn from_palette(name: &str, p: &Palette) -> ThemeFile {
        let c = |c: Color| Some(color_name(c));
        ThemeFile {
            name: Some(name.to_string()),
            base: None,
            background: c(p.background),
            text: c(p.text),
            bright: c(p.bright),
            dim: c(p.dim),
            faint: c(p.faint),
            border: c(p.border),
            selection: c(p.selection),
            good: c(p.good),
            warn: c(p.warn),
            bad: c(p.bad),
            claude: c(p.claude),
            codex: c(p.codex),
            gradient: Some(p.gradient.map(color_name)),
        }
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("theme serializes")
    }
}

const ANSI: [(&str, Color); 16] = [
    ("black", Color::Black),
    ("red", Color::Red),
    ("green", Color::Green),
    ("yellow", Color::Yellow),
    ("blue", Color::Blue),
    ("magenta", Color::Magenta),
    ("cyan", Color::Cyan),
    ("white", Color::Gray),
    ("bright-black", Color::DarkGray),
    ("bright-red", Color::LightRed),
    ("bright-green", Color::LightGreen),
    ("bright-yellow", Color::LightYellow),
    ("bright-blue", Color::LightBlue),
    ("bright-magenta", Color::LightMagenta),
    ("bright-cyan", Color::LightCyan),
    ("bright-white", Color::White),
];

/// `#rrggbb`, `#rgb`, an ANSI colour name (`red`, `bright-black`, …), an
/// xterm-256 index (`0`–`255`), or `default` for the terminal's own colour.
pub fn parse_color(s: &str) -> Result<Color> {
    let t = s.trim().to_ascii_lowercase().replace('_', "-");
    if let Some(hex) = t.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()
            .ok_or_else(|| anyhow!("invalid colour {s:?}"))?;
        return match digits.as_slice() {
            [r, g, b] => Ok(Color::Rgb(r * 17, g * 17, b * 17)),
            [r1, r2, g1, g2, b1, b2] => Ok(Color::Rgb(r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)),
            _ => bail!("invalid colour {s:?}: use #rrggbb"),
        };
    }
    if matches!(t.as_str(), "default" | "reset" | "none") {
        return Ok(Color::Reset);
    }
    let t = if t == "gray" || t == "grey" { "bright-black".to_string() } else { t };
    if let Some((_, c)) = ANSI.iter().find(|(n, _)| *n == t) {
        return Ok(*c);
    }
    if let Ok(i) = t.parse::<u8>() {
        return Ok(Color::Indexed(i));
    }
    bail!("unknown colour {s:?}: use #rrggbb, an ANSI name like \"bright-black\", 0-255, or \"default\"")
}

/// The inverse of [`parse_color`].
pub fn color_name(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => i.to_string(),
        Color::Reset => "default".into(),
        other => ANSI.iter().find(|(_, a)| *a == other).map(|(n, _)| n.to_string()).unwrap_or_else(|| "default".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeSet;

    #[test]
    fn colours() {
        assert_eq!(parse_color("#ff8000").unwrap(), Color::Rgb(255, 128, 0));
        assert_eq!(parse_color("#F80").unwrap(), Color::Rgb(255, 136, 0));
        assert_eq!(parse_color("bright_black").unwrap(), Color::DarkGray);
        assert_eq!(parse_color("grey").unwrap(), Color::DarkGray);
        assert_eq!(parse_color("Default").unwrap(), Color::Reset);
        assert_eq!(parse_color("208").unwrap(), Color::Indexed(208));
        for bad in ["#12345", "#gggggg", "purple-ish", "256"] {
            assert!(parse_color(bad).is_err(), "{bad}");
        }
        for c in [Color::Rgb(1, 2, 3), Color::LightCyan, Color::Gray, Color::Indexed(9), Color::Reset] {
            assert_eq!(parse_color(&color_name(c)).unwrap(), c);
        }
    }

    #[test]
    fn partial_file_overrides_base() {
        let base = ThemeSet::builtin_default().palette;
        let f = ThemeFile::parse("text = \"#010203\"\ngradient = [\"red\", \"yellow\", \"green\"]").unwrap();
        let p = f.apply(base).unwrap();
        assert_eq!(p.text, Color::Rgb(1, 2, 3));
        assert_eq!(p.gradient, [Color::Red, Color::Yellow, Color::Green]);
        assert_eq!(p.border, base.border);
        assert!(ThemeFile::parse("txet = \"#000000\"").is_err(), "typos are reported");
        assert!(ThemeFile::parse("text = \"nope\"").unwrap().apply(base).is_err());
    }

    #[test]
    fn roundtrip() {
        let p = ThemeSet::builtin_default().palette;
        let text = ThemeFile::from_palette("x", &p).to_toml();
        assert_eq!(ThemeFile::parse(&text).unwrap().apply(p).unwrap(), p);
    }
}
