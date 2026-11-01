//! Interactive dashboard.

mod draw;

use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Local;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::TableState;

use crate::aggregate::{summarize, Range, Row, Sort, Summary};
use crate::collector::{Command, Shared};
use crate::event::Tool;
use crate::sources::codex::RateLimits;
use crate::sources::SourceStatus;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Projects,
    Models,
    Sessions,
}

impl View {
    pub const ALL: [View; 3] = [View::Projects, View::Models, View::Sessions];

    fn next(self) -> View {
        match self {
            View::Projects => View::Models,
            View::Models => View::Sessions,
            View::Sessions => View::Projects,
        }
    }

    fn prev(self) -> View {
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

pub struct Options {
    pub range: Range,
    pub active_window_ms: i64,
    pub hide_missing: bool,
    pub config_path: PathBuf,
}

/// Copy of the collector state needed for one frame.
#[derive(Default)]
pub struct Snapshot {
    pub status: Vec<SourceStatus>,
    pub limits: Option<RateLimits>,
    pub loaded: bool,
    pub pricing_sources: Vec<String>,
    pub history_path: Option<PathBuf>,
    pub history_error: Option<String>,
    pub events: usize,
    pub last_scan: Option<Duration>,
}

pub struct App {
    data: Shared,
    tx: Sender<Command>,
    opts: Options,
    pub range: Range,
    pub sort: Sort,
    pub focus: Tool,
    pub views: [View; 2],
    pub tables: [TableState; 2],
    pub zoom: bool,
    pub help: bool,
    pub summary: Option<Summary>,
    summary_key: Option<(u64, Range, Sort, i64)>,
    pub snap: Snapshot,
    /// Click targets recorded while drawing.
    pub hits: Vec<(Rect, Hit)>,
    pub flash: Option<(String, Instant)>,
    quit: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Hit {
    Panel(Tool),
    Tab(Tool, View),
    Range(Range),
}

impl App {
    fn new(data: Shared, tx: Sender<Command>, opts: Options) -> App {
        App {
            data,
            tx,
            range: opts.range,
            opts,
            sort: Sort::Cost,
            focus: Tool::Claude,
            views: [View::Projects; 2],
            tables: Default::default(),
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

    fn refresh(&mut self) {
        let now = Local::now();
        let d = self.data.read().unwrap();
        // Recompute at most once a minute when nothing changed, so "active"
        // and the day boundary stay current.
        let key = (d.version, self.range, self.sort, now.timestamp() / 60);
        if self.summary_key != Some(key) {
            self.summary = Some(summarize(&d.events, self.range, self.sort, now, self.opts.active_window_ms));
            self.summary_key = Some(key);
        }
        self.snap = Snapshot {
            status: d.status.clone(),
            limits: d.codex_limits.clone(),
            loaded: d.loaded,
            pricing_sources: d.pricing_sources.clone(),
            history_path: d.history_path.clone(),
            history_error: d.history_error.clone(),
            events: d.events.len(),
            last_scan: d.last_scan,
        };
        drop(d);
        if !self.visible().contains(&self.focus) {
            if let Some(&t) = self.visible().first() {
                self.focus = t;
            }
        }
        for t in Tool::ALL {
            let n = self.rows(t).len();
            let st = &mut self.tables[t.index()];
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
        let Some(s) = &self.summary else { return &[] };
        let ts = s.tool(t);
        match self.views[t.index()] {
            View::Projects => &ts.projects,
            View::Models => &ts.models,
            View::Sessions => &ts.session_rows,
        }
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.opts.config_path
    }

    fn move_focus(&mut self, delta: isize) {
        let v = self.visible();
        if let Some(pos) = v.iter().position(|&t| t == self.focus) {
            let n = v.len() as isize;
            self.focus = v[((pos as isize + delta).rem_euclid(n)) as usize];
        }
    }

    fn move_selection(&mut self, tool: Tool, delta: isize) {
        let n = self.rows(tool).len();
        if n == 0 {
            return;
        }
        let st = &mut self.tables[tool.index()];
        let cur = st.selected().unwrap_or(0) as isize;
        st.select(Some((cur + delta).clamp(0, n as isize - 1) as usize));
    }

    fn set_view(&mut self, tool: Tool, view: View) {
        self.views[tool.index()] = view;
        self.tables[tool.index()] = TableState::default().with_selected(Some(0));
    }

    fn on_key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.help {
            if matches!(k.code, KeyCode::Esc | KeyCode::Char('?' | 'q' | 'h') | KeyCode::F(1) | KeyCode::Enter) {
                self.help = false;
            } else if ctrl && k.code == KeyCode::Char('c') {
                self.quit = true;
            }
            return;
        }
        let f = self.focus;
        match k.code {
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => self.zoom = false,
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(f, 1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(f, -1),
            KeyCode::PageDown => self.move_selection(f, 10),
            KeyCode::PageUp => self.move_selection(f, -10),
            KeyCode::Home | KeyCode::Char('g') => self.move_selection(f, isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(f, isize::MAX / 2),
            KeyCode::Right | KeyCode::Char('l') => self.set_view(f, self.views[f.index()].next()),
            KeyCode::Left | KeyCode::Char('h') => self.set_view(f, self.views[f.index()].prev()),
            KeyCode::Char('p') => self.set_view(f, View::Projects),
            KeyCode::Char('m') => self.set_view(f, View::Models),
            KeyCode::Char('S') => self.set_view(f, View::Sessions),
            KeyCode::Char('1') => self.range = Range::Today,
            KeyCode::Char('2') => self.range = Range::Week,
            KeyCode::Char('3') => self.range = Range::Month,
            KeyCode::Char('4') => self.range = Range::All,
            KeyCode::Char('[') => {
                self.range = Range::ALL[(Range::ALL.iter().position(|&r| r == self.range).unwrap() + 3) % 4]
            }
            KeyCode::Char(']') => {
                self.range = Range::ALL[(Range::ALL.iter().position(|&r| r == self.range).unwrap() + 1) % 4]
            }
            KeyCode::Char('s') => self.sort = self.sort.next(),
            KeyCode::Char('z') | KeyCode::Enter => self.zoom = !self.zoom,
            KeyCode::Char('r') => {
                let _ = self.tx.send(Command::Rescan);
                self.flash = Some(("rescanning…".into(), Instant::now()));
            }
            _ => {}
        }
    }

    fn hit(&self, col: u16, row: u16) -> Option<Hit> {
        let p = Position::new(col, row);
        // Later (more specific) targets win.
        self.hits.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, h)| *h)
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let hit = self.hit(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d = if m.kind == MouseEventKind::ScrollDown { 1 } else { -1 };
                if let Some(Hit::Panel(t) | Hit::Tab(t, _)) = hit {
                    self.focus = t;
                    self.move_selection(t, d);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => match hit {
                Some(Hit::Tab(t, v)) => {
                    self.focus = t;
                    self.set_view(t, v);
                }
                Some(Hit::Panel(t)) => self.focus = t,
                Some(Hit::Range(r)) => self.range = r,
                None => {}
            },
            _ => {}
        }
    }
}

pub fn run(data: Shared, tx: Sender<Command>, opts: Options) -> Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableMouseCapture);
    let mut app = App::new(data, tx, opts);
    let res = (|| -> Result<()> {
        while !app.quit {
            app.refresh();
            terminal.draw(|f| draw::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(500))? {
                // Handle everything queued before the next frame.
                loop {
                    match event::read()? {
                        TEvent::Key(k) => app.on_key(k),
                        TEvent::Mouse(m) => app.on_mouse(m),
                        _ => {}
                    }
                    if app.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}
