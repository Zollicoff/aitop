//! Dashboard state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Local;
use ratatui::layout::Rect;
use ratatui::widgets::TableState;

use super::menu::Menu;
use super::Options;
use crate::aggregate::{summarize, Filter, Range, Row, Sort, Summary, ToolSummary};
use crate::budget::{self, Budget, Level, Period};
use crate::collector::{Command, Shared};
use crate::config::Config;
use crate::event::{Event, Tool};
use crate::limits::RateLimits;
use crate::sources::SourceStatus;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum View {
    #[default]
    Projects,
    Models,
    Sessions,
}

impl View {
    pub const ALL: [View; 3] = [View::Projects, View::Models, View::Sessions];

    pub(super) fn next(self) -> View {
        match self {
            View::Projects => View::Models,
            View::Models => View::Sessions,
            View::Sessions => View::Projects,
        }
    }

    pub(super) fn prev(self) -> View {
        self.next().next()
    }

    pub fn label(self) -> &'static str {
        match self {
            View::Projects => "Projects",
            View::Models => "Models",
            View::Sessions => "Sessions",
        }
    }
}

/// Copy of the collector state needed for one frame.
#[derive(Default)]
pub struct Snapshot {
    pub status: Vec<SourceStatus>,
    /// Indexed by `Tool::index()`.
    pub limits: Vec<Option<RateLimits>>,
    pub loaded: bool,
    pub pricing_sources: Vec<String>,
    pub history_path: Option<PathBuf>,
    pub history_error: Option<String>,
    pub events: usize,
    pub last_scan: Option<Duration>,
}

/// How many recent responses the activity feed keeps.
const FEED_LEN: usize = 100;

/// A message shown in the footer for a while.
pub struct Flash {
    pub text: String,
    pub until: Instant,
    /// `Warn` / `Over` colour budget alerts.
    pub level: Level,
}

/// Per-tool UI state.
#[derive(Default)]
pub struct PanelState {
    pub view: View,
    pub table: TableState,
    /// Drill-down path: each filter with the view that was showing before it.
    pub crumbs: Vec<(Filter, View)>,
    /// Table search text.
    pub query: String,
}

impl PanelState {
    pub fn filters(&self) -> Vec<Filter> {
        self.crumbs.iter().map(|(f, _)| f.clone()).collect()
    }
}

pub struct App {
    data: Shared,
    pub(super) tx: Sender<Command>,
    pub(super) opts: Options,
    /// The Esc menu or the Options screen, when open.
    pub menu: Option<Menu>,
    pub range: Range,
    pub sort: Sort,
    pub focus: Tool,
    /// Indexed by `Tool::index()`.
    pub panels: Vec<PanelState>,
    pub zoom: bool,
    pub help: bool,
    /// Shared so drawing code can hold it while mutating the rest of the app.
    pub summary: Option<Arc<Summary>>,
    summary_key: Option<(u64, Range, Sort, i64, u64)>,
    /// Bumped when any panel's drill-down filters change.
    pub(super) filters_rev: u64,
    /// Typing into the focused panel's search box.
    pub searching: bool,
    /// Row (by key) to select after the next refresh, when backing out of a drill-down.
    pub(super) reselect: Option<(Tool, String)>,
    pub snap: Snapshot,
    /// Click targets recorded while drawing.
    pub hits: Vec<(Rect, Hit)>,
    pub flash: Option<Flash>,
    /// Budgets evaluated with the current summary.
    pub budgets: Vec<Budget>,
    /// Level of each budget at the last refresh, to alert on changes.
    budget_levels: HashMap<(Option<Tool>, Period), Level>,
    /// Key of the active theme.
    pub theme: String,
    /// The activity feed is shown.
    pub show_feed: bool,
    /// Newest events first (all tools), for the activity feed.
    pub feed: Vec<Event>,
    pub(super) quit: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Hit {
    Panel(Tool),
    Tab(Tool, View),
    Range(Range),
}

/// The rows a panel shows in `view`, narrowed by its search `query`
/// (case-insensitive, matching the name, detail or key).
pub fn rows_for<'a>(ts: &'a ToolSummary, view: View, query: &str) -> Vec<&'a Row> {
    let rows = match view {
        View::Projects => &ts.projects,
        View::Models => &ts.models,
        View::Sessions => &ts.session_rows,
    };
    let q = query.trim().to_lowercase();
    rows.iter()
        .filter(|r| q.is_empty() || [&r.name, &r.detail, &r.key].iter().any(|s| s.to_lowercase().contains(&q)))
        .collect()
}

