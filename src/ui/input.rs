//! Keyboard and mouse handling.

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;

use super::app::{App, Hit, View};
use crate::aggregate::Range;
use crate::collector::Command;

impl App {
    pub(super) fn on_key(&mut self, k: KeyEvent) {
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
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.menu.is_some() {
            self.on_menu_key(k);
            return;
        }
        if self.searching {
            self.on_search_key(k);
            return;
        }
        let f = self.focus;
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            // Esc backs out one step (search, drill-down, zoom), then opens the menu.
            KeyCode::Esc => {
                if !self.clear_search() && !self.drill_out() {
                    if self.zoom {
                        self.zoom = false;
                    } else {
                        self.open_menu();
                    }
                }
            }
            KeyCode::Char('o') | KeyCode::F(2) => self.open_options(),
            KeyCode::Backspace => {
                self.drill_out();
            }
            KeyCode::Enter => self.drill_in(),
            KeyCode::Char('/') => self.start_search(),
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(f, 1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(f, -1),
            KeyCode::PageDown => self.move_selection(f, 10),
            KeyCode::PageUp => self.move_selection(f, -10),
            KeyCode::Home | KeyCode::Char('g') => self.move_selection(f, isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(f, isize::MAX),
            KeyCode::Right | KeyCode::Char('l') => self.set_view(f, self.panels[f.index()].view.next()),
            KeyCode::Left | KeyCode::Char('h') => self.set_view(f, self.panels[f.index()].view.prev()),
            KeyCode::Char('p') => self.set_view(f, View::Projects),
            KeyCode::Char('m') => self.set_view(f, View::Models),
            KeyCode::Char('S') => self.set_view(f, View::Sessions),
            KeyCode::Char('1') => self.range = Range::Today,
            KeyCode::Char('2') => self.range = Range::Week,
            KeyCode::Char('3') => self.range = Range::Month,
            KeyCode::Char('4') => self.range = Range::All,
            KeyCode::Char('[') => self.range = self.range.cycle(-1),
            KeyCode::Char(']') => self.range = self.range.cycle(1),
            KeyCode::Char('s') => self.sort = self.sort.next(),
            KeyCode::Char('z') => self.zoom = !self.zoom,
            KeyCode::Char('a') => self.show_feed = !self.show_feed,
            KeyCode::Char('t') => self.cycle_theme(1),
            KeyCode::Char('T') => self.cycle_theme(-1),
            KeyCode::Char('r') => {
                let _ = self.tx.send(Command::Rescan);
                self.flash("rescanning…");
            }
            _ => {}
        }
    }

    fn hit(&self, col: u16, row: u16) -> Option<Hit> {
        let p = Position::new(col, row);
        // Later (more specific) targets win.
        self.hits.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, h)| *h)
    }

    pub(super) fn on_mouse(&mut self, m: MouseEvent) {
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
