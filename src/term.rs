use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtyPair, PtySize};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct Term {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: (u16, u16),
    dirty: Arc<AtomicBool>,
    last_output: Arc<Mutex<std::time::Instant>>,
    scrolled: usize,
}

fn cells(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n.max(1)).unwrap()
}

fn color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn control(c: char) -> u8 {
    match c {
        '2' | ' ' | '@' => 0x00,
        '3' | '[' => 0x1b,
        '4' | '\\' => 0x1c,
        '5' | ']' => 0x1d,
        '6' | '^' => 0x1e,
        '7' | '/' | '_' | '-' => 0x1f,
        '8' | '?' => 0x7f,
        c => c.to_ascii_lowercase() as u8 & 0x1f,
    }
}

impl Term {
    pub fn spawn(command: &std::process::Command, rows: u16, cols: u16) -> Result<Term, String> {
        let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
        let PtyPair { master, slave } = native_pty_system().openpty(size).map_err(|e| e.to_string())?;
        let mut builder = CommandBuilder::new(command.get_program());
        builder.args(command.get_args());
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                builder.env(key, value);
            }
        }
        builder.env("TERM", "xterm-256color");
        if let Some(dir) = command.get_current_dir().map(Into::into).or(std::env::current_dir().ok()) {
            builder.cwd(dir);
        }
        let child = slave.spawn_command(builder).map_err(|e| e.to_string())?;
        drop(slave);
        let mut reader = master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = master.take_writer().map_err(|e| e.to_string())?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(cells(rows), cells(cols), 2000)));
        let dirty = Arc::new(AtomicBool::new(true));
        let last_output = Arc::new(Mutex::new(std::time::Instant::now()));
        let (shared, changed, heard) = (parser.clone(), dirty.clone(), last_output.clone());
        std::thread::spawn(move || {
            let mut bytes = [0u8; 8192];
            while let Ok(n @ 1..) = reader.read(&mut bytes) {
                shared.lock().unwrap().process(&bytes[..n]);
                changed.store(true, Ordering::Relaxed);
                *heard.lock().unwrap() = std::time::Instant::now();
            }
            changed.store(true, Ordering::Relaxed);
        });
        Ok(Term { parser, writer, master, child, size: (rows, cols), dirty, last_output, scrolled: 0 })
    }

    pub fn changed(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    pub fn alive(&mut self) -> bool {
        self.child.try_wait().is_ok_and(|status| status.is_none())
    }

    pub fn failed(&mut self) -> bool {
        self.child.try_wait().is_ok_and(|status| status.is_some_and(|status| !status.success()))
    }

    pub fn scroll(&mut self, delta: i32) {
        let mut parser = self.parser.lock().unwrap();
        let wanted = (self.scrolled as i32 + delta).max(0) as usize;
        parser.screen_mut().set_scrollback(wanted);
        self.scrolled = wanted.min(parser.screen().scrollback_len());
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn click(&mut self, column: u16, row: u16) {
        let screen = self.parser.lock().unwrap().screen().clone();
        if self.scrolled > 0 || screen.mouse_protocol_mode() == vt100::MouseProtocolMode::None {
            return;
        }
        let (x, y) = (column + 1, row + 1);
        let bytes = match screen.mouse_protocol_encoding() {
            vt100::MouseProtocolEncoding::Sgr => format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").into_bytes(),
            _ if x > 222 || y > 222 => return,
            _ => [b"\x1b[M".as_slice(), &[32, 32 + x as u8, 32 + y as u8], b"\x1b[M", &[35, 32 + x as u8, 32 + y as u8]].concat(),
        };
        let _ = self.writer.write_all(&bytes).and_then(|_| self.writer.flush());
    }

    pub fn type_text(&mut self, text: &str) {
        let bracketed = self.parser.lock().unwrap().screen().bracketed_paste();
        let pasted = if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text.to_string() };
        let _ = self.writer.write_all(pasted.as_bytes()).and_then(|_| self.writer.flush());
    }

    pub fn quiet_for(&self) -> std::time::Duration {
        self.last_output.lock().unwrap().elapsed()
    }

    pub fn input(&mut self, key: KeyEvent) {
        if self.scrolled > 0 {
            self.scroll(-(self.scrolled as i32));
        }
        let mut bytes = Vec::new();
        if key.modifiers.contains(KeyModifiers::ALT) {
            bytes.push(0x1b);
        }
        let sequence: &[u8] = match key.code {
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => &[control(c)],
            KeyCode::Char(c) => &c.to_string().into_bytes(),
            KeyCode::Enter => b"\r",
            KeyCode::Esc => b"\x1b",
            KeyCode::Backspace => b"\x7f",
            KeyCode::Tab => b"\t",
            KeyCode::BackTab => b"\x1b[Z",
            KeyCode::Up => b"\x1b[A",
            KeyCode::Down => b"\x1b[B",
            KeyCode::Right => b"\x1b[C",
            KeyCode::Left => b"\x1b[D",
            KeyCode::Home => b"\x1b[H",
            KeyCode::End => b"\x1b[F",
            KeyCode::PageUp => b"\x1b[5~",
            KeyCode::PageDown => b"\x1b[6~",
            KeyCode::Delete => b"\x1b[3~",
            _ => return,
        };
        bytes.extend_from_slice(sequence);
        let _ = self.writer.write_all(&bytes).and_then(|_| self.writer.flush());
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer) -> Option<Position> {
        if self.size != (area.height, area.width) && area.height > 0 && area.width > 0 {
            self.size = (area.height, area.width);
            let size = PtySize { rows: area.height, cols: area.width, pixel_width: 0, pixel_height: 0 };
            let _ = self.master.resize(size);
            self.parser.lock().unwrap().screen_mut().set_size(cells(area.height), cells(area.width));
        }
        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        for row in 0..area.height {
            for col in 0..area.width {
                let Some(cell) = screen.cell(row, col).filter(|cell| !cell.is_wide_continuation()) else { continue };
                let mut style = Style::new().fg(color(cell.fgcolor())).bg(color(cell.bgcolor()));
                for (on, modifier) in [
                    (cell.bold(), Modifier::BOLD),
                    (cell.italic(), Modifier::ITALIC),
                    (cell.underline(), Modifier::UNDERLINED),
                    (cell.inverse(), Modifier::REVERSED),
                ] {
                    if on {
                        style = style.add_modifier(modifier);
                    }
                }
                let target = &mut buffer[(area.x + col, area.y + row)];
                target.set_symbol(if cell.contents().is_empty() { " " } else { cell.contents() });
                target.set_style(style);
            }
        }
        let (row, col) = screen.cursor_position();
        (!screen.hide_cursor()).then(|| Position::new(area.x + col, area.y + row))
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
