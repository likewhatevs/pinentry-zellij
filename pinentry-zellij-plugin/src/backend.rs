//! ANSI escape sequence backend for ratatui.
//!
//! Zellij plugins output via `print!()` rather than writing to a terminal
//! directly. This backend captures ratatui cell draws into an internal grid
//! and converts them to an ANSI string suitable for `print!()`.

use std::fmt::Write as FmtWrite;

use arrayvec::ArrayString;
use ratatui_core::backend::{Backend, ClearType, WindowSize};
use ratatui_core::buffer::Cell;
use ratatui_core::layout::{Position, Size};
use ratatui_core::style::{Color, Modifier};

/// Max bytes for a single cell symbol. Covers all single Unicode code points
/// (max 4 bytes UTF-8) which is what ratatui produces per cell.
const SYMBOL_CAP: usize = 4;

/// A ratatui Backend that renders cells into an ANSI escape sequence string.
pub struct AnsiBackend {
    width: u16,
    height: u16,
    cells: Vec<Vec<CellState>>,
    cursor_pos: Position,
    cursor_visible: bool,
}

/// Per-cell state stored entirely on the stack (no heap allocation).
#[derive(Clone)]
struct CellState {
    symbol: ArrayString<SYMBOL_CAP>,
    fg: Option<Color>,
    bg: Option<Color>,
    modifiers: Modifier,
}

impl Default for CellState {
    fn default() -> Self {
        let mut symbol = ArrayString::new();
        symbol.push(' ');
        Self {
            symbol,
            fg: None,
            bg: None,
            modifiers: Modifier::empty(),
        }
    }
}

impl AnsiBackend {
    pub fn new(width: u16, height: u16) -> Self {
        let cells = vec![vec![CellState::default(); width as usize]; height as usize];
        Self {
            width,
            height,
            cells,
            cursor_pos: Position::ORIGIN,
            cursor_visible: false,
        }
    }

    /// Render the cell buffer into an ANSI string suitable for print!().
    pub fn to_ansi(&self) -> String {
        // Pre-allocate: ~10 bytes per cell is a reasonable estimate
        // (symbol + possible SGR escape)
        let mut out = String::with_capacity(self.width as usize * self.height as usize * 10 + 16);
        // Move cursor to top-left and clear screen
        out.push_str("\x1b[H\x1b[2J");

        for (y, row) in self.cells.iter().enumerate() {
            if y > 0 {
                out.push_str("\r\n");
            }
            for cell in row {
                let has_style =
                    cell.fg.is_some() || cell.bg.is_some() || !cell.modifiers.is_empty();

                if !has_style {
                    out.push_str(&cell.symbol);
                    continue;
                }

                out.push_str("\x1b[");
                let mut need_sep = false;

                // Modifiers — write fixed codes directly, no allocation
                if cell.modifiers.contains(Modifier::BOLD) {
                    out.push('1');
                    need_sep = true;
                }
                if cell.modifiers.contains(Modifier::DIM) {
                    if need_sep {
                        out.push(';');
                    }
                    out.push('2');
                    need_sep = true;
                }
                if cell.modifiers.contains(Modifier::ITALIC) {
                    if need_sep {
                        out.push(';');
                    }
                    out.push('3');
                    need_sep = true;
                }
                if cell.modifiers.contains(Modifier::UNDERLINED) {
                    if need_sep {
                        out.push(';');
                    }
                    out.push('4');
                    need_sep = true;
                }
                if cell.modifiers.contains(Modifier::REVERSED) {
                    if need_sep {
                        out.push(';');
                    }
                    out.push('7');
                    need_sep = true;
                }

                if let Some(fg) = cell.fg {
                    if need_sep {
                        out.push(';');
                    }
                    write_color(&mut out, fg, false);
                    need_sep = true;
                }
                if let Some(bg) = cell.bg {
                    if need_sep {
                        out.push(';');
                    }
                    write_color(&mut out, bg, true);
                }

                out.push('m');
                out.push_str(&cell.symbol);
                out.push_str("\x1b[0m");
            }
        }
        out
    }
}

