//! Saving individual settings back to `config.toml`, keeping the user's
//! comments and layout (the options screen uses this).

use std::path::Path;

use anyhow::{bail, Context, Result};
use toml_edit::{DocumentMut, Item, Table, Value};

/// Set `key` (a path like `["budget", "daily"]`) to `value`, or remove it
/// when `value` is `None`. Creates the file and any tables as needed.
pub fn save(path: &Path, key: &[&str], value: Option<Value>) -> Result<()> {
    let Some((last, tables)) = key.split_last() else { bail!("empty key") };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut doc: DocumentMut =
        text.parse().with_context(|| format!("{} has errors; fix it before changing options", path.display()))?;

    let mut table: &mut Table = doc.as_table_mut();
    for name in tables {
        let item = table.entry(name).or_insert_with(|| Item::Table(Table::new()));
        let Some(t) = item.as_table_mut() else { bail!("`{name}` in {} is not a table", path.display()) };
        table = t;
    }
    match value {
        Some(mut v) => match table.get_mut(last) {
            // Replace the value through the existing entry, so comments
            // attached to the key (above it) and after the value survive.
            Some(item) => {
                if let Some(old) = item.as_value() {
                    *v.decor_mut() = old.decor().clone();
                }
                *item = Item::Value(v);
            }
            None => {
                table.insert(last, Item::Value(v));
            }
        },
        None => {
            table.remove(last);
        }
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write a temporary file next to the config and rename it into place,
    // so a crash never leaves half a file.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_in_place_keeping_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/config.toml");
        // A missing file is created.
        save(&p, &["theme"], Some("dracula".into())).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "theme = \"dracula\"");

        std::fs::write(&p, "# my settings\ntheme = \"nord\"  # favourite\n\n[budget]\n# per day\ndaily = 5.0\n")
            .unwrap();
        save(&p, &["theme"], Some("gruvbox".into())).unwrap();
        save(&p, &["budget", "daily"], Some(12.5.into())).unwrap();
        save(&p, &["budget", "monthly"], Some(300.0.into())).unwrap();
        save(&p, &["graph_symbol"], Some("braille".into())).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("# my settings\n"), "{text}");
        assert!(text.contains("theme = \"gruvbox\"  # favourite"), "{text}");
        assert!(text.contains("# per day\ndaily = 12.5"), "{text}");
        assert!(text.contains("monthly = 300.0"), "{text}");
        // The result is still a valid config.
        let cfg: crate::config::Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.budget.daily, Some(12.5));
        assert_eq!(cfg.graph_symbol, crate::appearance::GraphSymbol::Braille);

        save(&p, &["budget", "daily"], None).unwrap();
        assert!(!std::fs::read_to_string(&p).unwrap().contains("daily"));
    }

    #[test]
    fn refuses_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \n").unwrap();
        assert!(save(&p, &["theme"], Some("x".into())).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "theme = \n", "left untouched");
        std::fs::write(&p, "budget = 3\n").unwrap();
        assert!(save(&p, &["budget", "daily"], Some(1.0.into())).is_err());
    }
}
