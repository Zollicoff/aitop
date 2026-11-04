//! Drill-down and search.
//!
//! Enter narrows the focused panel to the selected row (a project, model or
//! session); the stats, chart and tables then cover only that. Drilling can
//! be repeated (a project, then one of its models) and Esc / Backspace go
//! back one level. `/` filters the visible table by name as you type.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use super::app::{App, View};
use crate::aggregate::{Filter, FilterKind};
use crate::format;

impl View {
    fn filter_kind(self) -> FilterKind {
        match self {
            View::Projects => FilterKind::Project,
            View::Models => FilterKind::Model,
            View::Sessions => FilterKind::Session,
        }
    }
}

impl App {
    /// Narrow the focused panel to its selected row.
    pub(super) fn drill_in(&mut self) {
        let tool = self.focus;
        let view = self.panels[tool.index()].view;
        let kind = view.filter_kind();
        let selected = self.panels[tool.index()].table.selected();
        let Some(row) = selected.and_then(|i| self.rows(tool).get(i).map(|r| (*r).clone())) else { return };
        let p = &self.panels[tool.index()];
        if p.crumbs.iter().any(|(f, _)| f.kind == kind) {
            // Already narrowed to one of these: the table has a single row.
            return;
        }
        let label = match kind {
            FilterKind::Project => row.name.clone(),
            FilterKind::Model => format::short_model(&row.name),
            FilterKind::Session => format!("session {}", row.key.chars().take(8).collect::<String>()),
        };
        let filter = Filter { kind, key: row.key.as_str().into(), label };
        // Show what's inside: the first view not already narrowed down.
        let narrowed: Vec<FilterKind> = p.crumbs.iter().map(|(f, _)| f.kind).chain([kind]).collect();
        let next = [View::Sessions, View::Models, View::Projects]
            .into_iter()
            .find(|v| !narrowed.contains(&v.filter_kind()))
            .unwrap_or(view);
        let p = &mut self.panels[tool.index()];
        p.crumbs.push((filter, view));
        p.query.clear();
        self.searching = false;
        self.set_view(tool, next);
        self.filters_rev += 1;
    }

    /// Undo the last drill-down of the focused panel. False if there was none.
    pub(super) fn drill_out(&mut self) -> bool {
        let tool = self.focus;
        let Some((filter, view)) = self.panels[tool.index()].crumbs.pop() else { return false };
        self.set_view(tool, view);
        self.filters_rev += 1;
        // Re-select the row we came from once the wider summary is computed.
        self.reselect = Some((tool, filter.key.to_string()));
        true
    }

    pub(super) fn start_search(&mut self) {
        self.searching = true;
    }

    /// Clear the focused panel's search. False if there was none.
    pub(super) fn clear_search(&mut self) -> bool {
        self.searching = false;
        let p = &mut self.panels[self.focus.index()];
        let had = !p.query.is_empty();
        p.query.clear();
        had
    }

    /// Keys while typing into the search box.
    pub(super) fn on_search_key(&mut self, k: KeyEvent) {
        let tool = self.focus;
        match k.code {
            KeyCode::Esc => {
                self.clear_search();
            }
            KeyCode::Enter => self.searching = false,
            KeyCode::Backspace => {
                if self.panels[tool.index()].query.pop().is_none() {
                    self.searching = false;
                }
            }
            KeyCode::Down => self.move_selection(tool, 1),
            KeyCode::Up => self.move_selection(tool, -1),
            KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.panels[tool.index()].query.push(c);
            }
            _ => return,
        }
        self.panels[tool.index()].table = TableState::default().with_selected(Some(0));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::event::Tool;
    use crate::ui::app::View;
    use crate::ui::tests::{app, render, text};

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn drill_in_and_out() {
        let mut a = app("aitop");
        // Claude projects by cost: webapp ($2.50), api ($1.00). Select api and drill in.
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Enter));
        let p = &a.panels[Tool::Claude.index()];
        assert_eq!(p.crumbs.len(), 1);
        assert_eq!(p.view, View::Sessions);
        a.refresh();
        let c = a.summary.as_ref().unwrap().tool(Tool::Claude).clone();
        assert_eq!(c.cost, 1.0, "only api");
        let out = text(&render(&mut a, 120, 36));
        assert!(out.contains("Claude Code › api"), "{out}");
        // Back out with Esc: unfiltered again, api still selected.
        a.on_key(key(KeyCode::Esc));
        a.refresh();
        assert_eq!(a.summary.as_ref().unwrap().tool(Tool::Claude).cost, 3.5);
        let p = &a.panels[Tool::Claude.index()];
        assert!(p.crumbs.is_empty());
        assert_eq!(p.view, View::Projects);
        assert_eq!(a.rows(Tool::Claude)[p.table.selected().unwrap()].name, "api");
    }

    #[test]
    fn search_filters_rows() {
        let mut a = app("aitop");
        a.on_key(key(KeyCode::Char('/')));
        assert!(a.searching);
        for c in "WEB".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        let names: Vec<String> = a.rows(Tool::Claude).iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, ["webapp"]);
        // Letters typed into the search box don't trigger shortcuts ('q' would quit).
        a.on_key(key(KeyCode::Char('q')));
        assert!(!a.quit);
        assert!(a.rows(Tool::Claude).is_empty());
        a.on_key(key(KeyCode::Backspace));
        a.on_key(key(KeyCode::Enter));
        assert!(!a.searching);
        let out = text(&render(&mut a, 120, 36));
        assert!(out.contains("/WEB"), "the query is shown as typed: {out}");
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.rows(Tool::Claude).len(), 2);
    }
}
