//! The available themes: built-ins plus `~/.config/aitop/themes/*.toml`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};

use super::file::ThemeFile;
use super::Palette;

const BUILTIN: [(&str, &str); 8] = [
    ("aitop", include_str!("builtin/aitop.toml")),
    ("catppuccin", include_str!("builtin/catppuccin.toml")),
    ("dracula", include_str!("builtin/dracula.toml")),
    ("gruvbox", include_str!("builtin/gruvbox.toml")),
    ("nord", include_str!("builtin/nord.toml")),
    ("solarized-light", include_str!("builtin/solarized-light.toml")),
    ("terminal", include_str!("builtin/terminal.toml")),
    ("tokyo-night", include_str!("builtin/tokyo-night.toml")),
];

#[derive(Clone, Debug)]
pub struct Theme {
    /// Lookup key: the file name without `.toml`.
    pub key: String,
    /// Display name.
    pub name: String,
    pub palette: Palette,
    /// `None` for built-ins.
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct ThemeSet {
    pub themes: Vec<Theme>,
    /// Problems with user theme files (they are skipped, not fatal).
    pub warnings: Vec<String>,
}

fn normalize(name: &str) -> String {
    name.trim().to_lowercase().replace([' ', '_'], "-")
}

impl ThemeSet {
    /// The default theme, parsed once.
    pub fn builtin_default() -> &'static Theme {
        static DEFAULT: OnceLock<Theme> = OnceLock::new();
        DEFAULT.get_or_init(|| {
            let f = ThemeFile::parse(BUILTIN[0].1).expect("built-in theme parses");
            let palette = f.apply(FALLBACK).expect("built-in theme is valid");
            Theme { key: BUILTIN[0].0.into(), name: f.name.unwrap_or_default(), palette, path: None }
        })
    }

    pub fn builtins() -> ThemeSet {
        let base = Self::builtin_default().palette;
        let themes = BUILTIN
            .iter()
            .map(|(key, text)| {
                let f = ThemeFile::parse(text).expect("built-in theme parses");
                let palette = f.apply(base).expect("built-in theme is valid");
                Theme { key: key.to_string(), name: f.name.unwrap_or_else(|| key.to_string()), palette, path: None }
            })
            .collect();
        ThemeSet { themes, warnings: Vec::new() }
    }

    /// Built-ins plus the `*.toml` files in `dir` (if it exists).
    pub fn load(dir: &Path) -> ThemeSet {
        let mut set = Self::builtins();
        let Ok(rd) = std::fs::read_dir(dir) else { return set };
        let mut paths: Vec<PathBuf> =
            rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
        paths.sort();
        for path in paths {
            match set.load_file(&path) {
                Ok(theme) => set.insert(theme),
                Err(e) => set.warnings.push(format!("{}: {e:#}", crate::paths::tilde(&path))),
            }
        }
        set
    }

    fn load_file(&self, path: &Path) -> Result<Theme> {
        let text = std::fs::read_to_string(path)?;
        let f = ThemeFile::parse(&text)?;
        let key = normalize(&path.file_stem().unwrap_or_default().to_string_lossy());
        // Without `base`, a file named after an existing theme tweaks that theme.
        let base = match &f.base {
            Some(b) => self.find(b).ok_or_else(|| anyhow!("unknown base theme {b:?}"))?.palette,
            None => self.find(&key).map(|t| t.palette).unwrap_or(Self::builtin_default().palette),
        };
        let palette = f.apply(base).context("invalid colour")?;
        Ok(Theme { name: f.name.clone().unwrap_or_else(|| key.clone()), key, palette, path: Some(path.to_path_buf()) })
    }

    /// Add a theme, replacing one with the same key.
    fn insert(&mut self, theme: Theme) {
        match self.themes.iter_mut().find(|t| t.key == theme.key) {
            Some(slot) => *slot = theme,
            None => self.themes.push(theme),
        }
    }

