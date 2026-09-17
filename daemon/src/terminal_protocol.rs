//! Terminal capabilities belong to the pane, not the process launching lazed.
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{NamedColor, Rgb};
use portable_pty::CommandBuilder;
use std::io::Write;
use std::sync::{mpsc, Arc, Mutex};

pub fn configure_environment(cmd: &mut CommandBuilder) {
    // Build runners and agent hosts often export NO_COLOR. A new interactive
    // pane is a color terminal; shell rc files can still override this policy.
    for key in ["NO_COLOR", "FORCE_COLOR", "CLICOLOR", "CLICOLOR_FORCE"] {
        cmd.env_remove(key);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "lazed");
}

pub struct Listener(mpsc::Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if matches!(
            event,
            Event::PtyWrite(_) | Event::ColorRequest(..) | Event::TextAreaSizeRequest(_)
        ) {
            let _ = self.0.send(event);
        }
    }
}

pub struct Replies {
    events: mpsc::Receiver<Event>,
    output: mpsc::Sender<String>,
}

pub fn connect(writer: Arc<Mutex<Box<dyn Write + Send>>>) -> (Listener, Replies) {
    let (events_tx, events) = mpsc::channel();
    let (output, bytes) = mpsc::channel::<String>();
    // Never wait for the child to consume a reply while holding PtyTerm's lock.
    std::thread::spawn(move || {
        for reply in bytes {
            let mut writer = super::lock(&writer);
            if writer
                .write_all(reply.as_bytes())
                .and_then(|_| writer.flush())
                .is_err()
            {
                break;
            }
        }
    });
    (Listener(events_tx), Replies { events, output })
}

impl Replies {
    pub fn discard(&self) {
        for _ in self.events.try_iter() {}
    }

    pub fn flush(&self, term: &Term<Listener>) {
        for event in self.events.try_iter() {
            if let Some(reply) = reply(event, term) {
                let _ = self.output.send(reply);
            }
        }
    }
}

fn reply(event: Event, term: &Term<Listener>) -> Option<String> {
    match event {
        Event::PtyWrite(text) => Some(text),
        Event::ColorRequest(index, format) => term.colors()[index]
            .or_else(|| palette(index))
            .map(|rgb| format(rgb)),
        Event::TextAreaSizeRequest(format) => Some(format(WindowSize {
            num_lines: term.screen_lines() as u16,
            num_cols: term.columns() as u16,
            cell_width: 0,
            cell_height: 0,
        })),
        _ => None,
    }
}

fn palette(index: usize) -> Option<Rgb> {
    // Keep the base/default colors in sync with src/terminal/config.ts.
    const ANSI: [u32; 16] = [
        0x1d1f21, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xc5c8c6, 0x666666,
        0xd54e53, 0xb9ca4a, 0xe7c547, 0x7aa6da, 0xc397d8, 0x70c0b1, 0xeaeaea,
    ];
    let value = match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let n = index - 16;
            let component = |v: usize| if v == 0 { 0 } else { 55 + 40 * v as u32 };
            (component(n / 36) << 16) | (component(n / 6 % 6) << 8) | component(n % 6)
        }
        232..=255 => (8 + 10 * (index as u32 - 232)) * 0x010101,
        n if n == NamedColor::Foreground as usize || n == NamedColor::Cursor as usize => 0xffffff,
        n if n == NamedColor::Background as usize => 0x0e1014,
        _ => return None,
    };
    Some(Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    #[test]
    fn pane_does_not_inherit_launcher_color_overrides() {
        let mut cmd = CommandBuilder::new("sh");
        for key in ["NO_COLOR", "FORCE_COLOR", "CLICOLOR", "CLICOLOR_FORCE"] {
            cmd.env(key, "0");
        }
        configure_environment(&mut cmd);
        for key in ["NO_COLOR", "FORCE_COLOR", "CLICOLOR", "CLICOLOR_FORCE"] {
            assert!(cmd.get_env(key).is_none());
        }
        assert_eq!(cmd.get_env("TERM").unwrap(), "xterm-256color");
        assert_eq!(cmd.get_env("COLORTERM").unwrap(), "truecolor");
    }

    #[test]
    fn tui_queries_get_color_and_terminal_replies() {
        let (tx, events) = mpsc::channel();
        let mut term = Term::new(
            Config::default(),
            &crate::term::Sz {
                cols: 80,
                lines: 24,
            },
            Listener(tx),
        );
        let mut parser: Processor = Processor::new();
        for byte in b"\x1b]10;?\x07\x1b]11;?\x1b\\\x1b]4;1;?\x07\x1b[6n\x1b[c\x1b[18t" {
            parser.advance(&mut term, *byte);
        }
        let replies: Vec<_> = events
            .try_iter()
            .filter_map(|event| reply(event, &term))
            .collect();
        assert_eq!(replies.len(), 6);
        assert_eq!(replies[0], "\x1b]10;rgb:ffff/ffff/ffff\x07");
        assert_eq!(replies[1], "\x1b]11;rgb:0e0e/1010/1414\x1b\\");
        assert_eq!(replies[2], "\x1b]4;1;rgb:cccc/6666/6666\x07");
        assert_eq!(replies[3], "\x1b[1;1R");
        assert!(replies[4].starts_with("\x1b[?"));
        assert_eq!(replies[5], "\x1b[8;24;80t");
    }

    #[test]
    fn question_background_and_answer_styles_survive_frame_serialization() {
        use crate::render::{row_ansi, Style};
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::index::Line;
        let size = crate::term::Sz { cols: 40, lines: 2 };
        let mut source = Term::new(Config::default(), &size, VoidListener);
        let mut client = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        let input =
            "\x1b[48;2;38;42;52m\x1b[1;36m> question\x1b[0m\r\n\x1b[38;5;114manswer \x1b[2;3mnote";
        for byte in input.bytes() {
            parser.advance(&mut source, byte);
        }
        let mut client_parser: Processor = Processor::new();
        for line in 0..2 {
            let row = &source.grid()[Line(line)];
            let frame = row_ansi(row.into_iter().map(|cell| {
                (
                    cell.c,
                    Style {
                        fg: cell.fg,
                        bg: cell.bg,
                        flags: cell.flags,
                    },
                )
            }));
            for byte in format!("\x1b[{};1H{}", line + 1, frame).bytes() {
                client_parser.advance(&mut client, byte);
            }
            for (before, after) in row.into_iter().zip(client.grid()[Line(line)].into_iter()) {
                assert_eq!(before.c, after.c);
                assert_eq!(before.fg, after.fg);
                assert_eq!(before.bg, after.bg);
                assert_eq!(before.flags, after.flags);
            }
        }
    }

    #[test]
    fn replies_are_delivered_to_the_pty_writer() {
        struct Writer(mpsc::Sender<Vec<u8>>);
        impl Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.send(bytes.to_vec()).unwrap();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (sent, received) = mpsc::channel();
        let (listener, replies) = connect(Arc::new(Mutex::new(Box::new(Writer(sent)))));
        let mut term = Term::new(
            Config::default(),
            &crate::term::Sz {
                cols: 80,
                lines: 24,
            },
            listener,
        );
        let mut parser: Processor = Processor::new();
        for byte in b"\x1b]11;?\x07" {
            parser.advance(&mut term, *byte);
        }
        replies.flush(&term);
        assert_eq!(
            received
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            b"\x1b]11;rgb:0e0e/1010/1414\x07"
        );
    }
}