/// Write an SGR color code directly into `out`. No heap allocation.
fn write_color(out: &mut String, color: Color, bg: bool) {
    let offset: u8 = if bg { 10 } else { 0 };
    match color {
        Color::Reset => {}
        Color::Black => {
            let _ = write!(out, "{}", 30 + offset);
        }
        Color::Red => {
            let _ = write!(out, "{}", 31 + offset);
        }
        Color::Green => {
            let _ = write!(out, "{}", 32 + offset);
        }
        Color::Yellow => {
            let _ = write!(out, "{}", 33 + offset);
        }
        Color::Blue => {
            let _ = write!(out, "{}", 34 + offset);
        }
        Color::Magenta => {
            let _ = write!(out, "{}", 35 + offset);
        }
        Color::Cyan => {
            let _ = write!(out, "{}", 36 + offset);
        }
        Color::Gray => {
            let _ = write!(out, "{}", 37 + offset);
        }
        Color::DarkGray => {
            let _ = write!(out, "{}", 90 + offset);
        }
        Color::LightRed => {
            let _ = write!(out, "{}", 91 + offset);
        }
        Color::LightGreen => {
            let _ = write!(out, "{}", 92 + offset);
        }
        Color::LightYellow => {
            let _ = write!(out, "{}", 93 + offset);
        }
        Color::LightBlue => {
            let _ = write!(out, "{}", 94 + offset);
        }
        Color::LightMagenta => {
            let _ = write!(out, "{}", 95 + offset);
        }
        Color::LightCyan => {
            let _ = write!(out, "{}", 96 + offset);
        }
        Color::White => {
            let _ = write!(out, "{}", 97 + offset);
        }
        Color::Rgb(r, g, b) => {
            let prefix = if bg { 48 } else { 38 };
            let _ = write!(out, "{prefix};2;{r};{g};{b}");
        }
        Color::Indexed(i) => {
            let prefix = if bg { 48 } else { 38 };
            let _ = write!(out, "{prefix};5;{i}");
        }
    }
}

#[derive(Debug)]
pub struct AnsiBackendError;

impl std::fmt::Display for AnsiBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AnsiBackend error")
    }
}

impl std::error::Error for AnsiBackendError {}

