use crate::draw::{accent, editor_area, selected_text};
use crate::term;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;
use ratatui::Frame;

pub struct Tab {
    pub(crate) name: String,
    pub(crate) term: term::Term,
    pub(crate) born: std::time::Instant,
    pub(crate) typing: Option<String>,
}

#[derive(Default)]
pub struct Tabs {
    pub(crate) terms: Vec<Tab>,
    pub(crate) active: usize,
    pub shown: bool,
    pub clipboard: Option<String>,
    areas: Vec<Rect>,
    back: Rect,
    closer: Rect,
    selection: Option<(Position, Position)>,
    copied: Option<usize>,
    copy: bool,
    enhanced: Option<bool>,
    cycling: bool,
    released: bool,
}

impl Tabs {
    fn hold(on: bool) -> bool {
        let flags = event::KeyboardEnhancementFlags::all();
        let _ = match on {
            true => ratatui::crossterm::execute!(std::io::stdout(), event::PushKeyboardEnhancementFlags(flags)),
            false => ratatui::crossterm::execute!(std::io::stdout(), event::PopKeyboardEnhancementFlags),
        };
        on
    }

    fn enhanced(&mut self) -> bool {
        *self.enhanced.get_or_insert_with(|| ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false))
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Ends the program in the tab on screen and goes to the next one, or back when it was the last.
    fn close_active(&mut self) {
        if self.active < self.terms.len() {
            self.terms.remove(self.active);
        }
        (self.selection, self.copied) = (None, None);
        self.active = self.active.min(self.terms.len().saturating_sub(1));
        self.shown &= !self.terms.is_empty();
    }

    pub fn close(&mut self, name: &str) {
        self.terms.retain(|tab| tab.name != name);
    }

    pub fn has(&self, name: &str) -> bool {
        self.terms.iter().any(|tab| tab.name == name)
    }

    pub fn changed(&mut self) -> bool {
        self.terms.iter().fold(false, |changed, tab| tab.term.changed() | changed)
    }

    pub fn run(&mut self, name: &str, command: &std::process::Command, screen: Rect) -> Result<(), String> {
        self.run_typing(name, command, screen, None)
    }

    /// `typing` is put into the program's input once it has settled, for the person to edit and send.
    pub fn run_typing(&mut self, name: &str, command: &std::process::Command, screen: Rect, typing: Option<String>) -> Result<(), String> {
        if !self.has(name) {
            let area = editor_area(screen);
            let term = term::Term::spawn(command, area.height, area.width)?;
            self.terms.push(Tab { name: name.into(), term, born: std::time::Instant::now(), typing });
        }
        if let Some(i) = self.terms.iter().position(|tab| tab.name == name) {
            (self.active, self.shown) = (i, true);
        }
        Ok(())
    }

    pub fn tick(&mut self) -> Vec<String> {
        for tab in self.terms.iter_mut().filter(|tab| tab.typing.is_some()) {
            let settled = tab.born.elapsed().as_millis() > 1500 && tab.term.quiet_for().as_millis() > 500;
            if settled || tab.born.elapsed().as_secs() > 10 {
                tab.term.type_text(&tab.typing.take().unwrap_or_default());
            }
        }
        let young = self.terms.iter_mut().filter(|tab| tab.born.elapsed().as_secs() < 3);
        let failed = young.filter_map(|tab| tab.term.failed().then(|| tab.name.clone())).collect();
        self.terms.retain_mut(|tab| tab.term.alive());
        self.shown &= !self.terms.is_empty();
        self.active = self.active.min(self.terms.len().saturating_sub(1));
        if self.cycling && !self.shown {
            self.cycling = Tabs::hold(false);
        }
        failed
    }

    pub fn open(&mut self) {
        if self.terms.is_empty() {
            return;
        }
        self.shown = true;
        if self.enhanced() {
            (self.cycling, self.released) = (Tabs::hold(true), false);
        }
    }

