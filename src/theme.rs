//! Colours. Truecolor when the terminal advertises it, otherwise the nearest
//! xterm-256 colour; `NO_COLOR` disables colour entirely.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};

use crate::event::Tool;

#[derive(Clone, Copy)]
enum Mode {
    TrueColor,
    Ansi256,
    None,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Mode::None;
        }
        let ct = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
        let term = std::env::var("TERM_PROGRAM").unwrap_or_default();
        if ct.contains("truecolor")
            || ct.contains("24bit")
            || matches!(term.as_str(), "iTerm.app" | "WezTerm" | "vscode" | "ghostty")
        {
            Mode::TrueColor
        } else {
            Mode::Ansi256
        }
    })
}

pub fn rgb(r: u8, g: u8, b: u8) -> Color {
    match mode() {
        Mode::TrueColor => Color::Rgb(r, g, b),
        Mode::Ansi256 => Color::Indexed(to_ansi256(r, g, b)),
        Mode::None => Color::Reset,
    }
}

fn to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // Greys map onto the 24-step ramp, everything else onto the 6x6x6 cube.
    if r.abs_diff(g) < 10 && g.abs_diff(b) < 10 {
        let avg = (r as u16 + g as u16 + b as u16) / 3;
        if avg < 8 {
            return 16;
        }
        if avg > 238 {
            return 231;
        }
        return 232 + ((avg - 8) * 24 / 231) as u8;
    }
    let q = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            ((v - 35) / 40).min(5)
        }
    };
    16 + 36 * q(r) + 6 * q(g) + q(b)
}

pub fn accent(t: Tool) -> Color {
    match t {
        Tool::Claude => rgb(0xe0, 0x8a, 0x5f),
        Tool::Codex => rgb(0x4c, 0xc3, 0x8a),
        Tool::Gemini => rgb(0x6f, 0x9c, 0xf6),
    }
}

pub fn border() -> Color {
    rgb(0x4a, 0x4d, 0x5c)
}

pub fn dim() -> Color {
    rgb(0x80, 0x84, 0x94)
}

pub fn faint() -> Color {
    rgb(0x3a, 0x3d, 0x4a)
}

pub fn text() -> Color {
    rgb(0xd8, 0xda, 0xe2)
}

pub fn bright() -> Color {
    rgb(0xf4, 0xf5, 0xf8)
}

pub fn warn() -> Color {
    rgb(0xe6, 0xc0, 0x4a)
}

pub fn good() -> Color {
    rgb(0x5c, 0xd0, 0x7d)
}

pub fn select_bg() -> Color {
    rgb(0x2c, 0x30, 0x3e)
}

pub fn dim_style() -> Style {
    Style::new().fg(dim())
}

pub fn label_style() -> Style {
    Style::new().fg(dim())
}

pub fn value_style() -> Style {
    Style::new().fg(text()).add_modifier(Modifier::BOLD)
}

/// btop-style green -> yellow -> red gradient for `f` in 0..=1.
pub fn gradient(f: f64) -> Color {
    let f = f.clamp(0.0, 1.0);
    let (a, b, t) = if f < 0.5 {
        ((0x4c, 0xd0, 0x7d), (0xe6, 0xc8, 0x4a), f * 2.0)
    } else {
        ((0xe6, 0xc8, 0x4a), (0xe5, 0x53, 0x4b), (f - 0.5) * 2.0)
    };
    let lerp = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    rgb(lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi256_mapping() {
        assert_eq!(to_ansi256(0, 0, 0), 16);
        assert_eq!(to_ansi256(255, 255, 255), 231);
        assert_eq!(to_ansi256(255, 0, 0), 196);
        assert!((232..=255).contains(&to_ansi256(128, 128, 128)));
    }
}
