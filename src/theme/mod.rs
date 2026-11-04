//! Colours.
//!
//! The active [`Palette`] comes from a theme (see [`file`] and [`ThemeSet`])
//! and is read through the small accessor functions below, so drawing code
//! never names a colour directly. Colours are converted for the terminal on
//! the way out: truecolor when the terminal advertises it, otherwise the
//! nearest xterm-256 colour, and nothing at all when `NO_COLOR` is set.

mod file;
mod set;

use std::cell::Cell;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};

use crate::event::Tool;

pub use file::ThemeFile;
pub use set::ThemeSet;

/// The colours a theme defines. Values are stored as the theme wrote them
/// (RGB or ANSI); terminal conversion happens in [`out`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// `Color::Reset` keeps the terminal's own background.
    pub background: Color,
    pub text: Color,
    pub bright: Color,
    pub dim: Color,
    pub faint: Color,
    pub border: Color,
    pub selection: Color,
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
    pub claude: Color,
    pub codex: Color,
    /// Low / middle / high stops of meter gradients.
    pub gradient: [Color; 3],
}

thread_local! {
    static CURRENT: Cell<Palette> = Cell::new(ThemeSet::builtin_default().palette);
}

/// Make `p` the palette used by every accessor (on this thread, which is
/// the one that draws).
pub fn set(p: Palette) {
    CURRENT.with(|c| c.set(p));
}

pub fn current() -> Palette {
    CURRENT.with(|c| c.get())
}

#[derive(Clone, Copy, PartialEq, Debug)]
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

/// Convert a theme colour for this terminal.
fn out(c: Color) -> Color {
    convert(c, mode())
}

fn convert(c: Color, mode: Mode) -> Color {
    match (mode, c) {
        (Mode::None, _) => Color::Reset,
        (Mode::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(to_ansi256(r, g, b)),
        _ => c,
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
    let p = current();
    out(match t {
        Tool::Claude => p.claude,
        Tool::Codex => p.codex,
    })
}

pub fn background() -> Color {
    out(current().background)
}

pub fn border() -> Color {
    out(current().border)
}

pub fn dim() -> Color {
    out(current().dim)
}

pub fn faint() -> Color {
    out(current().faint)
}

pub fn text() -> Color {
    out(current().text)
}

pub fn bright() -> Color {
    out(current().bright)
}

pub fn warn() -> Color {
    out(current().warn)
}

pub fn good() -> Color {
    out(current().good)
}

pub fn bad() -> Color {
    out(current().bad)
}

pub fn select_bg() -> Color {
    out(current().selection)
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

/// btop-style low → mid → high gradient for `f` in 0..=1. RGB stops are
/// blended; ANSI stops (which can't be blended) switch at thirds.
pub fn gradient(f: f64) -> Color {
    out(gradient_raw(current().gradient, f))
}

fn gradient_raw(stops: [Color; 3], f: f64) -> Color {
    let f = f.clamp(0.0, 1.0);
    let (a, b, t) = if f < 0.5 { (stops[0], stops[1], f * 2.0) } else { (stops[1], stops[2], (f - 0.5) * 2.0) };
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let lerp = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
            Color::Rgb(lerp(r1, r2), lerp(g1, g2), lerp(b1, b2))
        }
        _ => stops[((f * 3.0) as usize).min(2)],
    }
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

    #[test]
    fn conversion_by_mode() {
        let c = Color::Rgb(255, 0, 0);
        assert_eq!(convert(c, Mode::TrueColor), c);
        assert_eq!(convert(c, Mode::Ansi256), Color::Indexed(196));
        assert_eq!(convert(Color::Red, Mode::Ansi256), Color::Red);
        assert_eq!(convert(c, Mode::None), Color::Reset);
    }

    #[test]
    fn gradients() {
        let rgb = [Color::Rgb(0, 0, 0), Color::Rgb(100, 100, 100), Color::Rgb(200, 0, 0)];
        assert_eq!(gradient_raw(rgb, 0.0), Color::Rgb(0, 0, 0));
        assert_eq!(gradient_raw(rgb, 0.25), Color::Rgb(50, 50, 50));
        assert_eq!(gradient_raw(rgb, 1.0), Color::Rgb(200, 0, 0));
        let ansi = [Color::Green, Color::Yellow, Color::Red];
        assert_eq!(gradient_raw(ansi, 0.1), Color::Green);
        assert_eq!(gradient_raw(ansi, 0.5), Color::Yellow);
        assert_eq!(gradient_raw(ansi, 0.9), Color::Red);
    }

    #[test]
    fn set_switches_palette() {
        let mut p = current();
        p.text = Color::Rgb(1, 2, 3);
        set(p);
        assert_eq!(current().text, Color::Rgb(1, 2, 3));
    }
}