    pub fn handle(&mut self, event: Event) {
        let count = self.terms.len();
        if !self.shown || count == 0 {
            return;
        }
        match event {
            Event::Key(key) if self.cycling && key.kind == KeyEventKind::Release => {
                self.released = true;
                if matches!(key.code, KeyCode::Modifier(event::ModifierKeyCode::LeftControl | event::ModifierKeyCode::RightControl)) {
                    self.cycling = Tabs::hold(false);
                }
            }
            Event::Key(key) if key.kind == KeyEventKind::Press && key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('\\' | '4' | '/' | '7')) => self.close_active(),
            Event::Key(key) if key.kind == KeyEventKind::Press && !matches!(key.code, KeyCode::Modifier(_)) => {
                (self.selection, self.copied) = (None, None);
                let switch = key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL);
                if self.cycling && switch && !self.released {
                    self.enhanced = Some(false);
                }
                if self.cycling && (!switch || !self.released) {
                    self.cycling = Tabs::hold(false);
                }
                match switch {
                    true if self.cycling => self.active = (self.active + 1) % count,
                    true => self.shown = false,
                    false => self.terms[self.active].term.input(key),
                }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    (self.selection, self.copied) = (None, None);
                    self.terms[self.active].term.scroll(if m.kind == MouseEventKind::ScrollUp { 3 } else { -3 });
                }
                MouseEventKind::Down(_) if self.back.contains(Position::new(m.column, m.row)) => self.shown = false,
                MouseEventKind::Down(_) if self.closer.contains(Position::new(m.column, m.row)) => self.close_active(),
                MouseEventKind::Down(_) => {
                    let at = Position::new(m.column, m.row);
                    self.active = self.areas.iter().position(|area| area.contains(at)).unwrap_or(self.active);
                    (self.selection, self.copied) = (Some((Position::new(m.column, m.row.max(1)), Position::new(m.column, m.row.max(1)))), None);
                }
                MouseEventKind::Drag(_) => {
                    if let Some((start, _)) = self.selection {
                        self.selection = Some((start, Position::new(m.column, m.row.max(1))));
                    }
                }
                MouseEventKind::Up(_) => match self.selection {
                    Some((start, end)) if start != end => self.copy = true,
                    _ => {
                        self.selection = None;
                        if m.row > 0 {
                            self.terms[self.active].term.click(m.column, m.row - 1);
                        }
                    }
                },
                _ => {}
            },
            _ => {}
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, back: &str) {
        if !self.shown || self.terms.is_empty() {
            return;
        }
        let screen = frame.area();
        frame.render_widget(Clear, screen);
        self.active = self.active.min(self.terms.len() - 1);
        self.areas.clear();
        let mut spans = Vec::new();
        let mut x = screen.x;
        for (i, tab) in self.terms.iter().enumerate() {
            let text = if i == self.active { format!(" {} {} × ", i + 1, tab.name) } else { format!(" {} {} ", i + 1, tab.name) };
            let width = text.chars().count() as u16;
            self.areas.push(Rect::new(x, screen.y, width, 1));
            if i == self.active {
                self.closer = Rect::new(x + width.saturating_sub(3), screen.y, 3, 1);
            }
            x += width + 1;
            match i == self.active {
                true => spans.push(Span::styled(text, Style::new().fg(Color::Black).bg(accent()).add_modifier(Modifier::BOLD))),
                false => spans.extend([
                    Span::styled(format!(" {}", i + 1), Style::new().fg(accent())),
                    Span::styled(format!(" {} ", tab.name), Style::new().fg(Color::Gray)),
                ]),
            }
            spans.push(Span::raw(" "));
        }
        let stepping = self.terms.len() > 1 && self.enhanced != Some(false);
        let place = if stepping { format!("{back} / next tab") } else { back.to_string() };
        let place = format!("{place}   ctrl-\\ · close this tab");
        self.back = Rect::new(x, screen.y, (10 + back.chars().count() as u16).min(screen.right().saturating_sub(x)), 1);
        spans.extend([
            Span::styled(" ctrl-q", Style::new().fg(accent()).add_modifier(Modifier::BOLD)),
            Span::styled(" · ", Style::new().fg(Color::Gray)),
            Span::styled(place, Style::new().add_modifier(Modifier::BOLD)),
        ]);
        frame.render_widget(Line::from(spans), Rect { height: 1, ..screen });
        let body = editor_area(screen);
        if let Some(cursor) = self.terms[self.active].term.render(body, frame.buffer_mut()) {
            frame.set_cursor_position(cursor);
        }
        if let Some((a, b)) = self.selection.filter(|(a, b)| a != b) {
            let text = selected_text(frame.buffer_mut(), body.x, body.right().saturating_sub(1), a, b, false);
            if self.copy {
                self.copy = false;
                self.copied = Some(text.chars().count());
                self.clipboard = Some(text);
            }
        }
        if let Some(count) = self.copied {
            let note = format!(" copied {count} characters ");
            let width = note.chars().count() as u16;
            let corner = Rect::new(screen.right().saturating_sub(width), screen.y, width.min(screen.width), 1);
            frame.render_widget(Span::styled(note, Style::new().fg(Color::Black).bg(Color::Yellow)), corner);
        }
    }
}
