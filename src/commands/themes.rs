//! `aitop themes`: list themes, or print one as a template.

use std::io::IsTerminal;

use anyhow::Result;

use super::Context;
use crate::paths::{config_dir, tilde};
use crate::theme::{self, Palette, ThemeFile};

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
    let bg = theme::ansi(p.background, true);
    let blocks: String = [p.claude, p.codex, p.text, p.dim, p.good, p.warn, p.bad]
        .iter()
        .map(|&c| format!("{}██", theme::ansi(c, false)))
        .collect();
    format!("{bg} {blocks} \x1b[0m")
}
