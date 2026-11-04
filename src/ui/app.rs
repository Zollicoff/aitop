//! Dashboard state.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Local;
use ratatui::layout::Rect;
use ratatui::widgets::TableState;

use super::Options;
use crate::aggregate::{summarize, Range, Row, Sort, Summary, ToolSummary};
use crate::collector::{Command, Shared};
use crate::event::Tool;
use crate::limits::RateLimits;
use crate::sources::SourceStatus;
use crate::theme;

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

/// Per-tool UI state.
#[derive(Default)]
pub struct PanelState {
    pub view: View,
    pub table: TableState,
}

pub struct App {
    data: Shared,
    pub(super) tx: Sender<Command>,
    opts: Options,
    pub range: Range,
    pub sort: Sort,
    pub focus: Tool,
    /// Indexed by `Tool::index()`.
    pub panels: Vec<PanelState>,
    pub zoom: bool,
    pub help: bool,
    /// Shared so drawing code can hold it while mutating the rest of the app.
    pub summary: Option<Arc<Summary>>,
    summary_key: Option<(u64, Range, Sort, i64)>,
    pub snap: Snapshot,
    /// Click targets recorded while drawing.
    pub hits: Vec<(Rect, Hit)>,
    pub flash: Option<(String, Instant)>,
    /// Key of the active theme.
    pub theme: String,
    pub(super) quit: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Hit {
    Panel(Tool),
    Tab(Tool, View),
    Range(Range),
}

/// The rows a panel shows in `view`.
pub fn rows_for(ts: &ToolSummary, view: View) -> &[Row] {
    match view {
        View::Projects => &ts.projects,
        View::Models => &ts.models,
        View::Sessions => &ts.session_rows,
    }
}

impl App {
    pub(super) fn new(data: Shared, tx: Sender<Command>, opts: Options) -> App {
        App {
            data,
            tx,
            range: opts.range,
            theme: opts.theme.clone(),
            opts,
            sort: Sort::Cost,
            focus: Tool::ALL[0],
            panels: Tool::ALL.iter().map(|_| PanelState::default()).collect(),
            zoom: false,
            help: false,
            summary: None,
            summary_key: None,
            snap: Snapshot::default(),
            hits: Vec::new(),
            flash: None,
            quit: false,
        }
    }

    /// Pull the latest data from the collector before drawing a frame.
    pub(super) fn refresh(&mut self) {
        self.refresh_data();
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
        let key = (d.version, self.range, self.sort, now.timestamp() / 60);
        if self.summary_key != Some(key) {
            self.summary = Some(Arc::new(summarize(&d.events, self.range, self.sort, now, self.opts.active_window_ms)));
            self.summary_key = Some(key);
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
            .filter(|&t| !self.opts.hide_missing || self.detected(t) || self.has_data(t))
            .collect();
        if shown.is_empty() {
            Tool::ALL.to_vec()
        } else {
            shown
        }
    }

    pub fn rows(&self, t: Tool) -> &[Row] {
        match &self.summary {
            Some(s) => rows_for(s.tool(t), self.panels[t.index()].view),
            None => &[],
        }
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
        self.flash = Some((msg.into(), Instant::now()));
    }

    /// Switch to the theme `delta` steps away in the list.
    pub(super) fn cycle_theme(&mut self, delta: isize) {
        let t = self.opts.themes.cycle(&self.theme, delta).clone();
        theme::set(t.palette);
        self.flash(format!("theme: {} · set theme = \"{}\" in the config to keep it", t.name, t.key));
        self.theme = t.key;
    }

    pub fn theme_name(&self) -> &str {
        self.opts.themes.find(&self.theme).map(|t| t.name.as_str()).unwrap_or(&self.theme)
    }

    pub fn theme_warnings(&self) -> &[String] {
        &self.opts.themes.warnings
    }

    pub(super) fn set_view(&mut self, tool: Tool, view: View) {
        self.panels[tool.index()] = PanelState { view, table: TableState::default().with_selected(Some(0)) };
    }
}
