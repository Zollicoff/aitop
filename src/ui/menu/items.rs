//! The settings the Options screen offers: where each lives in the config
//! file, how it is shown, and how it changes.

use toml_edit::Value;

use crate::aggregate::Range;
use crate::appearance::Choice;
use crate::budget::WeekStart;
use crate::event::Tool;
use crate::format;
use crate::ui::app::App;

/// Sets a value from typed text; `Err` explains what was wrong.
pub(super) type Input = fn(&mut App, &str) -> Result<(), String>;

/// One editable setting.
pub struct Setting {
    pub section: usize,
    pub name: &'static str,
    pub help: &'static str,
    /// Where it lives in config.toml.
    pub(super) key: &'static [&'static str],
    /// The current value, as shown.
    pub show: fn(&App) -> String,
    /// ←/→: step the value.
    pub(super) step: fn(&mut App, isize),
    /// Enter: set from typed text (numbers), if supported.
    pub(super) input: Option<Input>,
    /// The value to save; `None` removes the key.
    pub(super) save: fn(&App) -> Option<Value>,
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

/// The on/off switch of one agent's panel.
macro_rules! agent {
    ($tool:expr) => {
        Setting {
            section: 2,
            name: $tool.name(),
            help: "Show this agent in the dashboard. One that is off is left out of the panels, the header totals and the activity feed; reports still include it. Agents other than Claude Code and Codex come from T3 Code and appear once they have usage.",
            key: &["agents", $tool.key()],
            show: |a| on_off(a.shown($tool)),
            step: |a, _| {
                let on = !a.shown($tool);
                a.opts.cfg.agents.set($tool, on);
                a.invalidate();
            },
            input: None,
            // Shown is the default, so only "off" is written down.
            save: |a| (!a.shown($tool)).then(|| false.into()),
        }
    };
}

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
        name: "Layout",
        help: "list: the agents in a list, with the selected one in full beside it. stacked: one full-width panel per agent. auto: stacked for one or two agents, the list for more. `v` switches while aitop runs.",
        key: &["layout"],
        show: |a| a.cfg().layout.name().into(),
        step: |a, d| a.opts.cfg.layout = a.cfg().layout.cycle(d),
        input: None,
        save: |a| Some(a.cfg().layout.name().into()),
    },
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
        help: "accent: each agent's colour; gradient: the theme's low → high colours by height.",
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
    // Agents
    agent!(Tool::Claude),
    agent!(Tool::Codex),
    agent!(Tool::Grok),
    agent!(Tool::Cursor),
    agent!(Tool::OpenCode),
    agent!(Tool::Antigravity),
    agent!(Tool::Pi),
    agent!(Tool::Muse),
    agent!(Tool::Other),
    // Budgets
    Setting {
        section: 3,
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
        section: 3,
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
        section: 3,
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
        section: 3,
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
        section: 3,
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
        section: 3,
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