impl App {
    pub(super) fn new(data: Shared, tx: Sender<Command>, opts: Options) -> App {
        App {
            data,
            tx,
            range: opts.range,
            theme: opts.theme.clone(),
            show_feed: opts.cfg.feed,
            feed: Vec::new(),
            opts,
            sort: Sort::Cost,
            focus: Tool::ALL[0],
            panels: Tool::ALL.iter().map(|_| PanelState::default()).collect(),
            zoom: false,
            help: false,
            menu: None,
            summary: None,
            summary_key: None,
            filters_rev: 0,
            searching: false,
            reselect: None,
            snap: Snapshot::default(),
            hits: Vec::new(),
            flash: None,
            budgets: Vec::new(),
            budget_levels: HashMap::new(),
            quit: false,
        }
    }

    /// Pull the latest data from the collector before drawing a frame.
    pub(super) fn refresh(&mut self) {
        self.refresh_data();
        if let Some((tool, key)) = self.reselect.take() {
            if let Some(i) = self.rows(tool).iter().position(|r| r.key == key) {
                self.panels[tool.index()].table.select(Some(i));
            }
        }
        if !self.visible().contains(&self.focus) {
            if let Some(&t) = self.visible().first() {
                self.focus = t;
            }
        }
        self.clamp_selections();
    }

    fn refresh_data(&mut self) {
        let now = Local::now();
        let d = self.data.read().unwrap();
        // Recompute when data, range or sort change, and at least once a
        // minute so "active" and the day boundary stay current.
        let key = (d.version, self.range, self.sort, now.timestamp() / 60, self.filters_rev);
        let recomputed = self.summary_key != Some(key);
        if recomputed {
            let filters: Vec<Vec<Filter>> = self.panels.iter().map(PanelState::filters).collect();
            let s = summarize(&d.events, self.range, self.sort, now, self.active_window_ms(), &filters);
            self.summary = Some(Arc::new(s));
            self.summary_key = Some(key);
            self.budgets = budget::evaluate(&d.events, &self.opts.cfg.budget, now);
            self.feed = d.events.iter().rev().take(FEED_LEN).cloned().collect();
        }
        self.snap = Snapshot {
            status: d.status.clone(),
            limits: d.limits.clone(),
            loaded: d.loaded,
            pricing_sources: d.pricing_sources.clone(),
            history_path: d.history_path.clone(),
            history_error: d.history_error.clone(),
            events: d.events.len(),
            last_scan: d.last_scan,
        };
        drop(d);
        if recomputed {
            self.check_budget_alerts(self.snap.loaded);
        }
    }

    /// Keep each table's selection inside its (possibly shrunk) row list.
    fn clamp_selections(&mut self) {
        for t in Tool::ALL {
            let n = self.rows(t).len();
            let st = &mut self.panels[t.index()].table;
            match st.selected() {
                _ if n == 0 => st.select(None),
                None => st.select(Some(0)),
                Some(i) if i >= n => st.select(Some(n - 1)),
                _ => {}
            }
        }
    }

    pub fn has_data(&self, t: Tool) -> bool {
        self.summary.as_ref().is_some_and(|s| s.tool(t).last_ts.is_some())
    }

    pub fn detected(&self, t: Tool) -> bool {
        self.snap.status.get(t.index()).is_some_and(|s| s.detected())
    }

