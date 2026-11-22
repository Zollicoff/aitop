//! btop-style menus: Esc opens the main menu (Options · Help · Quit), and
//! the Options screen edits settings live, saving each change to the
//! config file (comments and layout are kept).

mod items;

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::App;
use crate::collector::Command;
use crate::{settings, theme};

#[cfg(test)]
use items::SETTINGS;
pub use items::{section, Setting};

pub enum Menu {
    Main { selected: usize },
    Options(OptionsState),
}

pub const MAIN_ITEMS: [&str; 3] = ["Options", "Help", "Quit"];

#[derive(Default)]
pub struct OptionsState {
    pub section: usize,
    pub selected: usize,
    /// Text being typed for the selected setting.
    pub editing: Option<String>,
    pub error: Option<String>,
}

pub const SECTIONS: [&str; 4] = ["General", "Display", "Agents", "Budgets"];

impl App {
    pub(super) fn open_menu(&mut self) {
        self.menu = Some(Menu::Main { selected: 0 });
    }

    pub(super) fn open_options(&mut self) {
        self.menu = Some(Menu::Options(OptionsState::default()));
    }

    /// Re-read the look settings into the theme.
    pub(super) fn apply_look(&mut self) {
        theme::set_look(theme::Look::from_config(self.cfg()));
    }

    pub(super) fn set_theme(&mut self, delta: isize) {
        let t = self.opts.themes.cycle(&self.theme, delta).clone();
        theme::set(t.palette);
        self.theme = t.key;
    }

    fn set_refresh(&mut self, secs: f64) {
        self.opts.cfg.refresh_seconds = secs;
        let _ = self.tx.send(Command::SetInterval(Duration::from_secs_f64(secs)));
    }

    /// Keys while a menu is open.
    pub(super) fn on_menu_key(&mut self, k: KeyEvent) {
        match &mut self.menu {
            Some(Menu::Main { selected }) => match k.code {
                KeyCode::Esc | KeyCode::Char('m') => self.menu = None,
                KeyCode::Up | KeyCode::Char('k') => *selected = (*selected + MAIN_ITEMS.len() - 1) % MAIN_ITEMS.len(),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => *selected = (*selected + 1) % MAIN_ITEMS.len(),
                KeyCode::Char('o') => self.open_options(),
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Char('?') | KeyCode::F(1) => {
                    self.menu = None;
                    self.help = true;
                }
                KeyCode::Enter => match *selected {
                    0 => self.open_options(),
                    1 => {
                        self.menu = None;
                        self.help = true;
                    }
                    _ => self.quit = true,
                },
                _ => {}
            },
            Some(Menu::Options(_)) => self.on_options_key(k),
            None => {}
        }
    }