impl Backend for AnsiBackend {
    type Error = AnsiBackendError;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            if (y as usize) < self.cells.len() && (x as usize) < self.width as usize {
                let cs = &mut self.cells[y as usize][x as usize];
                cs.symbol.clear();
                // Truncate to SYMBOL_CAP if needed (shouldn't happen for
                // single-codepoint symbols that ratatui produces per cell)
                let sym = cell.symbol();
                if sym.len() <= SYMBOL_CAP {
                    let _ = cs.symbol.try_push_str(sym);
                }
                // Symbols > SYMBOL_CAP are skipped (would require splitting
                // a UTF-8 codepoint). Shouldn't happen — ratatui produces
                // single codepoints per cell.
                let style = cell.style();
                cs.fg = style.fg.filter(|c| *c != Color::Reset);
                cs.bg = style.bg.filter(|c| *c != Color::Reset);
                cs.modifiers = style.add_modifier;
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.cursor_visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.cursor_visible = true;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.cursor_pos)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.cursor_pos = position.into();
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        for row in &mut self.cells {
            for cell in row {
                cell.symbol.clear();
                cell.symbol.push(' ');
                cell.fg = None;
                cell.bg = None;
                cell.modifiers = Modifier::empty();
            }
        }
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        match clear_type {
            ClearType::All => self.clear(),
            _ => Ok(()),
        }
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(Size::new(self.width, self.height))
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(WindowSize {
            columns_rows: Size::new(self.width, self.height),
            pixels: Size::new(self.width * 8, self.height * 16),
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_core::buffer::Cell;
    use ratatui_core::style::Style;

    #[test]
    fn new_backend_size() {
        let backend = AnsiBackend::new(40, 10);
        assert_eq!(backend.size().unwrap(), Size::new(40, 10));
    }

    #[test]
    fn draw_plain_cells() {
        let mut backend = AnsiBackend::new(5, 1);
        let cell_a = Cell::new("a");
        let cell_b = Cell::new("b");
        backend
            .draw([(0, 0, &cell_a), (1, 0, &cell_b)].into_iter())
            .unwrap();
        let ansi = backend.to_ansi();
        assert!(ansi.contains("ab   "));
    }

    #[test]
    fn draw_styled_cell() {
        let mut backend = AnsiBackend::new(3, 1);
        let mut cell = Cell::new("X");
        cell.set_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        let ansi = backend.to_ansi();
        // Should contain bold (1) and red fg (31)
        assert!(ansi.contains("\x1b[1;31m"));
        assert!(ansi.contains("X"));
        assert!(ansi.contains("\x1b[0m"));
    }

    #[test]
    fn draw_rgb_color() {
        let mut backend = AnsiBackend::new(3, 1);
        let mut cell = Cell::new("R");
        cell.set_style(Style::default().fg(Color::Rgb(255, 128, 0)));
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        let ansi = backend.to_ansi();
        assert!(ansi.contains("38;2;255;128;0"));
    }

    #[test]
    fn draw_indexed_color() {
        let mut backend = AnsiBackend::new(3, 1);
        let mut cell = Cell::new("I");
        cell.set_style(Style::default().bg(Color::Indexed(42)));
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        let ansi = backend.to_ansi();
        assert!(ansi.contains("48;5;42"));
    }

    #[test]
    fn clear_resets_cells() {
        let mut backend = AnsiBackend::new(3, 1);
        let cell = Cell::new("X");
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        backend.clear().unwrap();
        let ansi = backend.to_ansi();
        assert!(!ansi.contains("X"));
    }

    #[test]
    fn cursor_operations() {
        let mut backend = AnsiBackend::new(10, 10);
        backend.show_cursor().unwrap();
        assert!(backend.cursor_visible);
        backend.hide_cursor().unwrap();
        assert!(!backend.cursor_visible);

        backend
            .set_cursor_position(Position { x: 5, y: 3 })
            .unwrap();
        assert_eq!(
            backend.get_cursor_position().unwrap(),
            Position { x: 5, y: 3 }
        );
    }

    #[test]
    fn multiline_output() {
        let backend = AnsiBackend::new(3, 2);
        let ansi = backend.to_ansi();
        // Should have two lines separated by \r\n
        assert!(ansi.contains("\r\n"));
    }

    #[test]
    fn out_of_bounds_draw_ignored() {
        let mut backend = AnsiBackend::new(3, 1);
        let cell = Cell::new("X");
        // Drawing out of bounds should not panic
        backend.draw([(10, 10, &cell)].into_iter()).unwrap();
    }

    #[test]
    fn bg_color() {
        let mut backend = AnsiBackend::new(3, 1);
        let mut cell = Cell::new("B");
        cell.set_style(Style::default().bg(Color::Blue));
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        let ansi = backend.to_ansi();
        // Blue bg = 44
        assert!(ansi.contains("44"));
    }

    #[test]
    fn modifier_dim_italic_underline_reversed() {
        let mut backend = AnsiBackend::new(3, 1);
        let mut cell = Cell::new("M");
        cell.set_style(Style::default().add_modifier(
            Modifier::DIM | Modifier::ITALIC | Modifier::UNDERLINED | Modifier::REVERSED,
        ));
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        let ansi = backend.to_ansi();
        assert!(ansi.contains("2")); // dim
        assert!(ansi.contains("3")); // italic
        assert!(ansi.contains("4")); // underline
        assert!(ansi.contains("7")); // reversed
    }

    #[test]
    fn window_size() {
        let mut backend = AnsiBackend::new(80, 24);
        let ws = backend.window_size().unwrap();
        assert_eq!(ws.columns_rows, Size::new(80, 24));
    }

    #[test]
    fn clear_region_all() {
        let mut backend = AnsiBackend::new(3, 1);
        let cell = Cell::new("X");
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        backend.clear_region(ClearType::All).unwrap();
        let ansi = backend.to_ansi();
        assert!(!ansi.contains("X"));
    }

    #[test]
    fn clear_region_other_is_noop() {
        let mut backend = AnsiBackend::new(3, 1);
        let cell = Cell::new("X");
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        backend.clear_region(ClearType::AfterCursor).unwrap();
        let ansi = backend.to_ansi();
        assert!(ansi.contains("X"));
    }

    #[test]
    fn flush_succeeds() {
        let mut backend = AnsiBackend::new(1, 1);
        assert!(backend.flush().is_ok());
    }

    #[test]
    fn color_reset_produces_no_style_sgr() {
        let mut backend = AnsiBackend::new(3, 1);
        let mut cell = Cell::new("R");
        cell.set_style(Style::default().fg(Color::Reset));
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        let ansi = backend.to_ansi();
        // Reset fg should not produce color SGR codes (only cursor/clear codes)
        assert!(!ansi.contains("\x1b[0m"), "should not have style reset");
    }

    #[test]
    fn all_named_colors() {
        let colors = [
            Color::Black,
            Color::Green,
            Color::Yellow,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ];
        for color in colors {
            let mut backend = AnsiBackend::new(1, 1);
            let mut cell = Cell::new("C");
            cell.set_style(Style::default().fg(color));
            backend.draw([(0, 0, &cell)].into_iter()).unwrap();
            let ansi = backend.to_ansi();
            // All named colors should produce SGR codes
            assert!(ansi.contains("\x1b["), "missing SGR for {color:?}");
        }
    }

    #[test]
    fn rgb_bg_color() {
        let mut backend = AnsiBackend::new(3, 1);
        let mut cell = Cell::new("G");
        cell.set_style(Style::default().bg(Color::Rgb(10, 20, 30)));
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        let ansi = backend.to_ansi();
        assert!(ansi.contains("48;2;10;20;30"));
    }

    #[test]
    fn indexed_fg_color() {
        let mut backend = AnsiBackend::new(3, 1);
        let mut cell = Cell::new("I");
        cell.set_style(Style::default().fg(Color::Indexed(200)));
        backend.draw([(0, 0, &cell)].into_iter()).unwrap();
        let ansi = backend.to_ansi();
        assert!(ansi.contains("38;5;200"));
    }
}