    /// Look up by key or display name, ignoring case, spaces and underscores.
    pub fn find(&self, name: &str) -> Option<&Theme> {
        let n = normalize(name);
        self.themes.iter().find(|t| t.key == n || normalize(&t.name) == n)
    }

    /// `find`, with a helpful error listing what exists.
    pub fn get(&self, name: &str) -> Result<&Theme> {
        self.find(name).ok_or_else(|| {
            let names: Vec<&str> = self.themes.iter().map(|t| t.key.as_str()).collect();
            anyhow!("unknown theme {name:?}; available: {}", names.join(", "))
        })
    }

    /// The theme `delta` steps after `key`, wrapping around.
    pub fn cycle(&self, key: &str, delta: isize) -> &Theme {
        let i = self.themes.iter().position(|t| t.key == key).unwrap_or(0) as isize;
        &self.themes[(i + delta).rem_euclid(self.themes.len() as isize) as usize]
    }
}

/// Only used to parse the default theme, which defines every colour.
const FALLBACK: Palette = Palette {
    background: ratatui::style::Color::Reset,
    text: ratatui::style::Color::Reset,
    bright: ratatui::style::Color::Reset,
    dim: ratatui::style::Color::Reset,
    faint: ratatui::style::Color::Reset,
    border: ratatui::style::Color::Reset,
    selection: ratatui::style::Color::Reset,
    good: ratatui::style::Color::Reset,
    warn: ratatui::style::Color::Reset,
    bad: ratatui::style::Color::Reset,
    claude: ratatui::style::Color::Reset,
    codex: ratatui::style::Color::Reset,
    gradient: [ratatui::style::Color::Reset; 3],
};

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn builtins_are_complete_and_distinct() {
        let set = ThemeSet::builtins();
        assert_eq!(set.themes.len(), BUILTIN.len());
        for (key, text) in BUILTIN {
            let f = ThemeFile::parse(text).unwrap();
            // Every built-in defines every colour, so none depends on the default.
            let all = [&f.background, &f.text, &f.bright, &f.dim, &f.faint, &f.border, &f.selection];
            let more = [&f.good, &f.warn, &f.bad, &f.claude, &f.codex];
            assert!(all.iter().chain(more.iter()).all(|c| c.is_some()) && f.gradient.is_some(), "{key}");
        }
        assert_eq!(set.find("Tokyo Night").unwrap().key, "tokyo-night");
        assert_eq!(set.find("SOLARIZED_light").unwrap().key, "solarized-light");
        assert!(set.get("nope").unwrap_err().to_string().contains("dracula"));
        assert_eq!(set.cycle("aitop", 1).key, "catppuccin");
        assert_eq!(set.cycle("aitop", -1).key, "tokyo-night");
    }

    #[test]
    fn user_themes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mine.toml"), "name = \"Mine\"\nbase = \"dracula\"\ntext = \"#010203\"")
            .unwrap();
        std::fs::write(dir.path().join("nord.toml"), "text = \"#040506\"").unwrap();
        std::fs::write(dir.path().join("broken.toml"), "text = \"not a colour\"").unwrap();
        std::fs::write(dir.path().join("orphan.toml"), "base = \"missing\"").unwrap();
        let set = ThemeSet::load(dir.path());
        let builtins = ThemeSet::builtins();
        let mine = set.get("mine").unwrap();
        assert_eq!(mine.palette.text, Color::Rgb(1, 2, 3));
        assert_eq!(mine.palette.background, builtins.get("dracula").unwrap().palette.background);
        // a user file replaces the built-in of the same name, building on it
        let nord = set.get("nord").unwrap();
        assert_eq!(nord.palette.text, Color::Rgb(4, 5, 6));
        assert_eq!(nord.palette.background, builtins.get("nord").unwrap().palette.background);
        assert_eq!(set.themes.len(), BUILTIN.len() + 1);
        assert_eq!(set.warnings.len(), 2, "{:?}", set.warnings);
    }
}
