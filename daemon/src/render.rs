//! Serialize terminal cells to ANSI, and produce line-granular diffs.
//!
//! A "frame" is the ANSI bytes that turn the client's previous viewport into
//! the current one: a cursor-position escape per changed row plus the row's
//! styled text. Full frames repaint every row.
use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    row: usize,
    col: usize,
    visible: bool,
    style: u8,
}

impl Cursor {
    pub fn from_term<T: EventListener>(term: &Term<T>) -> Self {
        let content = term.renderable_content();
        let style = term.cursor_style();
        let shape = match style.shape {
            CursorShape::Block | CursorShape::HollowBlock | CursorShape::Hidden => 1,
            CursorShape::Underline => 3,
            CursorShape::Beam => 5,
        };
        Self {
            row: content.cursor.point.line.0.max(0) as usize + 1,
            col: content.cursor.point.column.0 + 1,
            visible: content.display_offset == 0 && content.cursor.shape != CursorShape::Hidden,
            style: shape + u8::from(!style.blinking),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

impl Style {
    fn ansi(&self) -> String {
        let mut s = String::with_capacity(64);
        s.push_str("\x1b[0m");
        let f = self.flags;
        if f.contains(Flags::BOLD) {
            s.push_str("\x1b[1m");
        }
        if f.contains(Flags::DIM) {
            s.push_str("\x1b[2m");
        }
        if f.contains(Flags::ITALIC) {
            s.push_str("\x1b[3m");
        }
        if f.contains(Flags::UNDERLINE) {
            s.push_str("\x1b[4m");
        }
        if f.contains(Flags::INVERSE) {
            s.push_str("\x1b[7m");
        }
        if f.contains(Flags::STRIKEOUT) {
            s.push_str("\x1b[9m");
        }
        s.push_str(&fg_ansi(self.fg));
        s.push_str(&bg_ansi(self.bg));
        s
    }
}

fn fg_ansi(c: Color) -> String {
    match c {
        Color::Named(n) => format!("\x1b[{}m", named_fg(n)),
        Color::Indexed(i) => format!("\x1b[38;5;{i}m"),
        Color::Spec(rgb) => format!("\x1b[38;2;{};{};{}m", rgb.r, rgb.g, rgb.b),
    }
}

fn bg_ansi(c: Color) -> String {
    match c {
        Color::Named(n) => format!("\x1b[{}m", named_bg(n)),
        Color::Indexed(i) => format!("\x1b[48;5;{i}m"),
        Color::Spec(rgb) => format!("\x1b[48;2;{};{};{}m", rgb.r, rgb.g, rgb.b),
    }
}

fn named_fg(c: NamedColor) -> u8 {
    use NamedColor::*;
    match c {
        Black => 30,
        Red => 31,
        Green => 32,
        Yellow => 33,
        Blue => 34,
        Magenta => 35,
        Cyan => 36,
        White => 37,
        BrightBlack => 90,
        BrightRed => 91,
        BrightGreen => 92,
        BrightYellow => 93,
        BrightBlue => 94,
        BrightMagenta => 95,
        BrightCyan => 96,
        BrightWhite => 97,
        Foreground => 39,
        Background => 39,
        Cursor => 39,
        DimBlack => 30,
        DimRed => 31,
        DimGreen => 32,
        DimYellow => 33,
        DimBlue => 34,
        DimMagenta => 35,
        DimCyan => 36,
        DimWhite => 37,
        BrightForeground => 39,
        DimForeground => 39,
    }
}

fn named_bg(c: NamedColor) -> u8 {
    use NamedColor::*;
    match c {
        Black => 40,
        Red => 41,
        Green => 42,
        Yellow => 43,
        Blue => 44,
        Magenta => 45,
        Cyan => 46,
        White => 47,
        BrightBlack => 100,
        BrightRed => 101,
        BrightGreen => 102,
        BrightYellow => 103,
        BrightBlue => 104,
        BrightMagenta => 105,
        BrightCyan => 106,
        BrightWhite => 107,
        Foreground => 49,
        Background => 49,
        Cursor => 49,
        DimBlack => 40,
        DimRed => 41,
        DimGreen => 42,
        DimYellow => 43,
        DimBlue => 44,
        DimMagenta => 45,
        DimCyan => 46,
        DimWhite => 47,
        BrightForeground => 49,
        DimForeground => 49,
    }
}

/// Serialize one row of cells to an ANSI string (no cursor positioning).
/// `cells` yields (char, Style) pairs in column order; wide-char spacers are
/// skipped by the caller.
pub fn row_ansi<I: Iterator<Item = (char, Style)>>(cells: I) -> String {
    let mut out = String::with_capacity(128);
    let mut cur: Option<Style> = None;
    for (ch, st) in cells {
        if cur != Some(st) {
            out.push_str(&st.ansi());
            cur = Some(st);
        }
        out.push(ch);
    }
    out.push_str("\x1b[0m");
    out
}

/// Cursor-position escape + serialized content for one viewport row.
pub fn write_row(out: &mut Vec<u8>, row_idx: usize, row: &str) {
    // \x1b[K erases the row first — without it a shorter rewrite leaves
    // the previous row's tail on screen (ghost text)
    out.extend_from_slice(format!("\x1b[{};1H\x1b[K", row_idx + 1).as_bytes());
    out.extend_from_slice(row.as_bytes());
}

/// Clear everything at and below `from` — used when the rowset shrinks.
pub fn write_clear_below(out: &mut Vec<u8>, from: usize) {
    out.extend_from_slice(format!("\x1b[{};1H\x1b[0J", from + 1).as_bytes());
}

/// Position/shape/visibility trailer for the application cursor. Row
/// painting moves the client cursor, so this restores it after a repaint
/// and also delivers cursor-only changes.
pub fn write_cursor(out: &mut Vec<u8>, cursor: Cursor) {
    out.extend_from_slice(
        format!(
            "\x1b[{};{}H\x1b[{} q\x1b[?25{}",
            cursor.row,
            cursor.col,
            cursor.style,
            if cursor.visible { 'h' } else { 'l' }
        )
        .as_bytes(),
    );
}

/// Diff two serialized rowsets; returns ANSI bytes and the new rows.
/// `full` forces a complete repaint.
///
/// Test-only reference implementation — the live emitter (`term::emit_frame`)
/// shares the `write_*` helpers but skips serializing undamaged rows.
#[cfg(test)]
pub fn diff_frame(
    prev: &mut Vec<String>,
    next: Vec<String>,
    full: bool,
    prev_cursor: &mut Option<Cursor>,
    cursor: Cursor,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(next.len() * 40);
    for (i, row) in next.iter().enumerate() {
        if !full && prev.get(i) == Some(row) {
            continue;
        }
        write_row(&mut out, i, row);
    }
    // shrink/grow mismatch: clear leftover region
    if prev.len() > next.len() {
        write_clear_below(&mut out, next.len());
    }
    *prev = next;
    if !out.is_empty() || full || *prev_cursor != Some(cursor) {
        write_cursor(&mut out, cursor);
    }
    *prev_cursor = Some(cursor);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Sz;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    #[test]
    fn frames_restore_cursor_and_deliver_cursor_only_updates() {
        let size = Sz { cols: 20, lines: 4 };
        let mut source = Term::new(Config::default(), &size, VoidListener);
        let mut client = Term::new(Config::default(), &size, VoidListener);
        let mut source_parser: Processor = Processor::new();
        let mut client_parser: Processor = Processor::new();
        let mut rows = Vec::new();
        let mut cursor = None;
        let screen = vec!["prompt".to_string(), "status".to_string()];

        for (index, sequence) in [
            "\x1b[1;7H\x1b[5 q", // input position before the status line
            "\x1b[2D",           // left arrow changes no cells
            "\x1b[?25l",         // TUI hides cursor while drawing
            "\x1b[?25h\x1b[4 q", // restore with a different shape
        ]
        .iter()
        .enumerate()
        {
            for byte in sequence.bytes() {
                source_parser.advance(&mut source, byte);
            }
            let frame = diff_frame(
                &mut rows,
                screen.clone(),
                index == 0,
                &mut cursor,
                Cursor::from_term(&source),
            );
            assert!(!frame.is_empty());
            for byte in frame {
                client_parser.advance(&mut client, byte);
            }
            assert!(Cursor::from_term(&client) == Cursor::from_term(&source));
        }
        assert!(diff_frame(
            &mut rows,
            screen.clone(),
            false,
            &mut cursor,
            Cursor::from_term(&source)
        )
        .is_empty());

        // Repainting unrelated text must still restore the input cursor.
        let frame = diff_frame(
            &mut rows,
            vec!["changed".into()],
            false,
            &mut cursor,
            Cursor::from_term(&source),
        );
        for byte in frame {
            client_parser.advance(&mut client, byte);
        }
        assert!(Cursor::from_term(&client) == Cursor::from_term(&source));

        // A fresh/reconnected client gets cursor state even for identical rows.
        assert!(!diff_frame(
            &mut rows,
            vec!["changed".into()],
            true,
            &mut cursor,
            Cursor::from_term(&source)
        )
        .is_empty());
    }

    #[test]
    fn scrollback_hides_cursor_until_live_view_is_restored() {
        let size = Sz { cols: 20, lines: 2 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        for byte in b"one\r\ntwo\r\nthree" {
            parser.advance(&mut term, *byte);
        }
        assert!(Cursor::from_term(&term).visible);
        term.scroll_display(Scroll::Delta(1));
        assert!(!Cursor::from_term(&term).visible);
        term.scroll_display(Scroll::Bottom);
        assert!(Cursor::from_term(&term).visible);
    }
}
