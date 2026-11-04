//! btop-style menus: Esc opens the main menu (Options · Help · Quit), and
//! the Options screen edits settings live, saving each change to the
//! config file (comments and layout are kept).

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use toml_edit::Value;

use super::app::App;
use crate::aggregate::Range;
use crate::appearance::Choice;
use crate::budget::WeekStart;
use crate::collector::Command;
use crate::{format, settings, theme};

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

pub const SECTIONS: [&str; 3] = ["General", "Display", "Budgets"];

/// Sets a value from typed text; `Err` explains what was wrong.
type Input = fn(&mut App, &str) -> Result<(), String>;

/// One editable setting.
pub struct Setting {
    pub section: usize,
    pub name: &'static str,
    pub help: &'static str,
    /// Where it lives in config.toml.
    key: &'static [&'static str],
    /// The current value, as shown.
    pub show: fn(&App) -> String,
    /// ←/→: step the value.
    step: fn(&mut App, isize),
    /// Enter: set from typed text (numbers), if supported.
    input: Option<Input>,
    /// The value to save; `None` removes the key.
    save: fn(&App) -> Option<Value>,
}

impl Setting {
    pub fn editable(&self) -> bool {
        self.input.is_some()
    }
}

fn on_off(b: bool) -> String {
    if b { "on" } else { "off" }.into()
}

fn parse_number(text: &str, min: f64, max: f64) -> Result<Option<f64>, String> {
    let t = text.trim().trim_start_matches('$');
    if t.is_empty() || t == "0" || t.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let v: f64 = t.parse().map_err(|_| format!("{text:?} is not a number"))?;
    if !(min..=max).contains(&v) {
        return Err(format!("must be between {min} and {max}"));
    }
    Ok(Some(v))
}

