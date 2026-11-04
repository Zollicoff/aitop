//! `aitop themes`: list themes, or print one as a template.

use std::io::IsTerminal;

use anyhow::Result;
use ratatui::style::Color;

use super::Context;
use crate::paths::{config_dir, tilde};
use crate::theme::{Palette, ThemeFile};

pub fn list(ctx: &Context) -> Result<()> {
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    let width = ctx.themes.themes.iter().map(|t| t.key.len()).max().unwrap_or(0);
    for t in &ctx.themes.themes {
        let marker = if t.key == ctx.theme { "●" } else { " " };
        let source = match &t.path {
            Some(p) => tilde(p),
            None => "built-in".into(),
        };
        let swatch = if color { swatch(&t.palette) } else { String::new() };
        println!("{marker} {:<width$}  {swatch}  {} ({source})", t.key, t.name);
    }
    for w in &ctx.themes.warnings {
        println!("! skipped {w}");
    }
    println!(
        "\nUse one with `--theme NAME`, `theme = \"NAME\"` in the config, or press `t` in the dashboard.\n\
         Custom themes go in {}/NAME.toml (`aitop themes show NAME` prints a template).",
        tilde(&config_dir().join("themes"))
    );
    Ok(())
}

pub fn show(ctx: &Context, name: &str) -> Result<()> {
    let t = ctx.themes.get(name)?;
    println!("# aitop theme. Save as {}/NAME.toml and edit.", tilde(&config_dir().join("themes")));
    print!("{}", ThemeFile::from_palette(&t.name, &t.palette).to_toml());
    Ok(())
}

/// A row of coloured blocks on the theme's background.
fn swatch(p: &Palette) -> String {
    let bg = sgr(p.background, true);
    let blocks: String = [p.claude, p.codex, p.text, p.dim, p.good, p.warn, p.bad]
        .iter()
        .map(|&c| format!("{}██", sgr(c, false)))
        .collect();
    format!("{bg} {blocks} \x1b[0m")
}

/// ANSI escape selecting `c` as foreground (or background).
fn sgr(c: Color, background: bool) -> String {
    let (base, bright, ext) = if background { (40, 100, 48) } else { (30, 90, 38) };
    let ansi =
        [Color::Black, Color::Red, Color::Green, Color::Yellow, Color::Blue, Color::Magenta, Color::Cyan, Color::Gray];
    let light = [
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    match c {
        Color::Rgb(r, g, b) => format!("\x1b[{ext};2;{r};{g};{b}m"),
        Color::Indexed(i) => format!("\x1b[{ext};5;{i}m"),
        Color::Reset => format!("\x1b[{}m", if background { 49 } else { 39 }),
        other => match (ansi.iter().position(|&a| a == other), light.iter().position(|&a| a == other)) {
            (Some(i), _) => format!("\x1b[{}m", base + i),
            (_, Some(i)) => format!("\x1b[{}m", bright + i),
            _ => String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(sgr(Color::Rgb(1, 2, 3), false), "\x1b[38;2;1;2;3m");
        assert_eq!(sgr(Color::Red, false), "\x1b[31m");
        assert_eq!(sgr(Color::LightCyan, true), "\x1b[106m");
        assert_eq!(sgr(Color::Indexed(208), true), "\x1b[48;5;208m");
        assert_eq!(sgr(Color::Reset, true), "\x1b[49m");
    }
}
