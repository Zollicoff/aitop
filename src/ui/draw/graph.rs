//! Filled graphs in btop's three styles: braille dots, block eighths, or
//! TTY shade characters.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

use crate::appearance::{GraphColor, GraphSymbol};
use crate::theme;

/// Block eighths, from one eighth up to a full cell.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// TTY shades, from a quarter to a full cell.
const SHADES: [char; 4] = ['░', '▒', '▓', '█'];
/// Braille dot bits, bottom row first, for the left and right dot columns.
const BRAILLE_LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
const BRAILLE_RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];

pub(super) struct Graph<'a> {
    /// Oldest first. The newest values that fit are drawn, stretched to the width.
    pub values: &'a [f64],
    pub symbol: GraphSymbol,
    pub color: GraphColor,
    pub accent: Color,
}

impl Graph<'_> {
    /// Height steps per cell for the symbol set.
    fn levels(&self) -> usize {
        match self.symbol {
            GraphSymbol::Braille => 4,
            GraphSymbol::Block => 8,
            GraphSymbol::Tty => 4,
        }
    }

    /// One height (in steps) per sample column: two per cell for braille.
    fn heights(&self, cells: usize, rows: usize) -> Vec<usize> {
        let samples = if self.symbol == GraphSymbol::Braille { cells * 2 } else { cells };
        let first = self.values.len().saturating_sub(samples);
        let vals = &self.values[first..];
        if vals.is_empty() || samples == 0 {
            return Vec::new();
        }
        let max = vals.iter().copied().fold(0.0, f64::max);
        let top = rows * self.levels();
        (0..samples)
            .map(|i| {
                let v = vals[i * vals.len() / samples];
                if v <= 0.0 || max <= 0.0 {
                    0
                } else {
                    // Anything above zero stays visible.
                    ((v / max * top as f64).round() as usize).clamp(1, top)
                }
            })
            .collect()
    }

    fn style(&self, row_from_bottom: usize, rows: usize) -> Style {
        let c = match self.color {
            GraphColor::Accent => self.accent,
            GraphColor::Gradient => theme::gradient((row_from_bottom as f64 + 0.5) / rows as f64),
        };
        Style::new().fg(c)
    }
}

impl Widget for Graph<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (cells, rows) = (area.width as usize, area.height as usize);
        if cells == 0 || rows == 0 {
            return;
        }
        let heights = self.heights(cells, rows);
        let n = self.levels();
        // Steps of `h` that fall in the cell `row` (counted from the bottom).
        let fill = |h: usize, row: usize| h.saturating_sub(row * n).min(n);
        for row in 0..rows {
            let y = area.bottom() - 1 - row as u16;
            let style = self.style(row, rows);
            for col in 0..cells {
                let ch = match self.symbol {
                    GraphSymbol::Block => match fill(heights.get(col).copied().unwrap_or(0), row) {
                        0 => continue,
                        k => BLOCKS[k - 1],
                    },
                    GraphSymbol::Tty => match fill(heights.get(col).copied().unwrap_or(0), row) {
                        0 => continue,
                        k => SHADES[k - 1],
                    },
                    GraphSymbol::Braille => {
                        let l = fill(heights.get(col * 2).copied().unwrap_or(0), row);
                        let r = fill(heights.get(col * 2 + 1).copied().unwrap_or(0), row);
                        let bits: u32 = BRAILLE_LEFT[..l].iter().chain(&BRAILLE_RIGHT[..r]).sum();
                        if bits == 0 {
                            continue;
                        }
                        char::from_u32(0x2800 + bits).unwrap_or(' ')
                    }
                };
                buf[(area.x + col as u16, y)].set_char(ch).set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(values: &[f64], symbol: GraphSymbol, w: u16, h: u16) -> Vec<String> {
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        Graph { values, symbol, color: GraphColor::Accent, accent: Color::Reset }.render(buf.area, &mut buf);
        buf.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect()).collect()
    }

    #[test]
    fn block_graph() {
        // max fills both rows; half is one full row; a tiny value still shows.
        assert_eq!(draw(&[4.0, 2.0, 0.01, 0.0], GraphSymbol::Block, 4, 2), ["█   ", "██▁ "]);
    }

    #[test]
    fn tty_graph() {
        assert_eq!(draw(&[4.0, 1.0], GraphSymbol::Tty, 2, 1), ["█░"]);
    }

    #[test]
    fn braille_graph() {
        // Two samples per cell: full left column + half right column.
        let rows = draw(&[4.0, 2.0], GraphSymbol::Braille, 1, 1);
        assert_eq!(rows, ["\u{28e7}"]); // left 4 dots (0x47) + right bottom 2 dots (0xA0)
    }

    #[test]
    fn newest_values_win_when_too_many() {
        assert_eq!(draw(&[9.0, 9.0, 0.0, 1.0], GraphSymbol::Block, 2, 1), [" █"]);
    }
}