    /// Panels to show, in order.
    pub fn visible(&self) -> Vec<Tool> {
        let shown: Vec<Tool> = Tool::ALL
            .into_iter()
            .filter(|&t| !self.opts.cfg.hide_missing || self.detected(t) || self.has_data(t))
            .collect();
        if shown.is_empty() {
            Tool::ALL.to_vec()
        } else {
            shown
        }
    }

    pub fn rows(&self, t: Tool) -> Vec<&Row> {
        let p = &self.panels[t.index()];
        match &self.summary {
            Some(s) => rows_for(s.tool(t), p.view, &p.query),
            None => Vec::new(),
        }
    }

    pub fn cfg(&self) -> &Config {
        &self.opts.cfg
    }

    /// Recompute summaries and budgets on the next refresh.
    pub(super) fn invalidate(&mut self) {
        self.summary_key = None;
    }

    fn active_window_ms(&self) -> i64 {
        (self.opts.cfg.active_minutes.max(1) * 60_000) as i64
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.opts.config_path
    }

    pub(super) fn move_focus(&mut self, delta: isize) {
        let v = self.visible();
        if let Some(pos) = v.iter().position(|&t| t == self.focus) {
            self.focus = v[(pos as isize + delta).rem_euclid(v.len() as isize) as usize];
        }
    }

    pub(super) fn move_selection(&mut self, tool: Tool, delta: isize) {
        let n = self.rows(tool).len();
        if n == 0 {
            return;
        }
        let st = &mut self.panels[tool.index()].table;
        let cur = st.selected().unwrap_or(0) as isize;
        st.select(Some(cur.saturating_add(delta).clamp(0, n as isize - 1) as usize));
    }

    pub(super) fn flash(&mut self, msg: impl Into<String>) {
        self.flash = Some(Flash { text: msg.into(), until: Instant::now() + Duration::from_secs(3), level: Level::Ok });
    }

    /// Flash (and optionally beep) when a budget moves to a worse level.
    /// Nothing fires for the levels found at startup.
    fn check_budget_alerts(&mut self, loaded: bool) {
        let first = self.budget_levels.is_empty();
        let mut worst: Option<&Budget> = None;
        for b in &self.budgets {
            let prev = self.budget_levels.insert((b.tool, b.period), b.level);
            let worse = prev.is_some_and(|p| b.level > p) || (prev.is_none() && !first && b.level > Level::Ok);
            if worse && worst.is_none_or(|w| b.level > w.level) {
                worst = Some(b);
            }
        }
        if !loaded {
            // Levels seen while history is still loading are not news.
            self.budget_levels.clear();
            return;
        }
        let Some(b) = worst else { return };
        let what = if b.level == Level::Over { "used up" } else { "nearly used" };
        let text =
            format!("⚠ {} {what}: {} of {}", b.name(), crate::format::cost(b.spent), crate::format::cost(b.limit));
        self.flash = Some(Flash { text, until: Instant::now() + Duration::from_secs(15), level: b.level });
        if self.opts.cfg.budget.bell {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = out.write_all(b"\x07").and_then(|_| out.flush());
        }
    }

    /// Switch to the theme `delta` steps away in the list.
    pub(super) fn cycle_theme(&mut self, delta: isize) {
        self.set_theme(delta);
        self.flash(format!("theme: {} · Esc → Options to keep it", self.theme_name()));
    }

    pub fn theme_name(&self) -> &str {
        self.opts.themes.find(&self.theme).map(|t| t.name.as_str()).unwrap_or(&self.theme)
    }

    pub fn theme_warnings(&self) -> &[String] {
        &self.opts.themes.warnings
    }

    pub(super) fn set_view(&mut self, tool: Tool, view: View) {
        let p = &mut self.panels[tool.index()];
        p.view = view;
        p.table = TableState::default().with_selected(Some(0));
    }
}