/// Round to cents so stepping doesn't accumulate float noise.
fn cents(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn budget_step(v: Option<f64>, delta: isize, unit: f64) -> Option<f64> {
    let next = cents(v.unwrap_or(0.0) + delta as f64 * unit);
    (next > 0.0).then_some(next)
}

fn show_budget(v: Option<f64>) -> String {
    v.map(format::cost).unwrap_or_else(|| "off".into())
}

const CLOCKS: [&str; 5] = ["%H:%M:%S", "%H:%M", "%I:%M:%S %p", "%I:%M %p", ""];

pub static SETTINGS: &[Setting] = &[
    // General
    Setting {
        section: 0,
        name: "Color theme",
        help: "Built-in themes plus any in ~/.config/aitop/themes. `aitop themes show NAME` prints one to start your own.",
        key: &["theme"],
        show: |a| a.theme_name().to_string(),
        step: |a, d| a.set_theme(d),
        input: None,
        save: |a| Some(a.theme.clone().into()),
    },
    Setting {
        section: 0,
        name: "Default range",
        help: "The range shown when aitop starts.",
        key: &["default_range"],
        show: |a| Range::parse(&a.cfg().default_range).unwrap_or(Range::Today).short().into(),
        step: |a, d| {
            let r = Range::parse(&a.cfg().default_range).unwrap_or(Range::Today).cycle(d);
            a.opts.cfg.default_range = r.short().into();
        },
        input: None,
        save: |a| Some(a.cfg().default_range.clone().into()),
    },
    Setting {
        section: 0,
        name: "Refresh interval",
        help: "Seconds between log rescans. Scans only read what is new, so short intervals are cheap.",
        key: &["refresh_seconds"],
        show: |a| format!("{} s", a.cfg().refresh_seconds),
        step: |a, d| {
            let v = (a.cfg().refresh_seconds + d as f64 * 0.5).clamp(0.5, 60.0);
            a.set_refresh(v);
        },
        input: Some(|a, t| {
            let v = parse_number(t, 0.2, 3600.0)?.ok_or("enter a number of seconds")?;
            a.set_refresh(v);
            Ok(())
        }),
        save: |a| Some(a.cfg().refresh_seconds.into()),
    },
    Setting {
        section: 0,
        name: "Active window",
        help: "A session counts as active if it produced output within this many minutes.",
        key: &["active_minutes"],
        show: |a| format!("{} min", a.cfg().active_minutes),
        step: |a, d| {
            a.opts.cfg.active_minutes = (a.cfg().active_minutes as i64 + d as i64).clamp(1, 240) as u64;
            a.invalidate();
        },
        input: None,
        save: |a| Some((a.cfg().active_minutes as i64).into()),
    },
    Setting {
        section: 0,
        name: "Hide missing tools",
        help: "Hide the panel of a tool whose logs aren't on this machine.",
        key: &["hide_missing"],
        show: |a| on_off(a.cfg().hide_missing),
        step: |a, _| a.opts.cfg.hide_missing = !a.cfg().hide_missing,
        input: None,
        save: |a| Some(a.cfg().hide_missing.into()),
    },
    Setting {
        section: 0,
        name: "Activity feed",
        help: "Show the live activity feed (also toggled with `a`; this also sets whether it opens at start).",
        key: &["feed"],
        show: |a| on_off(a.cfg().feed),
        step: |a, _| {
            a.opts.cfg.feed = !a.cfg().feed;
            a.show_feed = a.cfg().feed;
        },
        input: None,
        save: |a| Some(a.cfg().feed.into()),
    },
    // Display
    Setting {
        section: 1,
        name: "Theme background",
        help: "Paint the theme's background colour. Off keeps the terminal's own background, so a transparent or translucent terminal shows through.",
        key: &["theme_background"],
        show: |a| on_off(a.cfg().theme_background),
        step: |a, _| {
            a.opts.cfg.theme_background = !a.cfg().theme_background;
            a.apply_look();
        },
        input: None,
        save: |a| Some(a.cfg().theme_background.into()),
    },
    Setting {
        section: 1,
        name: "Truecolor",
        help: "24-bit colour. auto: detect from the terminal (COLORTERM); off: the nearest of 256 colours.",
        key: &["truecolor"],
        show: |a| a.cfg().truecolor.name().into(),
        step: |a, d| {
            a.opts.cfg.truecolor = a.cfg().truecolor.cycle(d);
            a.apply_look();
        },
        input: None,
        save: |a| Some(a.cfg().truecolor.name().into()),
    },
    Setting {
        section: 1,
        name: "Box style",
        help: "How panel borders are drawn.",
        key: &["box_style"],
        show: |a| a.cfg().box_style.name().into(),
        step: |a, d| {
            a.opts.cfg.box_style = a.cfg().box_style.cycle(d);
            a.apply_look();
        },
        input: None,
        save: |a| Some(a.cfg().box_style.name().into()),
    },
    Setting {
        section: 1,
        name: "Graph symbol",
        help: "braille: finest detail (needs a font with braille); block: solid bars; tty: shade characters that work on a bare console.",
        key: &["graph_symbol"],
        show: |a| a.cfg().graph_symbol.name().into(),
        step: |a, d| a.opts.cfg.graph_symbol = a.cfg().graph_symbol.cycle(d),
        input: None,
        save: |a| Some(a.cfg().graph_symbol.name().into()),
    },
    Setting {
        section: 1,
        name: "Graph colour",
        help: "accent: each tool's colour; gradient: the theme's low → high colours by height.",
        key: &["graph_color"],
        show: |a| a.cfg().graph_color.name().into(),
        step: |a, d| a.opts.cfg.graph_color = a.cfg().graph_color.cycle(d),
        input: None,
        save: |a| Some(a.cfg().graph_color.name().into()),
    },
    Setting {
        section: 1,
        name: "Chart shows",
        help: "What the per-tool chart plots over time.",
        key: &["chart"],
        show: |a| a.cfg().chart.name().into(),
        step: |a, d| a.opts.cfg.chart = a.cfg().chart.cycle(d),
        input: None,
        save: |a| Some(a.cfg().chart.name().into()),
    },
    Setting {
        section: 1,
        name: "Clock",
        help: "Header clock format (strftime), or off. Any other format can be set as clock_format in the config.",
        key: &["clock_format"],
        show: |a| match a.cfg().clock_format.as_str() {
            "" => "off".into(),
            f => format!("{f}  ({})", format::clock(chrono::Local::now(), f).unwrap_or_default()),
        },
        step: |a, d| {
            let i = CLOCKS.iter().position(|c| *c == a.cfg().clock_format).map(|i| i as isize).unwrap_or(-1);
            a.opts.cfg.clock_format = CLOCKS[(i + d).rem_euclid(CLOCKS.len() as isize) as usize].into();
        },
        input: None,
        save: |a| Some(a.cfg().clock_format.clone().into()),
    },
    // Budgets
    Setting {
        section: 2,
        name: "Daily budget",
        help: "Spend limit for today across all tools, in USD. ←/→ steps by $1; enter to type an amount; 0 turns it off.",
        key: &["budget", "daily"],
        show: |a| show_budget(a.cfg().budget.daily),
        step: |a, d| {
            a.opts.cfg.budget.daily = budget_step(a.cfg().budget.daily, d, 1.0);
            a.invalidate();
        },
        input: Some(|a, t| {
            a.opts.cfg.budget.daily = parse_number(t, 0.0, 1e7)?;
            a.invalidate();
            Ok(())
        }),
        save: |a| a.cfg().budget.daily.map(Value::from),
    },
    Setting {
        section: 2,
        name: "Weekly budget",
        help: "Spend limit for this week across all tools. ←/→ steps by $5; enter to type; 0 turns it off.",
        key: &["budget", "weekly"],
        show: |a| show_budget(a.cfg().budget.weekly),
        step: |a, d| {
            a.opts.cfg.budget.weekly = budget_step(a.cfg().budget.weekly, d, 5.0);
            a.invalidate();
        },
        input: Some(|a, t| {
            a.opts.cfg.budget.weekly = parse_number(t, 0.0, 1e7)?;
            a.invalidate();
            Ok(())
        }),
        save: |a| a.cfg().budget.weekly.map(Value::from),
    },
    Setting {
        section: 2,
        name: "Monthly budget",
        help: "Spend limit for this month across all tools. ←/→ steps by $10; enter to type; 0 turns it off. Per-tool budgets go under [budget.claude] / [budget.codex] in the config.",
        key: &["budget", "monthly"],
        show: |a| show_budget(a.cfg().budget.monthly),
        step: |a, d| {
            a.opts.cfg.budget.monthly = budget_step(a.cfg().budget.monthly, d, 10.0);
            a.invalidate();
        },
        input: Some(|a, t| {
            a.opts.cfg.budget.monthly = parse_number(t, 0.0, 1e7)?;
            a.invalidate();
            Ok(())
        }),
        save: |a| a.cfg().budget.monthly.map(Value::from),
    },
    Setting {
        section: 2,
        name: "Warn at",
        help: "A budget turns amber at this share of its limit.",
        key: &["budget", "warn_at"],
        show: |a| format!("{}%", a.cfg().budget.warn_at),
        step: |a, d| {
            a.opts.cfg.budget.warn_at = (a.cfg().budget.warn_at + d as f64 * 5.0).clamp(50.0, 100.0);
            a.invalidate();
        },
        input: None,
        save: |a| Some(a.cfg().budget.warn_at.into()),
    },
    Setting {
        section: 2,
        name: "Week starts",
        help: "First day of the week for weekly budgets.",
        key: &["budget", "week_starts"],
        show: |a| match a.cfg().budget.week_starts {
            WeekStart::Monday => "monday".into(),
            WeekStart::Sunday => "sunday".into(),
        },
        step: |a, _| {
            a.opts.cfg.budget.week_starts = match a.cfg().budget.week_starts {
                WeekStart::Monday => WeekStart::Sunday,
                WeekStart::Sunday => WeekStart::Monday,
            };
            a.invalidate();
        },
        input: None,
        save: |a| {
            Some(match a.cfg().budget.week_starts {
                WeekStart::Monday => "monday".into(),
                WeekStart::Sunday => "sunday".into(),
            })
        },
    },
    Setting {
        section: 2,
        name: "Alert bell",
        help: "Ring the terminal bell when a budget becomes nearly or fully used.",
        key: &["budget", "bell"],
        show: |a| on_off(a.cfg().budget.bell),
        step: |a, _| a.opts.cfg.budget.bell = !a.cfg().budget.bell,
        input: None,
        save: |a| Some(a.cfg().budget.bell.into()),
    },
];

/// The settings in `section`, in order.
pub fn section(section: usize) -> Vec<&'static Setting> {
    SETTINGS.iter().filter(|s| s.section == section).collect()
}

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
    use crate::appearance::{BoxStyle, ChartMetric, GraphColor, GraphSymbol, TrueColor};
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
        // Display section: theme background off, box style double.
        key(&mut a, KeyCode::Tab);
        key(&mut a, KeyCode::Right);
        assert!(!a.cfg().theme_background);
        assert!(!theme::look().theme_background, "applied live");
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Right);
        assert_eq!(a.cfg().box_style, BoxStyle::Plain);
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