    fn on_options_key(&mut self, k: KeyEvent) {
        let Some(Menu::Options(st)) = &mut self.menu else { return };
        let items = section(st.section);
        let Some(&setting) = items.get(st.selected) else { return };

        if let Some(buf) = &mut st.editing {
            match k.code {
                KeyCode::Esc => st.editing = None,
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => buf.push(c),
                KeyCode::Enter => {
                    let text = st.editing.take().unwrap_or_default();
                    if let Some(input) = setting.input {
                        match input(self, &text) {
                            Ok(()) => self.save_setting(setting),
                            Err(e) => self.options_error(Some(e)),
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match k.code {
            KeyCode::Esc => self.menu = Some(Menu::Main { selected: 0 }),
            KeyCode::Char('q') => self.menu = None,
            KeyCode::Up | KeyCode::Char('k') => st.selected = (st.selected + items.len() - 1) % items.len(),
            KeyCode::Down | KeyCode::Char('j') => st.selected = (st.selected + 1) % items.len(),
            KeyCode::Tab | KeyCode::BackTab => {
                let d = if k.code == KeyCode::Tab { 1 } else { SECTIONS.len() - 1 };
                st.section = (st.section + d) % SECTIONS.len();
                st.selected = 0;
                st.error = None;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l') => {
                let d = if matches!(k.code, KeyCode::Left | KeyCode::Char('h')) { -1 } else { 1 };
                (setting.step)(self, d);
                self.save_setting(setting);
            }
            KeyCode::Enter if setting.editable() => {
                st.editing = Some(String::new());
                st.error = None;
            }
            KeyCode::Enter => {
                (setting.step)(self, 1);
                self.save_setting(setting);
            }
            _ => {}
        }
    }

    fn options_error(&mut self, e: Option<String>) {
        if let Some(Menu::Options(st)) = &mut self.menu {
            st.error = e;
        }
    }

    fn save_setting(&mut self, s: &Setting) {
        let result = settings::save(self.config_path(), s.key, (s.save)(self)).map_err(|e| format!("not saved: {e:#}"));
        self.options_error(result.err());
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::appearance::{BoxStyle, ChartMetric, Choice, GraphColor, GraphSymbol, Layout, TrueColor};
    use crate::event::Tool;
    use crate::ui::tests::{app, render, text};

    fn key(a: &mut App, c: KeyCode) {
        a.on_key(KeyEvent::new(c, KeyModifiers::NONE));
    }

    #[test]
    fn esc_opens_menu_and_options_save() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = app("aitop");
        a.opts.config_path = dir.path().join("config.toml");
        std::fs::write(&a.opts.config_path, "# keep me\nrefresh_seconds = 2.0\n").unwrap();

        key(&mut a, KeyCode::Esc);
        assert!(matches!(a.menu, Some(Menu::Main { .. })));
        assert!(text(&render(&mut a, 120, 36)).contains("Options"));
        key(&mut a, KeyCode::Enter); // Options
        assert!(matches!(a.menu, Some(Menu::Options(_))));

        // General: theme → next theme, saved.
        key(&mut a, KeyCode::Right);
        assert_eq!(a.theme, "catppuccin");
        // Display section: the list layout, theme background off, another box style.
        key(&mut a, KeyCode::Tab);
        key(&mut a, KeyCode::Right);
        assert_eq!(a.cfg().layout, Layout::List);
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Right);
        assert!(!a.cfg().theme_background);
        assert!(!theme::look().theme_background, "applied live");
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Right);
        assert_eq!(a.cfg().box_style, BoxStyle::Plain);
        // Agents: turn Pi off (the seventh in the list).
        key(&mut a, KeyCode::Tab);
        for _ in 0..6 {
            key(&mut a, KeyCode::Down);
        }
        let out = text(&render(&mut a, 120, 40));
        assert!(out.contains("Agents") && out.contains("Muse Code") && out.contains("Other agents"), "{out}");
        key(&mut a, KeyCode::Right);
        assert!(!a.shown(Tool::Pi) && a.shown(Tool::Grok));
        // Budgets: type a daily budget.
        key(&mut a, KeyCode::Tab);
        key(&mut a, KeyCode::Enter);
        for c in "12.5".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.cfg().budget.daily, Some(12.5));
        let out = text(&render(&mut a, 120, 40));
        assert!(out.contains("Daily budget") && out.contains("$12.50"), "{out}");

        let saved = std::fs::read_to_string(&a.opts.config_path).unwrap();
        assert!(saved.starts_with("# keep me"), "{saved}");
        let cfg: crate::config::Config = toml::from_str(&saved).unwrap();
        assert_eq!(cfg.theme.as_deref(), Some("catppuccin"));
        assert!(!cfg.theme_background);
        assert_eq!(cfg.box_style, BoxStyle::Plain);
        assert_eq!(cfg.budget.daily, Some(12.5));
        assert_eq!(cfg.layout, Layout::List);
        assert!(!cfg.agents.shown(Tool::Pi) && cfg.agents.shown(Tool::Claude));
        assert!(saved.contains("[agents]\npi = false"), "{saved}");

        // A bad number is reported, not saved.
        key(&mut a, KeyCode::Enter);
        for c in "abc".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        key(&mut a, KeyCode::Enter);
        let Some(Menu::Options(st)) = &a.menu else { panic!() };
        assert!(st.error.as_deref().unwrap().contains("not a number"));
        assert_eq!(a.cfg().budget.daily, Some(12.5));

        // Esc goes back to the main menu, then closes it.
        key(&mut a, KeyCode::Esc);
        assert!(matches!(a.menu, Some(Menu::Main { .. })));
        key(&mut a, KeyCode::Esc);
        assert!(a.menu.is_none());
        theme::set_look(theme::Look::default());
    }

    #[test]
    fn quit_from_menu() {
        let mut a = app("aitop");
        key(&mut a, KeyCode::Esc);
        key(&mut a, KeyCode::Up); // wraps to Quit
        key(&mut a, KeyCode::Enter);
        assert!(a.quit);
    }

    #[test]
    fn every_setting_round_trips_through_the_config() {
        let mut a = app("aitop");
        for s in SETTINGS {
            (s.step)(&mut a, 1);
            let v = (s.save)(&a);
            let mut doc = toml_edit::DocumentMut::new();
            match s.key {
                [k] => doc[*k] = toml_edit::Item::Value(v.unwrap()),
                [t, k] => {
                    if let Some(v) = v {
                        doc[*t] = toml_edit::table();
                        doc[*t][*k] = toml_edit::Item::Value(v);
                    }
                }
                _ => unreachable!(),
            }
            let parsed: Result<crate::config::Config, _> = toml::from_str(&doc.to_string());
            assert!(parsed.is_ok(), "{}: {:?}", s.name, parsed.err());
        }
        assert!(TrueColor::ALL.len() == 3 && GraphSymbol::ALL.len() == 3 && GraphColor::ALL.len() == 2);
        assert_eq!(ChartMetric::ALL.len(), 2);
        theme::set_look(theme::Look::default());
    }
}
