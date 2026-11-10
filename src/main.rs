mod bundle;
mod mrs;
mod term;

use bundle::{Bundle, Finished, Item, Kind, Note, Provenance, State, Status};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListState, Padding, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};
use std::collections::HashMap;
use syntect::easy::HighlightLines;
use std::path::{Path, PathBuf};

#[derive(serde::Serialize, serde::Deserialize)]
struct Panes {
    guide: u16,
    detail: f32,
    image: f32,
    #[serde(default = "Panes::beside")]
    side: bool,
    #[serde(default)]
    side_with_artifact: bool,
    #[serde(default = "Panes::third")]
    wide_detail: f32,
    #[serde(default = "Panes::third")]
    wide_image: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Handle {
    Guide,
    Split,
    Picture,
    WideDetail,
    WideImage,
}

fn share(percent: f32) -> Constraint {
    Constraint::Ratio((percent.clamp(0.0, 100.0) * 100.0) as u32, 10_000)
}

impl Default for Panes {
    fn default() -> Panes {
        Panes { guide: 40, detail: 40.0, image: 50.0, side: true, side_with_artifact: false, wide_detail: 34.0, wide_image: 34.0 }
    }
}

impl Panes {
    fn beside() -> bool {
        true
    }

    fn third() -> f32 {
        34.0
    }

    fn path() -> PathBuf {
        mrs::data_dir().join("panes.json")
    }

    fn load() -> Panes {
        let saved = std::fs::read_to_string(Panes::path()).ok();
        saved.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn resize(&mut self, key: char) {
        match key {
            'H' => self.guide = self.guide.saturating_sub(4).max(6),
            'L' => self.guide = self.guide.saturating_add(4).min(600),
            'K' => self.detail = (self.detail - 5.0).max(4.0),
            'J' => self.detail = (self.detail + 5.0).min(96.0),
            '<' => self.image = (self.image + 5.0).min(94.0),
            _ => self.image = (self.image - 5.0).max(6.0),
        }
        self.save();
    }

    fn save(&self) {
        if cfg!(test) {
            return;
        }
        let _ = std::fs::create_dir_all(mrs::data_dir());
        let _ = std::fs::write(Panes::path(), serde_json::to_string(self).unwrap_or_default());
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Session {
    agent: String,
    handle: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    cwd: String,
}

pub fn url_encode(text: &str) -> String {
    let keep = |b: u8| b.is_ascii_alphanumeric() || b"-_.~".contains(&b);
    text.bytes().map(|b| if keep(b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub fn uuid() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    let _ = std::fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes));
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

pub struct Tab {
    name: String,
    term: term::Term,
    born: std::time::Instant,
    typing: Option<String>,
}

#[derive(Default)]
pub struct Tabs {
    terms: Vec<Tab>,
    active: usize,
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
            Event::Key(key) if key.kind == KeyEventKind::Press && key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('\\' | '4')) => self.close_active(),
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

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Ask,
    Verify,
    Artifact,
}

#[derive(Clone, Copy)]
enum Target {
    Reuse(usize),
    New(usize),
}

const SKILL: &str = include_str!("../skill/SKILL.md");
const DRIVE: &str = include_str!("../skill/DRIVE.md");
const GUIDE: &str = include_str!("../skill/GUIDE.md");
const SETUP: &str = include_str!("../SETUP.md");
pub const SETUP_PROMPT: &str = "Set up revui for me on this machine. Run: revui setup --guide and follow what it prints.";

#[derive(Clone, Copy, Default)]
struct Mark {
    file: usize,
    new: u32,
    old: u32,
    sign: char,
}

type Diff = (Vec<Line<'static>>, usize, Vec<Mark>);

struct Input {
    title: String,
    text: String,
    cursor: usize,
    note: Note,
    edit: Option<usize>,
}

enum Finish {
    Menu(usize),
    Preview(usize, String),
}

struct Point {
    heading: String,
    text: String,
    file: Option<String>,
    line: Option<u32>,
    start: Option<u32>,
    old: bool,
    reply_to: Option<String>,
    item: Option<String>,
}

const FINISH: [&str; 4] = [
    "Publish as one comment on the merge request",
    "Publish as one comment, and approve",
    "Publish each point as its own thread on the diff",
    "Copy the text instead, and post nothing",
];

/// Long code lines continue on the next row under a blank gutter, so nothing runs off the pane.
fn wrap_code(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width < 12 || line.width() <= width {
        return vec![line];
    }
    let gutter = line.spans.first().map_or(Style::new(), |sign| sign.style);
    let cells: Vec<(char, Style)> = line.spans.iter().flat_map(|span| span.content.chars().map(|c| (c, span.style)).collect::<Vec<_>>()).collect();
    let mut rows = Vec::new();
    let mut rest = cells.as_slice();
    while !rest.is_empty() {
        let room = if rows.is_empty() { width } else { width - 2 };
        let (row, after) = rest.split_at(room.min(rest.len()));
        let mut spans = if rows.is_empty() { Vec::new() } else { vec![Span::styled("  ", gutter)] };
        for run in row.chunk_by(|a, b| a.1 == b.1) {
            spans.push(Span::styled(run.iter().map(|(c, _)| *c).collect::<String>(), run[0].1));
        }
        rows.push(Line::from(spans));
        rest = after;
    }
    rows
}

fn hunk_lines(path: &str, hunk: &[String], highlight: bool) -> Vec<Line<'static>> {
    let (syntaxes, theme) = syntax();
    let name = path.rsplit('/').next().unwrap_or("");
    let language = syntaxes
        .find_syntax_by_extension(name.rsplit('.').next().unwrap_or(""))
        .or_else(|| syntaxes.find_syntax_by_extension(name))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let mut old = HighlightLines::new(language, theme);
    let mut new = HighlightLines::new(language, theme);
    let mut lines = Vec::new();
    for text in hunk {
        let (sign, code) = text.split_at(text.len().min(1));
        let (mark, background) = match sign {
            "+" => (Color::Green, Some(Color::Rgb(20, 46, 28))),
            "-" => (Color::Red, Some(Color::Rgb(58, 22, 26))),
            _ => (Color::DarkGray, None),
        };
        let paint = |style: Style| background.map_or(style, |bg| style.bg(bg));
        let mut spans = vec![Span::styled(format!("{sign} "), paint(Style::new().fg(mark).add_modifier(Modifier::BOLD)))];
        let source = format!("{code}\n");
        let colored = match (highlight, sign) {
            (false, _) | (_, "\\") => Vec::new(),
            (_, "-") => old.highlight_line(&source, syntaxes).unwrap_or_default(),
            _ => {
                if sign == " " {
                    let _ = old.highlight_line(&source, syntaxes);
                }
                new.highlight_line(&source, syntaxes).unwrap_or_default()
            }
        };
        if colored.is_empty() {
            let plain = if sign == "\\" { Style::new().fg(Color::DarkGray) } else { Style::new() };
            spans.push(Span::styled(code.replace('\t', "    "), paint(plain)));
        }
        for (style, piece) in colored {
            let color = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
            spans.push(Span::styled(piece.trim_end_matches('\n').replace('\t', "    "), paint(Style::new().fg(color))));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn syntax() -> &'static (syntect::parsing::SyntaxSet, syntect::highlighting::Theme) {
    static LOADED: std::sync::OnceLock<(syntect::parsing::SyntaxSet, syntect::highlighting::Theme)> = std::sync::OnceLock::new();
    LOADED.get_or_init(|| {
        let theme = two_face::theme::extra().get(two_face::theme::EmbeddedThemeName::TwoDark).clone();
        (two_face::syntax::extra_newlines(), theme)
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Overview,
    Group(usize),
    Item(usize),
    Spacer,
    Heading,
    Thread(usize),
    Note(usize),
}

struct App {
    bundle: Bundle,
    state: State,
    rows: Vec<Row>,
    sel: usize,
    scroll: u16,
    evidence: usize,
    zoom: bool,
    panes: Panes,
    focus: usize,
    detail_scroll: u16,
    list: ListState,
    areas: [Rect; 3],
    drag: Option<Handle>,
    hover: Option<Handle>,
    handles: Vec<(Rect, Handle)>,
    right_area: Rect,
    upper_area: Rect,
    side: bool,
    diffed: Diff,
    highlighted: HashMap<(usize, usize), Vec<Line<'static>>>,
    incoming: Option<std::sync::mpsc::Receiver<((usize, usize), Vec<Line<'static>>)>>,
    selection: Option<(usize, Position, Position)>,
    modal_selection: Option<(Position, Position)>,
    copy: bool,
    clipboard: Option<String>,
    tabs: Tabs,
    note: String,
    busy: Option<String>,
    checked: std::collections::BTreeSet<(String, usize)>,
    opened: std::collections::BTreeSet<(String, usize)>,
    detail_rows: Vec<Option<usize>>,
    detail_folds: Vec<bool>,
    detail_notes: Vec<Option<usize>>,
    detail_parts: Vec<Option<usize>>,
    verdict_row: Option<(usize, u16, u16)>,
    detail_area: Rect,
    menu: Option<(Mode, usize)>,
    input: Option<Input>,
    finish: Option<Finish>,
    threads: Vec<mrs::Thread>,
    threads_rx: Option<std::sync::mpsc::Receiver<(Vec<mrs::Thread>, Option<(String, String)>)>>,
    dates: Option<(String, String)>,
    cursor: usize,
    help: bool,
    picture_area: Rect,
    open_at: Option<(usize, u32)>,
    hide_diff: bool,
    palette: Option<(String, usize)>,
    palette_cursor: usize,
    pending: Option<KeyCode>,
    back_area: Rect,
    reclick: bool,
    picked_lines: Option<(usize, usize)>,
    action_buttons: Vec<(Rect, KeyCode)>,
    diff_width: u16,
    reset_armed: bool,
    screen: Rect,
    in_tab: bool,
    row_heights: Vec<u16>,
    buttons: Vec<(Rect, KeyCode)>,
    confirm: Option<String>,
    job: Option<std::process::Child>,
    picker: Picker,
    images: HashMap<PathBuf, Option<StatefulProtocol>>,
}

impl App {
    fn new(bundle: Bundle, picker: Picker) -> App {
        let rows = Vec::new();
        let state = bundle.load_state();
        let (sender, incoming) = std::sync::mpsc::channel();
        let work: Vec<_> = bundle.groups.iter().flatten().map(|&(fi, hi)| ((fi, hi), bundle.files[fi].path.clone(), bundle.files[fi].hunks[hi].lines.clone())).collect();
        std::thread::spawn(move || {
            for (key, path, lines) in work {
                if sender.send((key, hunk_lines(&path, &lines, true))).is_err() {
                    break;
                }
            }
        });
        let mut app = App {
            bundle,
            state,
            rows,
            sel: 0,
            scroll: 0,
            evidence: 0,
            zoom: false,
            panes: Panes::load(),
            focus: 0,
            detail_scroll: 0,
            list: ListState::default(),
            areas: [Rect::default(); 3],
            drag: None,
            hover: None,
            handles: Vec::new(),
            right_area: Rect::default(),
            upper_area: Rect::default(),
            side: false,
            diffed: Diff::default(),
            highlighted: HashMap::new(),
            incoming: Some(incoming),
            selection: None,
            modal_selection: None,
            copy: false,
            clipboard: None,
            tabs: Tabs::default(),
            note: String::new(),
            busy: None,
            checked: Default::default(),
            opened: Default::default(),
            detail_rows: Vec::new(),
            detail_folds: Vec::new(),
            detail_notes: Vec::new(),
            detail_parts: Vec::new(),
            verdict_row: None,
            detail_area: Rect::default(),
            menu: None,
            input: None,
            finish: None,
            threads: Vec::new(),
            threads_rx: None,
            dates: None,
            cursor: 0,
            help: false,
            picture_area: Rect::default(),
            open_at: None,
            hide_diff: false,
            palette: None,
            palette_cursor: 0,
            pending: None,
            back_area: Rect::default(),
            reclick: false,
            picked_lines: None,
            action_buttons: Vec::new(),
            diff_width: 0,
            reset_armed: false,
            screen: Rect::default(),
            in_tab: false,
            row_heights: Vec::new(),
            buttons: Vec::new(),
            confirm: None,
            job: None,
            picker,
            images: HashMap::new(),
        };
        app.fetch_threads();
        app.rebuild_rows();
        app.select(0);
        app
    }

    fn fetch_threads(&mut self) {
        let Some(source) = self.bundle.review.sources.first().filter(|s| !s.repo.is_empty() && !s.reference.is_empty()) else { return };
        let (repo, reference) = (source.repo.clone(), source.reference.clone());
        let cache = self.bundle.dir.join("threads.json");
        if self.threads.is_empty() && self.dates.is_none() {
            let saved = std::fs::read_to_string(&cache).ok().and_then(|text| serde_json::from_str(&text).ok());
            if let Some((threads, dates)) = saved {
                (self.threads, self.dates) = (threads, dates);
                self.rebuild_rows();
            }
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let fetched = std::thread::scope(|scope| {
                let dates = scope.spawn(|| mrs::dates(&repo, &reference));
                (mrs::threads(&repo, &reference), dates.join().ok().flatten())
            });
            if !cfg!(test) {
                let _ = std::fs::write(cache, serde_json::to_string(&fetched).unwrap_or_default());
            }
            sender.send(fetched)
        });
        self.threads_rx = Some(receiver);
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        if !self.bundle.review.summary.is_empty() {
            rows.push(Row::Overview);
        }
        for g in 0..self.bundle.groups.len() {
            rows.push(Row::Group(g));
            rows.extend(items_in(&self.bundle, Some(g)));
        }
        rows.extend(items_in(&self.bundle, None));
        if !self.threads.is_empty() || !self.state.notes.is_empty() || self.threads_rx.is_some() {
            rows.extend([Row::Spacer, Row::Heading]);
        }
        rows.extend((0..self.threads.len()).map(Row::Thread));
        rows.extend((0..self.state.notes.len()).map(Row::Note));
        self.rows = rows;
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
        self.diffed = self.build_diff();
    }

    fn locate(&self, file: &str, line: u32, old: bool) -> Option<usize> {
        self.bundle.groups.iter().position(|group| {
            group.iter().any(|&(fi, hi)| {
                let hunk = &self.bundle.files[fi].hunks[hi];
                let count = |skip: char| hunk.lines.iter().filter(|l| !l.starts_with([skip, '\\'])).count() as u32;
                let range = if old { hunk.old_start..hunk.old_start + count('+') } else { hunk.new_start..hunk.new_start + count('-') };
                self.bundle.files[fi].path == file && range.contains(&line)
            })
        })
    }

    fn anchor(&self) -> Option<(String, u32, bool)> {
        match *self.rows.get(self.sel)? {
            Row::Group(_) | Row::Heading | Row::Spacer | Row::Overview => None,
            Row::Item(i) => self.bundle.review.items[i].at.as_ref().map(|at| (at.file.clone(), at.line, false)),
            Row::Thread(i) => {
                let thread = self.threads.get(i)?;
                Some((thread.file.clone()?, thread.line?, thread.old))
            }
            Row::Note(i) => {
                let note = self.state.notes.get(i)?;
                match (&note.file, note.line) {
                    (Some(file), Some(line)) => Some((file.clone(), line, note.old)),
                    _ => {
                        let item = self.bundle.review.items.iter().find(|item| Some(&item.id) == note.item.as_ref())?;
                        item.at.as_ref().map(|at| (at.file.clone(), at.line, false))
                    }
                }
            }
        }
    }

    fn start_note(&mut self, whole: bool) {
        let mut note = Note::default();
        let (mut title, mut text, mut edit) = ("Comment on the whole merge request".to_string(), String::new(), None);
        let ranged = self.picked_lines.take().filter(|_| !whole && self.focus == 2 && !self.diffed.2.is_empty());
        match self.rows.get(self.sel).copied() {
            _ if whole => {}
            _ if ranged.is_some() => {
                let last = self.diffed.2.len() - 1;
                let (from, to) = ranged.map_or((0, 0), |(from, to)| (from.min(last), to.min(last)));
                let mark = self.diffed.2[to];
                let removed = mark.sign == '-';
                let line_of = |mark: &Mark| if removed { mark.old } else { mark.new }.max(1);
                let first = self.diffed.2[from..=to].iter().find(|other| other.file == mark.file).map_or(line_of(&mark), line_of);
                note.file = Some(self.bundle.files[mark.file].path.clone());
                (note.line, note.old) = (Some(line_of(&mark)), removed);
                note.start = Some(first).filter(|first| *first < line_of(&mark));
                let lines = note.start.map_or(String::new(), |first| format!("{first}-"));
                title = format!("Comment on {}:{lines}{}", self.bundle.files[mark.file].path, line_of(&mark));
            }
            Some(Row::Thread(i)) => {
                let thread = &self.threads[i];
                note.reply_to = Some(thread.id.clone());
                title = format!("Reply to {}", thread.notes.first().map_or("", |n| n.0.as_str()));
            }
            Some(Row::Note(i)) => {
                (note, text, edit) = (self.state.notes[i].clone(), self.state.notes[i].text.clone(), Some(i));
                title = "Edit your comment".into();
            }
            _ if self.focus == 2 && !self.diffed.2.is_empty() => {
                let mark = self.diffed.2[self.cursor.min(self.diffed.2.len() - 1)];
                let removed = mark.sign == '-';
                note.file = Some(self.bundle.files[mark.file].path.clone());
                note.line = Some(if removed { mark.old } else { mark.new }.max(1));
                note.old = removed;
                title = format!("Comment on {}:{}", self.bundle.files[mark.file].path, note.line.unwrap_or(1));
            }
            Some(Row::Item(i)) => {
                let item = &self.bundle.review.items[i];
                note.item = Some(item.id.clone());
                (note.file, note.line) = item.at.as_ref().map_or((None, None), |at| (Some(at.file.clone()), Some(at.line)));
                title = format!("Comment on {}", item.id);
            }
            _ => {}
        }
        let cursor = text.chars().count();
        self.input = Some(Input { title, text, cursor, note, edit });
    }

    fn submit_note(&mut self, input: Input) {
        let text = input.text.trim().to_string();
        match (input.edit, text.is_empty()) {
            (Some(i), true) => drop(self.state.notes.remove(i)),
            (Some(i), false) => self.state.notes[i].text = text,
            (None, true) => return,
            (None, false) => self.state.notes.push(Note { text, posted: false, ..input.note }),
        }
        self.state.finished = None;
        let _ = self.bundle.save_state(&self.state);
        self.rebuild_rows();
    }

    fn points(&self) -> Vec<Point> {
        let mut points = Vec::new();
        for item in &self.bundle.review.items {
            let verdict = self.state.verdicts.get(&item.id).map(String::as_str);
            let raise = matches!((item.kind, verdict), (Kind::Finding, Some("agree")) | (Kind::Decision, Some("dismiss")));
            if raise && !self.state.published.contains(&item.id) {
                let (file, line) = item.at.as_ref().map_or((None, None), |at| (Some(at.file.clone()), Some(at.line)));
                let quoted = item.body.trim().lines().map(|line| match line.strip_prefix("# ") {
                    Some(label) => format!("> **{label}**"),
                    None => format!("> {line}").trim_end().to_string(),
                });
                let mut text = format!("> **{}**\n>\n{}", item.title, quoted.collect::<Vec<_>>().join("\n"));
                for note in self.state.notes.iter().filter(|note| !note.posted && note.item.as_ref() == Some(&item.id)) {
                    text += &format!("\n\n{}", note.text);
                }
                points.push(Point { heading: String::new(), text, file, line, start: None, old: false, reply_to: None, item: Some(item.id.clone()) });
            }
        }
        let raised: Vec<&String> = points.iter().filter_map(|point| point.item.as_ref()).collect();
        let mut loose = Vec::new();
        for note in self.state.notes.iter().filter(|note| !note.posted && !note.item.as_ref().is_some_and(|id| raised.contains(&id))) {
            let heading = note.item.as_ref().filter(|_| note.file.is_none()).map_or(String::new(), |id| format!("On {id}"));
            loose.push(Point { heading, text: note.text.clone(), file: note.file.clone(), line: note.line, start: note.start, old: note.old, reply_to: note.reply_to.clone(), item: None });
        }
        points.extend(loose);
        points
    }

    fn compose(&self, points: &[&Point]) -> String {
        let source = self.bundle.review.sources.first();
        let (url, head) = source.map_or(("", ""), |s| (s.url.as_str(), s.head_sha.as_str()));
        let mut body = format!("Reviewed at `{head:.8}`.");
        if points.is_empty() {
            return body + " No comments.";
        }
        for point in points {
            let place = match (&point.file, point.line) {
                (Some(file), Some(line)) => {
                    let lines = point.start.map_or(line.to_string(), |start| format!("{start}-{line}"));
                    match mrs::file_url(url, head, file, point.start.unwrap_or(line)).filter(|_| !point.old) {
                        Some(link) => format!("[`{file}:{lines}`]({link})"),
                        None => format!("`{file}:{lines}`"),
                    }
                }
                _ => String::new(),
            };
            let lead = match (point.heading.is_empty(), place.is_empty()) {
                (true, true) => String::new(),
                (true, false) if point.text.starts_with("> ") => format!("{place}\n  "),
                (true, false) => format!("{place}: "),
                (false, true) => format!("**{}**\n  ", point.heading),
                (false, false) => format!("**{}** ({place})\n  ", point.heading),
            };
            body += &format!("\n\n- {lead}{}", point.text.trim().replace('\n', "\n  "));
        }
        body
    }

    fn item(&self) -> Option<&Item> {
        match self.rows[self.sel] {
            Row::Item(i) => Some(&self.bundle.review.items[i]),
            _ => None,
        }
    }

    fn group(&self) -> Option<usize> {
        match self.rows[self.sel] {
            Row::Group(g) => Some(g),
            Row::Item(i) => self.bundle.review.items[i].section,
            Row::Heading | Row::Spacer | Row::Overview => None,
            Row::Thread(_) | Row::Note(_) => {
                let located = self.anchor().and_then(|(file, line, old)| self.locate(&file, line, old));
                let item = match self.rows[self.sel] {
                    Row::Note(i) => self.state.notes.get(i).and_then(|note| note.item.clone()),
                    _ => None,
                };
                located.or_else(|| self.bundle.review.items.iter().find(|i| Some(&i.id) == item.as_ref())?.section)
            }
        }
    }

    fn select(&mut self, sel: usize) {
        self.sel = sel.min(self.rows.len().saturating_sub(1));
        self.evidence = 0;
        self.detail_scroll = 0;
        self.diffed = self.build_diff();
        self.cursor = self.diffed.1;
        self.scroll = self.diffed.1.saturating_sub(8) as u16;
    }

    fn build_diff(&self) -> Diff {
        let mut lines = Vec::new();
        let mut marks = Vec::new();
        let mut anchor = 0;
        let at = self.anchor();
        let Some(group) = self.group().and_then(|g| self.bundle.groups.get(g)) else {
            return (vec![Line::raw("This is not tied to one part of the diff.")], 0, marks);
        };
        let mine: Vec<(&str, u32, u32, bool)> = self.state.notes.iter().filter_map(|n| Some((n.file.as_deref()?, n.start.or(n.line)?, n.line?, n.old))).collect();
        let theirs: Vec<(&str, u32, bool)> = self.threads.iter().filter_map(|t| Some((t.file.as_deref()?, t.line?, t.old))).collect();
        let mut last_file = usize::MAX;
        for &(fi, hi) in group {
            let file = &self.bundle.files[fi];
            let hunk = &file.hunks[hi];
            if fi != last_file {
                lines.push(Line::styled(file.path.clone(), Style::new().add_modifier(Modifier::BOLD)));
                marks.push(Mark { file: fi, new: hunk.new_start, old: hunk.old_start, sign: ' ' });
                last_file = fi;
            }
            lines.push(Line::styled(hunk.header.clone(), Style::new().fg(accent())));
            marks.push(Mark { file: fi, new: hunk.new_start, old: hunk.old_start, sign: ' ' });
            let code = self.highlighted.get(&(fi, hi)).cloned().unwrap_or_else(|| hunk_lines(&file.path, &hunk.lines, false));
            let (mut new_line, mut old_line) = (hunk.new_start, hunk.old_start);
            for (text, mut line) in hunk.lines.iter().zip(code) {
                let sign = text.chars().next().unwrap_or(' ');
                let removed = sign == '-';
                let here = (file.path.as_str(), if removed { old_line } else { new_line }, removed);
                let noted = mine.iter().any(|(path, from, to, old)| *path == here.0 && *old == here.2 && (*from..=*to).contains(&here.1));
                let marker = match (noted, theirs.contains(&here)) {
                    (true, _) => Some(Color::Yellow),
                    (_, true) => Some(accent()),
                    _ => None,
                };
                if let (Some(color), Some(first)) = (marker, line.spans.first_mut()) {
                    first.content = format!("{sign}●").into();
                    first.style = first.style.fg(color);
                }
                if sign != '\\' && at.as_ref().is_some_and(|(f, l, old)| f == here.0 && *l == here.1 && *old == here.2) {
                    anchor = lines.len();
                    for span in &mut line.spans {
                        span.style = span.style.bg(Color::Rgb(84, 72, 24));
                    }
                    if let Some(sign) = line.spans.first_mut() {
                        sign.style = sign.style.fg(Color::Yellow);
                    }
                }
                for row in wrap_code(line, self.diff_width as usize) {
                    lines.push(row);
                    marks.push(Mark { file: fi, new: new_line, old: old_line, sign });
                }
                match sign {
                    '-' => old_line += 1,
                    '+' => new_line += 1,
                    '\\' => {}
                    _ => (new_line, old_line) = (new_line + 1, old_line + 1),
                }
            }
        }
        (lines, anchor, marks)
    }

    fn absorb_highlights(&mut self) {
        match self.threads_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok((threads, dates))) => {
                let selected = self.rows.get(self.sel).copied();
                (self.threads, self.threads_rx) = (threads, None);
                self.dates = dates.or(self.dates.take());
                self.rebuild_rows();
                let kept = selected.and_then(|row| self.rows.iter().position(|other| *other == row));
                self.sel = kept.unwrap_or(self.sel).min(self.rows.len().saturating_sub(1));
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.threads_rx = None;
                self.rebuild_rows();
            }
            _ => {}
        }
        let Some(incoming) = &self.incoming else { return };
        let current = self.group().and_then(|g| self.bundle.groups.get(g)).cloned().unwrap_or_default();
        let mut touched = false;
        loop {
            match incoming.try_recv() {
                Ok((key, lines)) => {
                    touched |= current.contains(&key);
                    self.highlighted.insert(key, lines);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.incoming = None;
                    break;
                }
            }
        }
        if touched {
            self.diffed = self.build_diff();
        }
    }

    fn file_at(&self, repo: &str, sha: &str, file: &str, folder: &str, absent: bool) -> Option<PathBuf> {
        let github = self.bundle.review.sources.iter().any(|source| source.repo == repo && source.reference.starts_with('#'));
        file_at(&self.bundle.dir, github, repo, sha, file, folder, absent)
    }

    fn prefetch(&self, files: &[(usize, u32)]) -> impl FnOnce() + Send + use<> {
        let dir = self.bundle.dir.clone();
        let sources = &self.bundle.review.sources;
        let jobs: Vec<_> = files
            .iter()
            .filter_map(|&(fi, _)| {
                let file = &self.bundle.files[fi];
                let source = sources.iter().find(|s| s.id == file.source).or(sources.first())?;
                let header = file.hunks.first().map_or("", |hunk| hunk.header.as_str());
                let (added, deleted) = (header.starts_with("@@ -0,0 "), header.contains(" +0,0 @@"));
                let base = if added { None } else { self.base_sha(&source.repo, &source.reference, &source.head_sha) };
                Some((source.reference.starts_with('#'), source.repo.clone(), source.head_sha.clone(), base, file.path.clone(), deleted))
            })
            .collect();
        move || {
            std::thread::scope(|scope| {
                for (github, repo, sha, base, path, deleted) in &jobs {
                    scope.spawn(|| file_at(&dir, *github, repo, sha, path, "files", *deleted));
                    scope.spawn(|| base.as_ref().and_then(|base| file_at(&dir, *github, repo, base, path, "base", false)));
                }
            })
        }
    }

    fn base_sha(&self, repo: &str, reference: &str, head: &str) -> Option<String> {
        let cached = self.bundle.dir.join("base_sha");
        if let Ok(sha) = std::fs::read_to_string(&cached) {
            return Some(sha.trim().to_string());
        }
        let sha = mrs::base_sha(repo, reference, head)?;
        let _ = std::fs::write(cached, &sha);
        Some(sha)
    }

    fn session(&self) -> Option<Session> {
        let saved = std::fs::read_to_string(self.bundle.dir.join("session.json")).ok()?;
        serde_json::from_str(&saved).ok()
    }

    fn options(&self) -> Vec<(String, Target)> {
        let agents = mrs::agents();
        let session = self.session();
        let saved = |agent: &mrs::Agent| session.as_ref().is_some_and(|s| s.agent == agent.name && !s.id.is_empty());
        let existing = |agent: &mrs::Agent| {
            let tab = self.tabs.terms.iter().position(|tab| tab.name == agent.name);
            match (agent.modal, tab) {
                (true, Some(tab)) => Some(format!("Tab {}: {}, already open", tab + 1, agent.name)),
                (true, None) if agent.resume.is_some() && saved(agent) => Some(format!("{}: pick up the earlier conversation", agent.name)),
                (false, _) if agent.send.is_some() && session.as_ref().is_some_and(|s| s.agent == agent.name) => {
                    Some(format!("{}: the one already running", agent.name))
                }
                _ => None,
            }
        };
        let fresh = |agent: &mrs::Agent| match agent.modal {
            true => format!("New {} in a tab here", agent.name),
            false => format!("New agent in a {}", agent.name),
        };
        let reuse = agents.iter().enumerate().filter_map(|(i, agent)| Some((existing(agent)?, Target::Reuse(i))));
        let new = agents.iter().enumerate().map(|(i, agent)| (fresh(agent), Target::New(i)));
        reuse.chain(new).collect()
    }

    fn diff_shown(&self) -> bool {
        !self.hide_diff && self.group().is_some_and(|g| g < self.bundle.groups.len())
    }

    fn open_target(&self) -> Option<(usize, u32)> {
        if self.open_at.is_some() {
            return self.open_at;
        }
        let (_, anchor, marks) = &self.diffed;
        let at = if self.focus == 2 { self.cursor } else if self.anchor().is_some() { *anchor } else { self.scroll as usize };
        marks.get(at).or(marks.first()).map(|mark| (mark.file, mark.new.max(1)))
    }

    fn section_files(&self) -> Vec<(usize, u32)> {
        let group = self.group().and_then(|g| self.bundle.groups.get(g)).cloned().unwrap_or_default();
        let mut files: Vec<(usize, u32)> = Vec::new();
        for (fi, hi) in group {
            if !files.iter().any(|&(file, _)| file == fi) {
                files.push((fi, self.bundle.files[fi].hunks[hi].new_start.max(1)));
            }
        }
        files
    }

    fn command(&self, script: &str, mode: Mode, fresh: bool) -> std::process::Command {
        let place = self.open_target().map(|(file, line)| (&self.bundle.files[file], line));
        let sources = &self.bundle.review.sources;
        let source = place.and_then(|(file, _)| sources.iter().find(|s| s.id == file.source)).or(sources.first());
        let (file, line) = place.map_or(("", 0), |(file, line)| (file.path.as_str(), line));
        let (url, sha, repo) = source.map_or(("", "", ""), |s| (s.url.as_str(), s.head_sha.as_str(), s.repo.as_str()));
        let file_url = mrs::file_url(url, sha, file, line).unwrap_or_default();
        let dir = self.bundle.dir.to_string_lossy();
        let looking_at = match self.item() {
            Some(item) => format!("{} \"{}\" ({file}:{line})", item.id, item.title),
            None => format!("{file}:{line}"),
        };
        let section = self.group().and_then(|g| self.bundle.review.sections.get(g));
        let picked: Vec<String> = self
            .checked
            .iter()
            .filter_map(|(id, n)| {
                let item = self.bundle.review.items.iter().find(|item| item.id == *id)?;
                Some((id, item.title.as_str(), item.evidence.get(*n)?.caption.as_str()))
            })
            .fold(Vec::<(&String, String)>::new(), |mut claims, (id, title, caption)| {
                match claims.last_mut().filter(|(last, _)| *last == id) {
                    Some((_, text)) => *text += &format!("; {caption}"),
                    None => claims.push((id, format!("{id} \"{title}\": {caption}"))),
                }
                claims
            })
            .into_iter()
            .map(|(_, text)| text)
            .collect();
        let target = match (self.item(), section) {
            _ if !picked.is_empty() => format!("these claims: {}", picked.join("; ")),
            (Some(item), _) => format!("{} \"{}\"", item.id, item.title),
            (None, Some(section)) => format!("every finding in the section \"{}\" that has no observed evidence", section.title),
            _ => "every finding that has no observed evidence".into(),
        };
        let guide = format!("the revui guide at {dir} (run `revui skill` first)");
        let prompt = match (mode, fresh) {
            (Mode::Ask, true) => format!("I'm reviewing {url} with {guide}. About {looking_at}: "),
            (Mode::Ask, false) => format!("About {looking_at}: "),
            (Mode::Verify, true) => format!("Verify {target} in {guide}. Reproduce it locally, off my screen."),
            (Mode::Verify, false) => format!("Verify {target}. Reproduce it locally, off my screen."),
            (Mode::Artifact, true) => format!("Make an artifact for {target} in {guide}. If it needs a run, recreate it locally, off my screen."),
            (Mode::Artifact, false) => format!("Make an artifact for {target}. If it needs a run, recreate it locally, off my screen."),
        };
        let (session, session_id) = self.session().map_or((String::new(), String::new()), |s| (s.handle, s.id));
        let first_hunk = place.and_then(|(file, _)| file.hunks.first()).map_or("", |hunk| hunk.header.as_str());
        let (added, deleted) = (first_hunk.starts_with("@@ -0,0 "), first_hunk.contains(" +0,0 @@"));
        let head_file = match script.contains("REVUI_HEAD_FILE") && !file.is_empty() {
            true => self.file_at(repo, sha, file, "files", deleted).unwrap_or_default(),
            false => PathBuf::new(),
        };
        let reference = source.map_or("", |s| s.reference.as_str());
        let base_file = match script.contains("REVUI_BASE_FILE") && !file.is_empty() {
            true => {
                let base = if added { None } else { self.base_sha(repo, reference, sha) };
                let fetched = base.and_then(|base| self.file_at(repo, &base, file, "base", false));
                fetched.or_else(|| self.file_at(repo, "", file, "base", true)).unwrap_or_default()
            }
            false => PathBuf::new(),
        };
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", script])
            .env("REVUI_CLAUDE", mrs::claude())
            .env("REVUI_URL", url)
            .env("REVUI_DIR", &*dir)
            .env("REVUI_REPO", repo)
            .env("REVUI_SHA", sha)
            .env("REVUI_FILE", file)
            .env("REVUI_LINE", line.to_string())
            .env("REVUI_FILE_URL", file_url)
            .env("REVUI_ITEM", self.item().map_or("", |i| i.id.as_str()))
            .env("REVUI_SESSION", session)
            .env("REVUI_SESSION_ID", session_id)
            .env("REVUI_HEAD_FILE", head_file)
            .env("REVUI_BASE_FILE", base_file)
            .env("REVUI_SOCKET", std::env::temp_dir().join(format!("revui-{}.sock", std::process::id())))
            .env("REVUI_PROMPT", prompt);
        command
    }

    fn evidence_path(&self) -> Option<PathBuf> {
        let evidence = &self.item()?.evidence;
        let selected = evidence.get(self.evidence).and_then(|e| e.path.as_ref());
        let path = selected.or_else(|| evidence.iter().find_map(|e| e.path.as_ref()))?;
        Some(self.bundle.dir.join(path))
    }

    fn detail_line(&self, column: u16, row: u16) -> Option<(usize, u16)> {
        let area = self.detail_area;
        let inside = area.contains(Position::new(column, row)) && row > area.y && column > area.x;
        inside.then(|| (self.detail_scroll as usize + (row - area.y - 1) as usize, column - area.x - 1))
    }

    fn detail_button(&self, column: u16, row: u16) -> Option<KeyCode> {
        let (line, at) = self.detail_line(column, row)?;
        let (button_row, yes, no) = self.verdict_row.filter(|(button_row, _, _)| *button_row == line)?;
        match at {
            at if at < yes => Some(KeyCode::Char('a')),
            at if at > yes && at <= yes + no => Some(KeyCode::Char('x')),
            _ => None,
        }
        .filter(|_| button_row == line)
    }

    /// The guide row of the part whose entry in the overview's reading order is under the pointer.
    fn detail_part(&self, column: u16, row: u16) -> Option<usize> {
        let (line, _) = self.detail_line(column, row)?;
        let part = (*self.detail_parts.get(line)?)?;
        self.rows.iter().position(|row| matches!(row, Row::Group(g) if *g == part))
    }

    /// The comment of yours under the pointer in the detail pane, as an index into the saved ones.
    fn detail_note(&self, column: u16, row: u16) -> Option<usize> {
        let (line, _) = self.detail_line(column, row)?;
        let nth = (*self.detail_notes.get(line)?)?;
        let id = &self.item()?.id;
        self.state.notes.iter().enumerate().filter(|(_, note)| note.item.as_ref() == Some(id)).map(|(i, _)| i).nth(nth)
    }

    /// The @name being typed at the cursor (where its @ is) and who it could be.
    // ponytail: only people already on this merge request; ask the host for project members if that proves thin
    fn mention(&self, input: &Input) -> Option<(usize, Vec<String>)> {
        let chars: Vec<char> = input.text.chars().collect();
        let end = input.cursor.min(chars.len());
        let mut at = end;
        while at > 0 && (chars[at - 1].is_alphanumeric() || matches!(chars[at - 1], '.' | '_' | '-')) {
            at -= 1;
        }
        if at == 0 || chars[at - 1] != '@' || (at > 1 && !chars[at - 2].is_whitespace()) {
            return None;
        }
        let typed: String = chars[at..end].iter().collect::<String>().to_lowercase();
        let authors = self.bundle.review.sources.iter().map(|source| source.author.as_str());
        let speakers = self.threads.iter().flat_map(|thread| thread.notes.iter().map(|note| note.0.as_str()));
        let mut names: Vec<String> = Vec::new();
        for name in authors.chain(speakers).filter_map(|name| name.strip_prefix('@')) {
            if name.to_lowercase().starts_with(&typed) && name.to_lowercase() != typed && !names.iter().any(|known| known == name) {
                names.push(name.to_string());
            }
        }
        (!names.is_empty()).then_some((at - 1, names))
    }

    /// A popup is open over the review. Terminal images draw above text, so they are left out meanwhile.
    fn layered(&self) -> bool {
        self.confirm.is_some() || self.menu.is_some() || self.palette.is_some() || self.input.is_some() || self.finish.is_some() || self.help
    }

    fn artifact_at_cursor(&self) -> bool {
        self.focus != 2 && self.evidence_path().is_some()
    }

    fn ask_to_generate(&mut self) {
        match self.bundle.review.sources.first().filter(|s| !s.url.is_empty()) {
            Some(_) => self.confirm = Some(mrs::generate_command()),
            None => self.note = "This bundle names no merge request, so there is nothing to generate from.".into(),
        }
    }

    fn generate(&mut self, command: String) {
        let dir = &self.bundle.dir;
        if !self.bundle.review.sections.is_empty() {
            let _ = std::fs::copy(dir.join("review.json"), dir.join("review.before.json"));
        }
        let mut sh = std::process::Command::new("sh");
        sh.args(["-c", &command]).env("REVUI_CLAUDE", mrs::claude()).env("REVUI_URL", &self.bundle.review.sources[0].url).env("REVUI_DIR", dir);
        let spawned = match mrs::generate_settings().1 {
            true => {
                self.tabs.close("guide");
                self.tabs.run("guide", &sh, self.screen).map(|_| None)
            }
            false => {
                let log = std::fs::File::create(dir.join("generate.log"));
                let child = log.and_then(|log| sh.stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn());
                child.map(Some).map_err(|e| e.to_string())
            }
        };
        match spawned {
            Ok(child) => {
                (self.job, self.in_tab) = (child, self.tabs.has("guide"));
                self.note.clear();
                let doing = match (self.bundle.successor.is_some(), self.bundle.review.sections.is_empty()) {
                    (true, _) => "Updating the guide",
                    (_, false) => "Regenerating the guide",
                    _ => "Generating a guide",
                };
                self.busy = Some(format!("{doing}. This screen reloads when it lands"));
            }
            Err(e) => self.note = format!("Could not start the generator: {e}"),
        }
    }

    fn tick(&mut self) {
        if self.in_tab && !self.tabs.has("guide") {
            (self.in_tab, self.busy) = (false, None);
            self.note = "The guide tab closed before a new guide landed.".into();
        }
        let failed = match &mut self.job {
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => Some(!status.success()),
                _ => None,
            },
            None => None,
        };
        if let Some(failed) = failed {
            self.job = None;
            if failed {
                self.busy = None;
                let log = self.bundle.dir.join("generate.log");
                let output = std::fs::read_to_string(&log).unwrap_or_default();
                let last = output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
                self.note = format!("The generator failed: {last} (log: {})", log.display());
            }
        }
    }

    fn toggle_evidence(&mut self, details: bool) {
        let Some(key) = self.item().filter(|item| self.evidence < item.evidence.len()).map(|item| (item.id.clone(), self.evidence)) else {
            return;
        };
        let set = if details { &mut self.opened } else { &mut self.checked };
        if !set.remove(&key) {
            set.insert(key);
        }
    }

    fn step(&mut self, delta: i32) {
        if self.focus == 2 {
            let last = self.diffed.0.len().saturating_sub(1) as i32;
            self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
            let height = self.areas[2].height.saturating_sub(2).max(1) as usize;
            let scroll = self.scroll as usize;
            self.scroll = if self.cursor < scroll { self.cursor } else if self.cursor >= scroll + height { self.cursor + 1 - height } else { scroll } as u16;
            return;
        }
        let count = self.item().map_or(0, |item| item.evidence.len()) as i32;
        let next = self.evidence as i32 + delta;
        let on_evidence = matches!(self.focus, 1 | 3);
        if !on_evidence || next < 0 || next >= count {
            return if self.focus == 3 { () } else { self.scroll_pane(self.focus, delta) };
        }
        self.evidence = next as usize;
        let height = self.detail_area.height.saturating_sub(2).max(1);
        let rows = detail(self, self.detail_area.width.saturating_sub(2).max(10) as usize);
        let first = rows.iter().position(|(_, n)| *n == Some(self.evidence)).unwrap_or(0) as u16;
        let last = rows.iter().rposition(|(_, n)| *n == Some(self.evidence)).unwrap_or(0) as u16;
        if first < self.detail_scroll {
            self.detail_scroll = first;
        } else if last >= self.detail_scroll + height {
            self.detail_scroll = (last + 1).saturating_sub(height).min(first);
        }
    }

    fn scroll_pane(&mut self, pane: usize, delta: i32) {
        let moved = |v: u16| (v as i32 + delta).max(0) as u16;
        match pane {
            0 if delta.abs() == 1 => {
                let count = self.rows.len().max(1) as i32;
                let mut next = (self.sel as i32 + delta).rem_euclid(count);
                while matches!(self.rows.get(next as usize), Some(Row::Spacer | Row::Heading)) {
                    next = (next + delta).rem_euclid(count);
                }
                self.select(next as usize);
            }
            0 => {
                let last = self.rows.len().saturating_sub(1) as i32;
                let mut next = (self.sel as i32 + delta.signum()).clamp(0, last);
                while matches!(self.rows.get(next as usize), Some(Row::Spacer | Row::Heading)) && next > 0 && next < last {
                    next += delta.signum();
                }
                if !matches!(self.rows.get(next as usize), Some(Row::Spacer | Row::Heading)) {
                    self.select(next as usize);
                }
            }
            1 => self.detail_scroll = moved(self.detail_scroll),
            _ => {
                self.scroll = moved(self.scroll).min(self.diffed.0.len().saturating_sub(1) as u16);
                let height = self.areas[2].height.saturating_sub(2).max(1) as usize;
                self.cursor = self.cursor.clamp(self.scroll as usize, self.scroll as usize + height - 1);
            }
        }
    }

    fn mouse(&mut self, m: MouseEvent) {
        let [guide, _, diff] = self.areas;
        let had_focus = self.focus;
        let over = self.areas.iter().position(|a| a.contains(Position::new(m.column, m.row)));
        let grabbed = self.handles.iter().find(|(seam, _)| seam.contains(Position::new(m.column, m.row))).map(|&(_, handle)| handle);
        let inside = |pane: usize| {
            let Some(&area) = self.areas.get(pane) else { return Position::new(m.column, m.row) };
            let column = m.column.clamp(area.x + 1, area.right().saturating_sub(2).max(area.x + 1));
            Position::new(column, m.row.clamp(area.y + 1, area.bottom().saturating_sub(2).max(area.y + 1)))
        };
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                self.selection = None;
                let delta = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 };
                over.into_iter().for_each(|pane| self.scroll_pane(pane, delta));
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.picked_lines = None;
                if self.back_area.contains(Position::new(m.column, m.row)) {
                    self.pending = Some(KeyCode::Char('q'));
                } else if let Some(&(_, code)) = self.action_buttons.iter().find(|(button, _)| button.contains(Position::new(m.column, m.row))) {
                    self.pending = Some(code);
                } else if grabbed.is_some() {
                    self.drag = grabbed;
                } else if self.picture_area.contains(Position::new(m.column, m.row)) {
                    self.pending = (had_focus == 3).then_some(KeyCode::Char('o'));
                    self.focus = 3;
                } else if let Some(code) = self.detail_button(m.column, m.row) {
                    self.focus = 1;
                    self.pending = Some(code);
                } else if let Some(row) = self.detail_part(m.column, m.row) {
                    self.focus = 0;
                    self.select(row);
                } else if let Some(note) = self.detail_note(m.column, m.row) {
                    self.focus = 1;
                    let (text, saved) = (self.state.notes[note].text.clone(), self.state.notes[note].clone());
                    self.input = Some(Input { title: "Edit your comment".into(), cursor: text.chars().count(), text, note: saved, edit: Some(note) });
                } else if let Some(pane) = over {
                    self.focus = pane;
                    self.selection = Some((pane, inside(pane), inside(pane)));
                    let line = self.detail_scroll as usize + m.row.saturating_sub(self.detail_area.y + 1) as usize;
                    let on_text = self.detail_area.contains(Position::new(m.column, m.row)) && m.row > self.detail_area.y;
                    if let (true, Some(&Some(n))) = (on_text, self.detail_rows.get(line)) {
                        self.evidence = n;
                        self.toggle_evidence(self.detail_folds.get(line) == Some(&true));
                    }
                    let (mut row, mut below) = (self.list.offset(), m.row.saturating_sub(guide.y + 1));
                    while row < self.rows.len() && below >= self.row_heights.get(row).copied().unwrap_or(1) {
                        below -= self.row_heights.get(row).copied().unwrap_or(1);
                        row += 1;
                    }
                    if pane == 0 && m.row > guide.y && row < self.rows.len() && !matches!(self.rows[row], Row::Spacer | Row::Heading) {
                        self.reclick = had_focus == 0 && self.sel == row;
                        self.select(row);
                    }
                    if pane == 2 && m.row > diff.y {
                        let line = self.scroll as usize + (m.row - diff.y - 1) as usize;
                        self.cursor = line.min(self.diffed.0.len().saturating_sub(1));
                    }
                } else {
                    self.selection = Some((usize::MAX, inside(usize::MAX), inside(usize::MAX)));
                }
            }
            MouseEventKind::Moved => self.hover = grabbed,
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Some(handle) => {
                    let (right, upper) = (self.right_area, self.upper_area);
                    let to = |at: u16, start: u16, length: u16| ((at + 1).saturating_sub(start) as f32 * 100.0 / length.max(1) as f32).clamp(4.0, 96.0);
                    match handle {
                        Handle::Guide => self.panes.guide = (m.column + 1).saturating_sub(guide.x).max(6),
                        Handle::Split if self.side => self.panes.detail = to(m.column, right.x, right.width),
                        Handle::Split => self.panes.detail = to(m.row, right.y, right.height),
                        Handle::Picture => self.panes.image = (100.0 - to(m.column, upper.x, upper.width)).clamp(4.0, 96.0),
                        Handle::WideDetail => {
                            let both = self.panes.wide_detail + self.panes.wide_image;
                            self.panes.wide_detail = to(m.column, right.x, right.width).min(both - 4.0);
                            self.panes.wide_image = both - self.panes.wide_detail;
                        }
                        Handle::WideImage => {
                            let end = to(m.column, right.x, right.width);
                            self.panes.wide_image = (end - self.panes.wide_detail).clamp(4.0, 96.0 - self.panes.wide_detail);
                        }
                    }
                }
                None => {
                    if let Some((pane, start, _)) = self.selection {
                        self.selection = Some((pane, start, inside(pane)));
                    }
                }
            },
            MouseEventKind::Up(_) => {
                if self.drag.take().is_some() {
                    self.panes.save();
                } else if self.selection.is_some_and(|(_, start, end)| start == end) {
                    self.selection = None;
                    match (self.reclick, self.rows.get(self.sel)) {
                        (true, Some(Row::Note(_))) => self.start_note(false),
                        (true, _) => self.pending = Some(KeyCode::Enter),
                        _ => {}
                    }
                } else {
                    self.copy = self.selection.is_some();
                    if let Some((2, from, to)) = self.selection {
                        let row = |at: Position| self.scroll as usize + at.y.saturating_sub(diff.y + 1) as usize;
                        self.picked_lines = Some((row(from).min(row(to)), row(from).max(row(to))));
                    }
                }
                self.reclick = false;
            }
            _ => {}
        }
    }

    fn key(&mut self, code: KeyCode) -> bool {
        self.selection = None;
        if !matches!(code, KeyCode::Char('c' | 'n')) {
            self.picked_lines = None;
        }
        let armed = std::mem::take(&mut self.reset_armed);
        if code == KeyCode::Char('R') {
            let drafts = self.state.notes.iter().filter(|note| !note.posted).count();
            let marks = self.state.verdicts.len();
            match (marks + drafts, armed) {
                (0, _) => self.note = "Nothing to reset: no marks and no unpublished comments.".into(),
                (_, false) => {
                    self.reset_armed = true;
                    self.note = format!("Press R again to discard {marks} marks and {drafts} unpublished comments. Anything already published stays.");
                }
                (_, true) => {
                    self.state.verdicts.clear();
                    self.state.notes.retain(|note| note.posted);
                    self.rebuild_rows();
                    self.sel = self.sel.min(self.rows.len().saturating_sub(1));
                    let _ = self.bundle.save_state(&self.state);
                    self.note = format!("Review reset: {marks} marks and {drafts} unpublished comments discarded.");
                }
            }
            return true;
        }
        if let Some(command) = self.confirm.take() {
            match code {
                KeyCode::Char('y') => self.generate(command),
                KeyCode::Char('n' | 'q') | KeyCode::Esc => {}
                _ => self.confirm = Some(command),
            }
            return true;
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Tab | KeyCode::BackTab => {
                let mut stops = vec![0, 1];
                stops.extend(self.evidence_path().is_some().then_some(3));
                stops.extend(self.diff_shown().then_some(2));
                let here = stops.iter().position(|&pane| pane == self.focus).unwrap_or(0);
                let step = if code == KeyCode::Tab { 1 } else { stops.len() - 1 };
                self.focus = stops[(here + step) % stops.len()];
            }
            KeyCode::Char('j') | KeyCode::Down => self.step(1),
            KeyCode::Char('k') | KeyCode::Up => self.step(-1),
            KeyCode::Char('e') => {
                let n = self.item().map_or(0, |i| i.evidence.len());
                self.evidence = if n == 0 { 0 } else { (self.evidence + 1) % n };
            }
            KeyCode::Char('z') => self.zoom = !self.zoom,
            KeyCode::Char(' ') if self.focus == 1 => self.toggle_evidence(false),
            KeyCode::F(9) => self.toggle_evidence(true),
            KeyCode::Char('g') => self.ask_to_generate(),
            KeyCode::Char(c @ ('H' | 'L' | 'K' | 'J' | '<' | '>')) => self.panes.resize(c),
            KeyCode::Char('o') | KeyCode::Enter => {
                if let Some(path) = self.evidence_path() {
                    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                    let _ = std::process::Command::new(opener).arg(path).spawn();
                }
            }
            KeyCode::Char('c' | 'n') => self.start_note(false),
            KeyCode::Char('C' | 'N') => self.start_note(true),
            KeyCode::Char('p' | 'f') => self.finish = Some(Finish::Menu(0)),
            KeyCode::Char('m') => match self.bundle.review.sources.first().map(|s| s.url.clone()).filter(|url| !url.is_empty()) {
                Some(url) => {
                    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                    let _ = std::process::Command::new(opener).arg(url).spawn();
                }
                None => self.note = "This bundle names no merge request.".into(),
            },
            KeyCode::Char('s') => {
                match self.evidence_path().is_some() && !self.zoom {
                    true => self.panes.side_with_artifact = !self.panes.side_with_artifact,
                    false => self.panes.side = !self.panes.side,
                }
                self.panes.save();
            }
            KeyCode::PageDown | KeyCode::PageUp | KeyCode::Char('u' | 'U') => {
                let pane = self.focus.min(2);
                let half = (self.areas[pane].height.saturating_sub(2).max(2) / 2) as i32;
                let way = if matches!(code, KeyCode::PageDown | KeyCode::Char('u')) { 1 } else { -1 };
                match pane {
                    0 => (0..half.min(8)).for_each(|_| self.scroll_pane(0, way * 2)),
                    _ => self.scroll_pane(pane, way * half),
                }
            }
            KeyCode::Char('d' | 'D') => {
                self.hide_diff = !self.hide_diff;
                self.focus = if self.hide_diff && self.focus == 2 { 1 } else { self.focus };
            }
            KeyCode::Char('x') if matches!(self.rows.get(self.sel), Some(Row::Note(_))) => {
                if let Some(&Row::Note(i)) = self.rows.get(self.sel) {
                    self.state.notes.remove(i);
                    self.rebuild_rows();
                }
            }
            KeyCode::Char(c @ ('a' | 'x')) => match self.item().map(|i| (i.id.clone(), i.kind)) {
                Some((id, kind)) => {
                    let verdict = if c == 'a' { "agree" } else { "dismiss" };
                    let cleared = self.state.verdicts.get(&id).is_some_and(|v| v == verdict);
                    self.note = match (cleared, kind, c) {
                        (true, _, _) => format!("{id}: your mark is cleared."),
                        (_, Kind::Decision, 'a') => format!("{id}: accepted. It will not be raised."),
                        (_, Kind::Decision, _) => format!("{id}: you push back on this. It goes out when you publish the review (p)."),
                        (_, Kind::Good, _) => format!("{id}: noted."),
                        (_, _, 'a') => format!("{id}: you agree. It goes out when you publish the review (p)."),
                        _ => format!("{id}: dismissed. It will not be raised."),
                    };
                    let wants_reason = !cleared && matches!((kind, c), (Kind::Finding, 'a') | (Kind::Decision, 'x'));
                    if !wants_reason {
                        let before = self.state.notes.len();
                        self.state.notes.retain(|note| note.posted || note.item.as_ref() != Some(&id) || note.file.is_some() || note.reply_to.is_some());
                        if self.state.notes.len() != before {
                            self.rebuild_rows();
                        }
                    }
                    if wants_reason {
                        let title = match kind {
                            Kind::Decision => format!("Why you push back on {id} (optional)"),
                            _ => format!("Comment on {id} (optional)"),
                        };
                        let note = Note { item: Some(id.clone()), ..Note::default() };
                        self.input = Some(Input { title, text: String::new(), cursor: 0, note, edit: None });
                    }
                    match cleared {
                        true => drop(self.state.verdicts.remove(&id)),
                        false => drop(self.state.verdicts.insert(id, verdict.into())),
                    }
                }
                None => self.note = "Select a finding first: a and x say whether you agree with that finding. Approving the merge request is under p.".into(),
            },
            _ => return true,
        }
        if matches!(code, KeyCode::Char('r' | 'a' | 'x')) {
            self.state.finished = None;
            let _ = self.bundle.save_state(&self.state);
        }
        true
    }
}

fn items_in(bundle: &Bundle, section: Option<usize>) -> impl Iterator<Item = Row> + '_ {
    let items = bundle.review.items.iter().enumerate();
    items.filter(move |(_, i)| i.section == section).map(|(i, _)| Row::Item(i))
}

fn without_tags(text: &str) -> String {
    let mut inside = false;
    let kept: String = text
        .chars()
        .filter(|&c| {
            let tag = inside || c == '<';
            inside = (inside || c == '<') && c != '>';
            !tag
        })
        .collect();
    let lines: Vec<&str> = kept.lines().map(str::trim_end).collect();
    let mut out: Vec<&str> = Vec::new();
    for line in lines {
        if !(line.trim().is_empty() && out.last().is_none_or(|last| last.trim().is_empty())) {
            out.push(line);
        }
    }
    let mut text = out.join("\n").trim().to_string();
    let emoji = [
        (":thumbsup:", "👍"),
        (":+1:", "👍"),
        (":thumbsdown:", "👎"),
        (":white_check_mark:", "✅"),
        (":heavy_check_mark:", "✔"),
        (":x:", "❌"),
        (":tada:", "🎉"),
        (":eyes:", "👀"),
        (":rocket:", "🚀"),
        (":pray:", "🙏"),
        (":warning:", "⚠"),
    ];
    for (code, glyph) in emoji {
        text = text.replace(code, glyph);
    }
    text
}

fn artifact_kind(path: &str) -> (&'static str, &'static str) {
    match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => ("▣", "image"),
        "mp4" | "mov" | "webm" | "mkv" => ("▶", "recording"),
        _ => ("≡", "file"),
    }
}

fn provenance(p: Provenance) -> Span<'static> {
    match p {
        Provenance::Observed => Span::styled("observed", Style::new().fg(Color::Green)),
        Provenance::Checked => Span::styled("verified", Style::new().fg(accent())),
        Provenance::Inferred => Span::styled("reasoned", Style::new().fg(Color::Yellow)),
    }
}

fn row_line(app: &App, row: Row) -> Line<'static> {
    let sections = &app.bundle.review.sections;
    match row {
        Row::Overview => Line::styled(" Overview", Style::new().fg(accent()).add_modifier(Modifier::BOLD)),
        Row::Group(g) => {
            match sections.get(g) {
                Some(s) => Line::from(vec![Span::raw(format!(" {}. ", g + 1)), Span::raw(s.title.clone())]),
                None => {
                    let path = &app.bundle.files[app.bundle.groups[g][0].0].path;
                    let tail = |n: usize| path.chars().skip(path.chars().count().saturating_sub(n)).collect::<String>();
                    if sections.is_empty() {
                        Line::raw(format!(" {}", tail(app.panes.guide as usize - 4)))
                    } else {
                        Line::styled(format!(" ⚠ {}  no section", tail(22)), Style::new().fg(Color::Yellow))
                    }
                }
            }
        }
        Row::Spacer => Line::raw(""),
        Row::Heading => {
            let mine = app.state.notes.iter().filter(|note| !note.posted).count();
            let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
            let notes = if mine > 0 { format!(", {} of yours", count(mine, "comment")) } else { String::new() };
            let mut line = vec![Span::styled(" On the merge request", Style::new().fg(accent()).add_modifier(Modifier::BOLD)), Span::raw("  ")];
            if app.threads_rx.is_some() {
                line.extend([spinner(), Span::raw(" ")]);
            }
            let threads = if app.threads_rx.is_some() && app.threads.is_empty() { "loading threads".to_string() } else { count(app.threads.len(), "thread") };
            line.push(Span::styled(format!("{threads}{notes}"), Style::new().fg(Color::DarkGray)));
            Line::from(line)
        }
        Row::Thread(i) => {
            let thread = &app.threads[i];
            let (author, body) = thread.notes.first().map_or(("", ""), |n| (n.0.as_str(), n.1.as_str()));
            let style = if thread.resolved { Style::new().fg(Color::DarkGray) } else { Style::new() };
            let state = if thread.resolved { "✓" } else { "◆" };
            let text: String = without_tags(body).chars().filter(|c| !matches!(c, '*' | '`' | '#' | '>')).collect();
            let preview = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("(no text)").to_string();
            Line::from(vec![
                Span::styled(format!("   {state} "), Style::new().fg(accent())),
                Span::styled(format!("{author} "), style.add_modifier(Modifier::BOLD)),
                Span::styled(preview.trim().to_string(), style),
            ])
        }
        Row::Note(i) => {
            let note = &app.state.notes[i];
            let (sign, style) = match note.posted {
                true => ("✓", Style::new().fg(Color::DarkGray)),
                false => ("✎", Style::new().fg(Color::Yellow)),
            };
            Line::from(vec![Span::styled(format!("   {sign} "), style), Span::styled(note.text.lines().next().unwrap_or("").to_string(), style)])
        }
        Row::Item(i) => {
            let item = &app.bundle.review.items[i];
            let verdict = app.state.verdicts.get(&item.id).map(String::as_str);
            let mark = match verdict {
                Some("agree") => Span::styled("   ✓ ", Style::new().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Some(_) => Span::styled("   ✗ ", Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)),
                None => Span::styled("   · ", Style::new().fg(Color::DarkGray)),
            };
            let dismissed = verdict == Some("dismiss") && item.kind != Kind::Decision;
            let color = match (item.kind, item.severity.as_str()) {
                (Kind::Decision, _) => Color::Blue,
                (Kind::Good, _) => Color::Green,
                (_, "fix") => Color::Rgb(255, 150, 70),
                (_, "suggestion") => Color::Yellow,
                (_, "question") => Color::Magenta,
                _ => Color::Red,
            };
            Line::from(vec![
                mark,
                Span::styled(format!("{} ", item.id), Style::new().fg(color).add_modifier(Modifier::BOLD)),
                match item.status {
                    Some(Status::Unreproduced | Status::Resolved) => Span::styled(item.title.clone(), Style::new().fg(Color::DarkGray)),
                    _ if dismissed => Span::styled(item.title.clone(), Style::new().fg(Color::DarkGray).add_modifier(Modifier::CROSSED_OUT)),
                    _ => Span::raw(item.title.clone()),
                },
            ])
        }
    }
}

type Rows = Vec<(Line<'static>, Option<usize>)>;

pub fn ticket(word: &str) -> Option<(usize, usize)> {
    let bytes = word.as_bytes();
    let boundary = |i: usize| i >= bytes.len() || !bytes[i].is_ascii_alphanumeric();
    (0..bytes.len()).find_map(|start| {
        if !bytes[start].is_ascii_uppercase() || (start > 0 && bytes[start - 1].is_ascii_alphanumeric()) {
            return None;
        }
        let dash = (start..bytes.len()).find(|&i| !bytes[i].is_ascii_uppercase() && !bytes[i].is_ascii_digit())?;
        let end = (dash + 1..bytes.len()).find(|&i| !bytes[i].is_ascii_digit()).unwrap_or(bytes.len());
        let jargon = ["UTF", "SHA", "AES", "TLS", "ISO", "HTTP", "MD", "RSA", "GPT", "ES", "UTC"].contains(&&word[start..dash]);
        (bytes[dash] == b'-' && dash - start >= 2 && end > dash + 1 && boundary(end) && !jargon).then_some((start, end))
    })
}

pub fn rich(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", base));
        }
        let core = word.trim_start_matches(['(', '[', '"', '\'']).trim_end_matches([')', ']', ',', '.', ';', ':', '"', '\'']);
        let line_ref = core.rsplit_once(':').is_some_and(|(file, line)| file.contains('.') && line.starts_with(|c: char| c.is_ascii_digit()));
        let path = core.contains('/') && core.rsplit('/').next().is_some_and(|name| name.contains('.'));
        if core.starts_with("http://") || core.starts_with("https://") {
            spans.push(Span::styled(word.to_string(), base.fg(Color::LightBlue).add_modifier(Modifier::UNDERLINED)));
        } else if let Some((start, end)) = ticket(word) {
            spans.push(Span::styled(word[..start].to_string(), base));
            spans.push(Span::styled(word[start..end].to_string(), base.fg(Color::Magenta).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled(word[end..].to_string(), base));
        } else if core.len() > 1 && core.starts_with('!') && core[1..].bytes().all(|b| b.is_ascii_digit()) {
            spans.push(Span::styled(word.to_string(), base.fg(accent()).add_modifier(Modifier::BOLD)));
        } else if line_ref || path {
            spans.push(Span::styled(word.to_string(), base.fg(accent())));
        } else {
            spans.push(Span::styled(word.to_string(), base));
        }
    }
    spans
}

fn body(rows: &mut Rows, text: &str, width: usize) {
    let labels = ["What breaks", "Failure path", "Suggest", "Suggestion", "Why it matters", "Impact", "Fix", "Question"];
    let heading = Style::new().fg(accent()).add_modifier(Modifier::BOLD);
    for line in text.lines() {
        let clean = line.trim_start_matches("**");
        let marked = clean.strip_prefix("# ").map(|title| (title.trim_end_matches(['*', ':', '.']), ""));
        let known = labels.iter().find_map(|label| {
            let rest = clean.strip_prefix(label)?;
            let rest = rest.strip_prefix("**").unwrap_or(rest);
            let rest = rest.strip_prefix(['.', ':'])?;
            Some((*label, rest.trim_start_matches("**").trim_start()))
        });
        match marked.or(known) {
            Some((title, rest)) => {
                rows.push((Line::styled(title.to_string(), heading), None));
                if !rest.is_empty() {
                    paragraph(rows, rest, width, "", Style::new(), None);
                }
            }
            None => paragraph(rows, line, width, "", Style::new(), None),
        }
    }
}

fn paragraph(rows: &mut Rows, text: &str, width: usize, indent: &str, style: Style, evidence: Option<usize>) {
    for raw in text.lines().chain(text.is_empty().then_some("")) {
        for part in wrap(raw, width.saturating_sub(indent.len()).max(1)) {
            let mut spans = vec![Span::raw(indent.to_string())];
            spans.extend(rich(&part, style));
            rows.push((Line::from(spans), evidence));
        }
    }
}

fn tier_label(tier: &str) -> &str {
    match tier {
        "core" => "the core change",
        "supporting" => "supporting code",
        "low" => "low signal, skim it",
        other => other,
    }
}

fn detail(app: &App, width: usize) -> Rows {
    let bundle = &app.bundle;
    let mut rows = Rows::new();
    let plain = Style::new();
    let dim = Style::new().fg(Color::DarkGray);
    let place = |file: &Option<String>, line: Option<u32>| match (file, line) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        _ => "the whole merge request".to_string(),
    };
    let section = |rows: &mut Rows, text: String| {
        paragraph(rows, "", width, "", plain, None);
        rows.push((Line::styled(format!("· {text}"), Style::new().fg(accent()).add_modifier(Modifier::BOLD)), None));
    };
    match app.rows.get(app.sel) {
        Some(Row::Overview) => {
            let review = &bundle.review;
            paragraph(&mut rows, &review.summary, width, "", plain, None);
            section(&mut rows, "Read in this order".into());
            let how = "The guide splits the diff into parts, the core of the change first. Each part is a set of hunks that belong together, with why they exist. A finding sits under the part whose code it is about, so a part with none had nothing to raise.";
            paragraph(&mut rows, how, width, "  ", dim, None);
            paragraph(&mut rows, "", width, "", plain, None);
            for (g, part) in review.sections.iter().enumerate() {
                let mut notes: Vec<&str> = vec![tier_label(&part.tier)];
                notes.extend(review.items.iter().filter(|item| item.section == Some(g)).map(|item| item.id.as_str()));
                notes.retain(|note| !note.is_empty());
                paragraph(&mut rows, &format!("{}. {}", g + 1, part.title), width, "  ", plain.add_modifier(Modifier::BOLD), None);
                if !notes.is_empty() {
                    rows.push((Line::styled(format!("     {}", notes.join(" · ")), dim), None));
                }
            }
            if bundle.has_unsectioned() {
                rows.push((Line::styled("  Then the hunks the guide put in no section.", dim), None));
            }
            return rows;
        }
        Some(Row::Heading) => {
            let text = "Below this line is the conversation on the merge request itself, not part of the guide: threads other people (and bots) have already posted, and the comments you have written here. Select one to read it. c replies to a thread, or edits one of your comments.";
            paragraph(&mut rows, text, width, "", plain, None);
            return rows;
        }
        Some(&Row::Thread(i)) => {
            let thread = &app.threads[i];
            let state = if thread.resolved { "resolved" } else { "open" };
            paragraph(&mut rows, &format!("Thread on {}", place(&thread.file, thread.line)), width, "", plain.add_modifier(Modifier::BOLD), None);
            rows.push((Line::styled(format!("{state} · on the merge request · n replies · enter opens it on the web{}", if thread.file.is_some() { " · o opens its file" } else { "" }), dim), None));
            for (author, body, when) in &thread.notes {
                paragraph(&mut rows, "", width, "", plain, None);
                let who = Span::styled(author.clone(), Style::new().fg(accent()).add_modifier(Modifier::BOLD));
                rows.push((Line::from(vec![who, Span::styled(format!("  {}", ago(when)), dim)]), None));
                paragraph(&mut rows, &without_tags(body), width, "  ", plain, None);
            }
            for reply in app.state.notes.iter().filter(|note| note.reply_to.as_ref() == Some(&thread.id)) {
                paragraph(&mut rows, "", width, "", plain, None);
                let state = if reply.posted { "you, posted" } else { "you, not posted yet" };
                rows.push((Line::styled(state, Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)), None));
                paragraph(&mut rows, &reply.text, width, "  ", plain, None);
            }
            return rows;
        }
        Some(&Row::Note(i)) => {
            let note = &app.state.notes[i];
            let about = match (&note.reply_to, &note.item) {
                (Some(_), _) => "a reply in a thread".to_string(),
                (_, Some(id)) if note.file.is_none() => format!("on {id}"),
                _ => {
                    let at = place(&note.file, note.line);
                    match (note.start, at.rsplit_once(':')) {
                        (Some(first), Some((file, line))) => format!("on {file}:{first}-{line}"),
                        _ => format!("on {at}"),
                    }
                }
            };
            paragraph(&mut rows, &format!("Your comment {about}"), width, "", plain.add_modifier(Modifier::BOLD), None);
            let state = if note.posted { "posted to the merge request" } else { "not published yet · c edits · x deletes · p publishes the review" };
            rows.push((Line::styled(state, dim), None));
            paragraph(&mut rows, "", width, "", plain, None);
            paragraph(&mut rows, &note.text, width, "", plain, None);
            return rows;
        }
        _ => {}
    }
    let Some(item) = app.item() else {
        let g = app.group().unwrap_or(0);
        if let Some(section) = bundle.review.sections.get(g) {
            let mut files: Vec<usize> = bundle.groups[g].iter().map(|&(file, _)| file).collect();
            files.sort();
            files.dedup();
            let raised = bundle.review.items.iter().any(|item| item.section == Some(g));
            let color = match section.tier.as_str() {
                "core" => accent(),
                "supporting" => Color::Blue,
                _ => Color::DarkGray,
            };
            let bold = plain.add_modifier(Modifier::BOLD);
            let mut heading = vec![Span::styled(format!("Part {} of {}", g + 1, bundle.review.sections.len()), bold)];
            if !section.tier.is_empty() {
                heading.extend([Span::styled(" · ", dim), Span::styled(tier_label(&section.tier).to_string(), bold.fg(color))]);
            }
            let count = format!("{} file{}", files.len(), if files.len() == 1 { "" } else { "s" });
            let found = if raised { "" } else { " · nothing raised here" };
            heading.push(Span::styled(format!(" · {count}{found}"), dim));
            rows.push((Line::from(heading), None));
            paragraph(&mut rows, "", width, "", plain, None);
        }
        let text = match bundle.review.sections.get(g) {
            Some(section) => section.why.as_str(),
            None if bundle.review.sections.is_empty() => {
                "No guide for this merge request yet: press g to request one. This is its plain diff, one file per row."
            }
            None => "These hunks are in the diff, but the guide put them in no section. Nothing vouches for them: read them.",
        };
        paragraph(&mut rows, text, width, "", plain, None);
        return rows;
    };
    let (label, color) = match (item.kind, item.severity.as_str()) {
        (Kind::Finding, "blocker") => ("Blocker", Color::Red),
        (Kind::Finding, "fix" | "should fix") => ("Should fix", Color::Rgb(230, 140, 60)),
        (Kind::Finding, "suggestion") => ("Suggestion", Color::Yellow),
        (Kind::Finding, "question") => ("Question", Color::Magenta),
        (Kind::Finding, _) => ("Finding", Color::Red),
        (Kind::Decision, _) => ("Decision", Color::Blue),
        (Kind::Good, _) => ("Good call", Color::Green),
    };
    let status = match item.status {
        Some(Status::Confirmed) => Span::styled(" · confirmed by a run", Style::new().fg(Color::Green)),
        Some(Status::Unreproduced) => Span::styled(" · checked, did not reproduce", Style::new().fg(Color::Yellow)),
        Some(Status::Resolved) => Span::styled(" · fixed at this head", Style::new().fg(Color::Green)),
        None => Span::raw(""),
    };
    let bold = plain.add_modifier(Modifier::BOLD);
    rows.push((Line::from(vec![Span::styled(format!("{} · ", item.id), bold), Span::styled(label, Style::new().fg(color).add_modifier(Modifier::BOLD)), status]), None));
    paragraph(&mut rows, &item.title, width, "", bold, None);
    if item.kind != Kind::Good {
        let verdict = app.state.verdicts.get(&item.id).map(String::as_str);
        let decision = item.kind == Kind::Decision;
        let (yes, no) = if decision { (" a · Accept ", " x · Push back ") } else { (" a · Agree ", " x · Dismiss ") };
        let idle = Style::new().fg(Color::Gray).bg(Color::Rgb(48, 54, 68));
        let on = |color: Color| Style::new().fg(Color::Black).bg(color).add_modifier(Modifier::BOLD);
        let (yes_style, no_style, after) = match (verdict, decision) {
            (Some("agree"), true) => (on(Color::Green), idle, "  not raised"),
            (Some("agree"), false) => (on(Color::Green), idle, "  goes out when you publish"),
            (Some(_), true) => (idle, on(Color::Red), "  goes out when you publish"),
            (Some(_), false) => (idle, on(Color::Red), "  not raised"),
            (None, _) => (idle, idle, ""),
        };
        let line = vec![Span::styled(yes, yes_style), Span::raw(" "), Span::styled(no, no_style), Span::styled(after, dim)];
        rows.push((Line::from(line), None));
    }
    paragraph(&mut rows, "", width, "", plain, None);
    body(&mut rows, &item.body, width);
    let notes: Vec<_> = app.state.notes.iter().filter(|note| note.item.as_ref() == Some(&item.id)).collect();
    if !notes.is_empty() {
        section(&mut rows, "Your comments on it".into());
        for note in notes {
            paragraph(&mut rows, &format!("✎ {}", note.text), width, "", Style::new().fg(Color::Yellow), None);
        }
    }
    if item.evidence.is_empty() {
        section(&mut rows, "Evidence (none)".into());
        paragraph(&mut rows, "None recorded · v verify · i artifact", width, "", dim, None);
        return rows;
    }
    section(&mut rows, format!("Evidence ({})", item.evidence.len()));
    for (n, evidence) in item.evidence.iter().enumerate() {
        let selected = n == app.evidence;
        let cursor = if selected { "> " } else { "  " };
        let checked = app.checked.contains(&(item.id.clone(), n));
        let picked = Span::styled(if checked { "  ✓" } else { "" }, Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        let picture = match &evidence.path {
            Some(path) => Span::styled(format!(" {}", artifact_kind(path).0), Style::new().fg(accent()).add_modifier(Modifier::BOLD)),
            None => Span::raw(""),
        };
        let number = Span::styled(format!("{:<2}", n + 1), dim);
        let open = app.opened.contains(&(item.id.clone(), n));
        rows.push((Line::from(vec![Span::raw(cursor), number, provenance(evidence.provenance), picture, picked]), Some(n)));
        paragraph(&mut rows, &evidence.caption, width, "    ", if selected { bold } else { plain }, Some(n));
        if evidence.path.is_some() || evidence.repro.is_some() || evidence.text.is_some() {
            let fold = if open { "    ▾ details" } else { "    ▸ details · ctrl-o" };
            rows.push((Line::styled(fold, dim), Some(n)));
        }
        if !open {
            continue;
        }
        if let Some(path) = &evidence.path {
            let (_, kind) = artifact_kind(path);
            let name = path.rsplit('/').next().unwrap_or(path);
            paragraph(&mut rows, &format!("Artifact: {name} ({kind}) · o opens it"), width, "    ", Style::new().fg(accent()), Some(n));
        }
        if let Some(repro) = &evidence.repro {
            let label = match (matches!(evidence.provenance, Provenance::Inferred), evidence.path.is_some()) {
                (true, true) => "    To redraw the artifact:",
                (true, false) => "    Command used:",
                (false, _) => "    To reproduce:",
            };
            rows.push((Line::styled(label, dim), Some(n)));
            paragraph(&mut rows, &format!("$ {repro}"), width, "      ", dim, Some(n));
        }
        if let Some(text) = &evidence.text {
            let label = if matches!(evidence.provenance, Provenance::Inferred) { "    The reasoning:" } else { "    What it showed:" };
            rows.push((Line::styled(label, dim), Some(n)));
            paragraph(&mut rows, text, width, "      ", plain, Some(n));
        }
        paragraph(&mut rows, "", width, "", plain, Some(n));
    }
    rows
}

fn pane(title: &str, focused: bool) -> Block<'static> {
    let (border, kind, label) = match focused {
        true => (Style::new().fg(accent()), BorderType::Thick, Style::new().fg(Color::Black).bg(accent()).add_modifier(Modifier::BOLD)),
        false => (Style::new().fg(Color::DarkGray), BorderType::Plain, Style::new().fg(Color::DarkGray)),
    };
    Block::bordered().border_type(kind).border_style(border).title(Span::styled(format!(" {title} "), label))
}

fn draw_image(app: &mut App, frame: &mut Frame, area: Rect, path: PathBuf) {
    let shown = app.item().and_then(|item| item.evidence.iter().position(|e| e.path.as_ref().is_some_and(|p| path.ends_with(p))));
    let pictures = app.item().map_or(0, |item| item.evidence.iter().filter(|e| e.path.is_some()).count());
    let keys = if app.focus == 3 && pictures > 0 { "j/k evidence · o opens · z zooms" } else { "o opens · z zooms" };
    let (_, kind) = artifact_kind(&path.to_string_lossy());
    let title = format!("{kind} for evidence {} · {keys}", shown.map_or(0, |n| n + 1));
    app.picture_area = area;
    let picker = &app.picker;
    let layered = app.layered();
    let image = app.images.entry(path.clone()).or_insert_with(|| {
        let decoded = image::ImageReader::open(&path).ok()?.with_guessed_format().ok()?.decode().ok()?;
        let decoded = match decoded.width() > 2200 {
            true => decoded.resize(2200, u32::MAX, image::imageops::FilterType::Triangle),
            false => decoded,
        };
        Some(picker.new_resize_protocol(decoded))
    });
    let block = pane(&title, app.focus == 3);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match image {
        Some(_) if app.drag.is_some() => {
            frame.render_widget(Line::styled(" release to redraw", Style::new().fg(Color::DarkGray)), inner);
        }
        Some(_) if layered => {}
        Some(protocol) => {
            let resize = || Resize::Scale(Some(image::imageops::FilterType::CatmullRom));
            let size = protocol.size_for(resize(), inner.as_size());
            let (width, height) = (size.width.min(inner.width), size.height.min(inner.height));
            let centred = Rect::new(inner.x + (inner.width - width) / 2, inner.y + (inner.height - height) / 2, width, height);
            frame.render_stateful_widget(StatefulImage::default().resize(resize()), centred, protocol);
        }
        None => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let small = std::fs::metadata(&path).is_ok_and(|m| m.len() < 400_000);
            let text = small.then(|| std::fs::read_to_string(&path).ok()).flatten();
            let lines: Vec<Line> = match (kind, text) {
                (_, Some(text)) => text.lines().take(inner.height as usize).map(|line| Line::raw(line.replace('\t', "    "))).collect(),
                ("recording", _) => vec![Line::raw(format!("A recording: {name}")), Line::raw(""), Line::raw("Press o to play it.")],
                _ => vec![Line::raw(format!("{name} cannot be shown here.")), Line::raw(""), Line::raw("Press o to open it.")],
            };
            frame.render_widget(Paragraph::new(lines), inner);
        }
    }
}

fn fit(keys: &[&str], width: u16) -> String {
    let mut shown = String::new();
    for key in keys {
        let longer = if shown.is_empty() { key.to_string() } else { format!("{shown} · {key}") };
        if longer.chars().count() + 4 > width as usize {
            break;
        }
        shown = longer;
    }
    if shown.is_empty() { shown } else { format!(" {shown} ") }
}

/// A thumb on the pane's right border, only when there is more than fits.
pub fn scrollbar(frame: &mut Frame, pane: Rect, total: usize, at: usize, focused: bool) {
    use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};
    let track = pane.inner(ratatui::layout::Margin { horizontal: 0, vertical: 1 });
    let shown = track.height as usize;
    if shown == 0 || total <= shown {
        return;
    }
    let mut state = ScrollbarState::new(total - shown).position(at.min(total - shown)).viewport_content_length(shown);
    let color = if focused { accent() } else { Color::DarkGray };
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None).track_symbol(None).thumb_symbol("█");
    frame.render_stateful_widget(bar.thumb_style(Style::new().fg(color)), track, &mut state);
}

type Wanted<'a> = [(&'a str, &'a str, KeyCode, Color)];

/// Two rows: the keys that work everywhere, then the buttons and what you can do here.
pub fn footer(frame: &mut Frame, area: Rect, wanted: &Wanted, actions: &str, around: &str) -> Vec<(Rect, KeyCode)> {
    let bottom = Rect { y: area.bottom().saturating_sub(1), height: 1.min(area.height), ..area };
    let mut areas = buttons(frame, bottom, wanted, true);
    let after = areas.last().map_or(bottom.x, |(button, _)| button.right());
    let rest = Rect { x: after, width: bottom.right().saturating_sub(after), ..bottom };
    let placed = [(around, Rect { height: 1.min(area.height), ..area }), (actions, rest)];
    for (text, row) in placed {
        frame.render_widget(hints(text), row);
        areas.extend(hint_areas(text, row));
    }
    areas
}

/// Filled buttons at one end of a row, for the few actions worth a click target.
pub fn buttons(frame: &mut Frame, row: Rect, wanted: &[(&str, &str, KeyCode, Color)], left: bool) -> Vec<(Rect, KeyCode)> {
    let labels: Vec<String> = wanted.iter().map(|(key, label, _, _)| format!(" {key} · {label} ")).collect();
    let total: u16 = labels.iter().map(|label| label.chars().count() as u16 + 1).sum();
    let mut x = if left { row.x + 1 } else { row.right().saturating_sub(total).max(row.x) };
    let mut areas = Vec::new();
    for (label, &(_, _, code, color)) in labels.iter().zip(wanted) {
        let button = Rect::new(x, row.y, label.chars().count() as u16, 1).intersection(row);
        frame.render_widget(Span::styled(label.clone(), Style::new().fg(Color::Black).bg(color).add_modifier(Modifier::BOLD)), button);
        areas.push((button, code));
        x = button.right() + 1;
    }
    areas
}

pub fn hints(text: &str) -> Line<'static> {
    let key = Style::new().fg(accent()).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];
    for (i, part) in text.split(" · ").enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::new().fg(Color::DarkGray)));
        }
        let (keys, what) = part.split_once(' ').unwrap_or((part, ""));
        spans.push(Span::styled(keys.to_string(), key));
        spans.push(Span::styled(format!(" {what}"), Style::new().fg(Color::Gray)));
    }
    Line::from(spans)
}

/// Where each hint in a row drawn by `hints` sits, with the key it stands for,
/// so a click on a hint does what its key does. Pairs such as j/k are left out.
pub fn hint_areas(text: &str, row: Rect) -> Vec<(Rect, KeyCode)> {
    let mut areas = Vec::new();
    let mut x = row.x + 1;
    for part in text.split(" · ") {
        let width = part.chars().count() as u16;
        let keys = part.split(' ').next().unwrap_or("");
        let code = match keys {
            "/" => Some(KeyCode::Char('/')),
            "j/k" | "a/x" => None,
            _ => match keys.split('/').next().unwrap_or("") {
                "enter" => Some(KeyCode::Enter),
                "tab" => Some(KeyCode::Tab),
                "esc" => Some(KeyCode::Esc),
                "ctrl-q" => Some(KeyCode::F(12)),
                key if key.chars().count() == 1 => key.chars().next().map(KeyCode::Char),
                _ => None,
            },
        };
        let area = Rect::new(x, row.y, width, 1).intersection(row);
        areas.extend(code.filter(|_| area.width == width).map(|code| (area, code)));
        x += width + 3;
    }
    areas
}

fn header(app: &App, width: u16) -> Vec<Line<'static>> {
    let review = &app.bundle.review;
    let dim = Style::new().fg(Color::DarkGray);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let join = |parts: &[&str]| parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" · ");
    let count = |label: &str, done: usize, total: usize| {
        let color = match (total, done >= total) {
            (0, _) => Color::DarkGray,
            (_, true) => Color::Green,
            (_, false) => Color::Yellow,
        };
        Span::styled(format!("{label} {done}/{total}"), Style::new().fg(color))
    };
    let mut open = String::new();
    if !app.tabs.terms.is_empty() {
        let numbered: Vec<_> = app.tabs.terms.iter().enumerate().map(|(i, tab)| format!("{} {}", i + 1, tab.name)).collect();
        open = format!("open: {}", numbered.join(", "));
    }
    let (title, rest) = match review.sources.as_slice() {
        [s] => {
            let name = format!("{} {}", s.repo.rsplit('/').next().unwrap_or(""), s.reference);
            let branches = if s.branch.is_empty() { String::new() } else { format!("{} → {}", s.branch, s.target) };
            let head = format!("head {:.8}", s.head_sha);
            let mut title = vec![Span::raw(" ")];
            if !name.trim().is_empty() {
                title.push(Span::styled(name.trim().to_string(), bold.fg(accent())));
                title.push(Span::styled(" · ", dim));
            }
            title.push(Span::styled(review.title.clone(), bold));
            let dates = app.dates.as_ref().map_or(String::new(), |(opened, updated)| format!("opened {} · updated {}", ago(opened), ago(updated)));
            (title, join(&[&open, &dates, &s.url, &head, &s.author, &branches]))
        }
        many => {
            let refs: Vec<_> = many.iter().map(|s| s.reference.as_str()).collect();
            let title = vec![Span::raw(" "), Span::styled(review.title.clone(), bold)];
            (title, join(&[&open, &format!("{} merge requests", many.len()), &refs.join(" ")]))
        }
    };
    let second = match (&app.busy, app.note.is_empty()) {
        (Some(busy), _) => Line::from(vec![Span::raw(" "), spinner(), Span::raw(format!(" {busy}…"))]),
        (None, true) => Line::from(vec![
            Span::raw(" "),
            count("verdicts", app.state.verdicts.len(), review.items.len()),
            Span::styled(format!(" · {rest}"), dim),
        ]),
        (None, false) => Line::styled(format!(" {}", app.note), Style::new().fg(Color::Yellow)),
    };
    let mut title = title;
    let lines = |sign: char| app.bundle.files.iter().flat_map(|file| &file.hunks).flat_map(|hunk| &hunk.lines).filter(|line| line.starts_with(sign)).count();
    let files = app.bundle.files.len();
    title.extend([
        Span::styled(format!("  +{}", lines('+')), Style::new().fg(Color::Green)),
        Span::styled(format!(" -{}", lines('-')), Style::new().fg(Color::Red)),
        Span::styled(format!(" · {files} file{}", if files == 1 { "" } else { "s" }), dim),
    ]);
    title.splice(1..1, [Span::styled(" ‹ ", Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD)), Span::raw(" ")]);
    let pending = app.state.notes.iter().filter(|note| !note.posted).count();
    match (&app.state.finished, pending) {
        (Some(done), _) => title.push(Span::styled(format!("  ✓ review {}", done.how), Style::new().fg(Color::Green))),
        (None, 0) => {}
        (None, n) => title.push(Span::styled(format!("  ✎ {n} unpublished"), Style::new().fg(Color::Yellow))),
    }
    if app.bundle.successor.is_some() {
        let head = review.sources.first().map_or("", |s| s.head_sha.as_str());
        let warning = format!("  ⚠ guide is for an older head ({head:.8}) · g update");
        title.push(Span::styled(warning, Style::new().fg(Color::Yellow)));
    }
    let over = title.iter().map(|span| span.content.chars().count()).sum::<usize>().saturating_sub(width as usize);
    if let Some(name) = title.iter_mut().find(|span| span.content == review.title).filter(|_| over > 0) {
        let keep = name.content.chars().count().saturating_sub(over + 1).max(12);
        name.content = format!("{}…", name.content.chars().take(keep).collect::<String>()).into();
    }
    vec![Line::from(title), second]
}

fn draw(app: &mut App, frame: &mut Frame) {
    clear_popup();
    draw_review(app, frame);
    if let Some(text) = popup_selection(frame, app.modal_selection) {
        if app.copy {
            app.copy = false;
            app.note = copied(text.chars().count());
            app.clipboard = Some(text);
        }
    }
}

fn draw_review(app: &mut App, frame: &mut Frame) {
    if !app.diff_shown() && app.focus == 2 {
        app.focus = 1;
    }
    if app.focus == 3 && app.evidence_path().is_none() {
        app.focus = 1;
    }
    let tabs = if app.tabs.terms.is_empty() { "" } else { " · ctrl-q tabs" };
    let around = format!("q/esc back · / commands · ? help · tab pane{tabs} · m open on the web · , settings");
    let artifact = matches!(app.focus, 1 | 3) && app.evidence_path().is_some();
    let target = app.open_target().map_or("file".to_string(), |(file, line)| {
        format!("{}:{line}", app.bundle.files[file].path.rsplit('/').next().unwrap_or(""))
    });
    let many = if app.section_files().len() > 1 { " · O open all its files" } else { "" };
    let on_web = matches!(app.rows.get(app.sel), Some(Row::Thread(_) | Row::Heading));
    let open = match (app.artifact_at_cursor(), app.open_target().is_some()) {
        (true, _) => " · o open artifact".to_string(),
        (_, true) => format!(" · o open {target}{many}"),
        _ => String::new(),
    };
    let enter = match (app.focus, artifact, on_web) {
        (0, _, true) => format!("enter open on the web{open}"),
        (0, _, _) => format!("enter go in{open}"),
        (3, true, _) => "enter/o open artifact".to_string(),
        (1, _, _) if app.item().is_some_and(|item| !item.evidence.is_empty()) => format!("space/enter pick for v/i{open}"),
        _ => format!("enter/o open {target}{many}"),
    };
    let (generate, regenerate): (Vec<(&str, &str, KeyCode, Color)>, &str) = match (app.bundle.successor.is_some(), app.bundle.review.sections.is_empty()) {
        (true, _) => (vec![("g", "Update guide", KeyCode::Char('g'), accent())], ""),
        (_, true) => (vec![("g", "Generate guide", KeyCode::Char('g'), accent())], ""),
        _ => (Vec::new(), " · g regenerate"),
    };
    let actions = format!("j/k move · {enter} · c comment · t ask · v verify · i artifact · a/x agree/dismiss{regenerate}");
    let [head, main, foot] = Layout::vertical([Constraint::Length(2), Constraint::Min(0), Constraint::Length(2)]).areas(frame.area());
    let publish = [("p", "Publish review", KeyCode::Char('p'), Color::Green)];
    let reserved = publish[0].1.chars().count() as u16 + 8;
    frame.render_widget(ratatui::text::Text::from(header(app, head.width.saturating_sub(reserved))), head);
    app.action_buttons = buttons(frame, Rect { height: 1.min(head.height), ..head }, &publish, false);
    app.back_area = Rect::new(head.x, head.y, 5.min(head.width), 1.min(head.height));
    if let [source] = app.bundle.review.sources.as_slice() {
        let name = format!("{} {}", source.repo.rsplit('/').next().unwrap_or(""), source.reference);
        let link = Rect::new(head.x + 5, head.y, name.trim().chars().count() as u16, 1).intersection(head);
        app.action_buttons.extend((!source.url.is_empty() && link.width > 0).then_some((link, KeyCode::Char('m'))));
    }
    app.screen = frame.area();
    let guide = app.panes.guide.min(main.width.saturating_sub(6)).max(6);
    let [left, right] = Layout::horizontal([Constraint::Length(guide), Constraint::Min(0)]).areas(main);
    let diff_hidden = !app.diff_shown();
    if diff_hidden && app.focus == 2 {
        app.focus = 1;
    }
    let image = app.evidence_path();
    let pictured = image.is_some() && !app.zoom;
    app.side = if pictured { app.panes.side_with_artifact } else { app.panes.side };
    let wide = pictured && app.side && !diff_hidden;
    let [upper, bottom] = match (diff_hidden, wide) {
        (true, _) | (_, true) => [right, Rect::default()],
        _ => {
            let direction = if app.side { ratatui::layout::Direction::Horizontal } else { ratatui::layout::Direction::Vertical };
            Layout::new(direction, [share(app.panes.detail), Constraint::Min(0)]).areas(right)
        }
    };
    let [top, picture, bottom] = match (wide, pictured) {
        (true, _) => Layout::horizontal([share(app.panes.wide_detail), share(app.panes.wide_image), Constraint::Min(0)]).areas(right),
        (_, true) => {
            let [text, picture] = Layout::horizontal([Constraint::Min(0), share(app.panes.image)]).areas(upper);
            [text, picture, bottom]
        }
        _ => [upper, Rect::default(), bottom],
    };
    let seam = |x: u16, beside: Rect| Rect::new(x.saturating_sub(1), beside.y, 2, beside.height);
    app.handles.clear();
    if !app.zoom {
        app.handles.push((seam(right.x, main), Handle::Guide));
        match wide {
            true => app.handles.extend([(seam(picture.x, right), Handle::WideDetail), (seam(bottom.x, right), Handle::WideImage)]),
            false => {
                app.handles.extend(pictured.then(|| (seam(picture.x, upper), Handle::Picture)));
                let below = Rect::new(right.x, bottom.y.saturating_sub(1), right.width, 2);
                app.handles.extend((!diff_hidden).then(|| if app.side { (seam(bottom.x, right), Handle::Split) } else { (below, Handle::Split) }));
            }
        }
    }
    (app.right_area, app.upper_area) = (right, upper);

    let width = left.width.saturating_sub(2) as usize;
    let rows: Vec<ratatui::text::Text> = app
        .rows
        .iter()
        .map(|&r| {
            let mut line = row_line(app, r);
            let Some(last) = line.spans.pop() else { return line.into() };
            let lead = line.width();
            if lead + last.content.chars().count() <= width {
                line.spans.push(last);
                return line.into();
            }
            let hang = if lead > width / 2 { 4 } else { lead };
            let mut lines = Vec::new();
            if hang != lead {
                lines.push(Line::from(std::mem::take(&mut line.spans)));
            }
            for part in wrap(&last.content, width.saturating_sub(hang).max(1)) {
                let mut spans = if lines.is_empty() { std::mem::take(&mut line.spans) } else { vec![Span::raw(" ".repeat(hang))] };
                spans.push(Span::styled(part, last.style));
                lines.push(Line::from(spans));
            }
            lines.into()
        })
        .collect();
    app.row_heights = rows.iter().map(|row| row.height() as u16).collect();
    let list = List::new(rows)
        .block(pane("guide", app.focus == 0))
        .highlight_style(match app.focus == 0 {
            true => Style::new().add_modifier(Modifier::REVERSED),
            false => Style::new().bg(Color::Rgb(48, 54, 68)).add_modifier(Modifier::BOLD),
        });
    app.list.select(Some(app.sel));
    frame.render_stateful_widget(list, left, &mut app.list);
    let above: u16 = app.row_heights.iter().take(app.list.offset()).sum();
    scrollbar(frame, left, app.row_heights.iter().sum::<u16>() as usize, above as usize, app.focus == 0);
    app.areas = if app.zoom { [left, right, Rect::default()] } else { [left, top, bottom] };

    app.picture_area = Rect::default();
    if app.focus == 3 && image.is_none() {
        app.focus = 1;
    }
    if let (true, Some(path)) = (app.zoom, image.clone()) {
        draw_image(app, frame, right, path);
        return;
    }
    if let Some(path) = image {
        draw_image(app, frame, picture, path);
    }
    let text_area = top;
    let (text, rows): (Vec<_>, Vec<_>) = detail(app, text_area.width.saturating_sub(2).max(10) as usize).into_iter().unzip();
    app.detail_scroll = app.detail_scroll.min(text.len().saturating_sub(text_area.height.saturating_sub(2) as usize) as u16);
    let picked = app.checked.len();
    let count = format!("{picked} picked");
    let mut keys: Vec<&str> = match (picked, app.item().map_or(0, |item| item.evidence.len()), app.focus == 1) {
        (0, _, false) => vec![],
        (0, 0, _) if app.item().is_some() => vec!["v verify", "i artifact"],
        (0, 0, _) => vec![],
        (0, _, _) => vec!["ctrl-o details", "j/k evidence", "v verify", "i artifact", "space pick for v/i"],
        _ => vec![&count, "v verify", "i artifact"],
    };
    keys.extend(app.hide_diff.then_some("d show diff"));
    let style = match picked {
        0 => Style::new().fg(Color::DarkGray),
        _ => Style::new().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
    };
    let hint = Line::styled(fit(&keys, text_area.width), style);
    let block = pane("detail", app.focus == 1).title_bottom(hint);
    let starts = |line: &Line, with: &str| line.spans.first().is_some_and(|span| span.content.starts_with(with));
    app.verdict_row = text.iter().position(|line| starts(line, " a · ")).map(|row| {
        let width = |n: usize| text[row].spans.get(n).map_or(0, |span| span.content.chars().count() as u16);
        (row, width(0), width(2))
    });
    let on_overview = matches!(app.rows.get(app.sel), Some(Row::Overview));
    let mut part = None;
    app.detail_parts = text
        .iter()
        .map(|line| {
            let shown = line.to_string();
            let numbered = shown.strip_prefix("  ").and_then(|rest| rest.split_once(". ")).and_then(|(n, _)| n.parse::<usize>().ok());
            part = match numbered {
                Some(n) => n.checked_sub(1),
                None if shown.starts_with("     ") => part,
                None => None,
            };
            part.filter(|_| on_overview)
        })
        .collect();
    let (mut note, mut count) = (None, 0);
    app.detail_notes = text
        .iter()
        .map(|line| {
            if line.to_string().starts_with("✎ ") {
                (note, count) = (Some(count), count + 1);
            } else if line.width() == 0 || starts(line, "· ") {
                note = None;
            }
            note
        })
        .collect();
    app.detail_folds = text.iter().map(|line| line.spans.first().is_some_and(|span| span.content.trim_start().starts_with(['▸', '▾']))).collect();
    let length = text.len();
    frame.render_widget(Paragraph::new(text).scroll((app.detail_scroll, 0)).block(block), text_area);
    scrollbar(frame, text_area, length, app.detail_scroll as usize, app.focus == 1);
    (app.detail_rows, app.detail_area) = (rows, text_area);

    let room = bottom.width.saturating_sub(2);
    if !diff_hidden && app.diff_width != room && app.drag.is_none() {
        let was = app.diffed.2.get(app.cursor).map(|mark| (mark.file, mark.new, mark.old, mark.sign));
        app.diff_width = room;
        app.diffed = app.build_diff();
        let same = |mark: &Mark| Some((mark.file, mark.new, mark.old, mark.sign)) == was;
        app.cursor = app.diffed.2.iter().position(same).unwrap_or(app.diffed.1).min(app.diffed.0.len().saturating_sub(1));
        let height = bottom.height.saturating_sub(2).max(1) as usize;
        if app.cursor < app.scroll as usize || app.cursor >= app.scroll as usize + height {
            app.scroll = app.cursor.saturating_sub(height / 3) as u16;
        }
    }
    app.scroll = app.scroll.min(app.diffed.0.len().saturating_sub(bottom.height.saturating_sub(2) as usize) as u16);
    let mut lines: Vec<Line> = app.diffed.0.iter().skip(app.scroll as usize).take(bottom.height as usize).cloned().collect();
    if let (true, Some(line)) = (app.focus == 2, lines.get_mut(app.cursor.saturating_sub(app.scroll as usize))) {
        if app.cursor >= app.scroll as usize {
            for span in &mut line.spans {
                span.style = span.style.bg(Color::Rgb(44, 58, 92));
            }
        }
    }
    if !diff_hidden {
        let layout = if app.side { "s stack" } else { "s side by side" };
        let mut keys = if app.focus == 2 { vec!["c comment", "o open file", "j/k line"] } else { vec![] };
        keys.extend([layout, "d hide"]);
        keys.retain(|key| !key.is_empty());
        let files = app.section_files();
        let here = app.diffed.2.get(app.cursor.min(app.diffed.2.len().saturating_sub(1))).map(|mark| mark.file);
        let title = match here.and_then(|file| Some((files.iter().position(|&(listed, _)| listed == file)?, file))) {
            Some((n, file)) => {
                let path = &app.bundle.files[file].path;
                let room = (bottom.width as usize).saturating_sub(26).max(12);
                let skipped = path.chars().count().saturating_sub(room);
                let tail: String = path.chars().skip(skipped).collect();
                format!("diff · file {} of {} · {}{tail}", n + 1, files.len(), if skipped > 0 { "…" } else { "" })
            }
            None => "diff".to_string(),
        };
        let block = pane(&title, app.focus == 2).title_bottom(Line::styled(fit(&keys, bottom.width), Style::new().fg(Color::DarkGray)));
        frame.render_widget(Paragraph::new(lines).block(block), bottom);
        scrollbar(frame, bottom, app.diffed.0.len(), app.scroll as usize, app.focus == 2);
    }

    app.action_buttons.extend(footer(frame, foot, &generate, &actions, &around));

    if let Some((pane, a, b)) = app.selection.filter(|(_, a, b)| a != b) {
        let (left, right) = match app.areas.get(pane) {
            Some(area) => (area.x + 1, area.right().saturating_sub(2)),
            None => (0, frame.area().width.saturating_sub(1)),
        };
        let text = selected_text(frame.buffer_mut(), left, right, a, b, pane == 1);
        if app.copy {
            app.copy = false;
            app.note = copied(text.chars().count());
            app.clipboard = Some(text);
        }
    }

    let held = app.drag.or(app.hover);
    if let Some(&(seam, _)) = app.handles.iter().find(|(_, handle)| Some(*handle) == held) {
        for position in seam.positions() {
            if let Some(cell) = frame.buffer_mut().cell_mut(position) {
                cell.set_fg(accent());
            }
        }
    }
    app.tabs.draw(frame, "back to review");

    app.buttons.clear();
    let dim = Style::new().fg(Color::DarkGray);
    let heading = Style::new().fg(accent()).add_modifier(Modifier::BOLD);
    let width = frame.area().width.saturating_sub(4).min(84).saturating_sub(6).max(10) as usize;
    if let Some(command) = &app.confirm {
        let source = app.bundle.review.sources.first();
        let url = source.map_or("", |s| s.url.as_str());
        let name = source.map_or(String::new(), |s| format!("{} {}", s.repo.rsplit('/').next().unwrap_or(""), s.reference));
        let mut lines = vec![Line::styled(name.trim().to_string(), heading), Line::raw(app.bundle.review.title.clone()), Line::raw("")];
        let newer = app.bundle.successor.as_ref().and_then(|dir| dir.file_name()).map(|n| n.to_string_lossy().into_owned());
        let newer = newer.as_deref().and_then(|name| name.rsplit('-').next()).unwrap_or("").to_string();
        let outdated = app.bundle.successor.is_some();
        if outdated {
            let written = source.map_or("", |s| s.head_sha.as_str());
            let warning = format!("⚠ This guide was written for head {written:.8}. The merge request is now at {newer}.");
            lines.extend(wrap(&warning, width).into_iter().map(|part| Line::styled(part, Style::new().fg(Color::Yellow))));
            lines.push(Line::raw(""));
            let explain = "Updating starts an agent run. It starts from this guide, reviews what changed since, and marks findings that are now fixed.";
            lines.extend(wrap(explain, width).into_iter().map(|part| Line::styled(part, dim)));
            lines.push(Line::raw(""));
        } else {
            if !app.bundle.review.sections.is_empty() {
                let warning = "⚠ This replaces the guide you are reading. The old one is kept as review.before.json. Your marks stay attached by id (F1, Q2), so check them afterwards.";
                lines.extend(wrap(warning, width).into_iter().map(|part| Line::styled(part, Style::new().fg(Color::Yellow))));
                lines.push(Line::raw(""));
            }
            lines.extend([Line::styled("Starts an agent run: a few minutes, and it spends tokens.", dim), Line::raw("")]);
        }
        for (i, part) in wrap(&command.replace("$REVUI_URL", url).replace("$REVUI_CLAUDE", &mrs::claude()), width - 2).into_iter().enumerate() {
            let prompt = Span::styled(if i == 0 { "$ " } else { "  " }, dim);
            lines.push(Line::from(vec![prompt, Span::styled(part, Style::new().fg(Color::Green))]));
        }
        let buttons = match outdated {
            true => [("y", "Update the guide", KeyCode::Char('y')), ("n", "Keep reading this one", KeyCode::Char('n'))],
            false => [("y", "Start", KeyCode::Char('y')), ("n", "Cancel", KeyCode::Char('n'))],
        };
        let title = match (outdated, app.bundle.review.sections.is_empty()) {
            (true, _) => " This guide is out of date ",
            (_, false) => " Regenerate the guide ",
            _ => " Generate a guide ",
        };
        app.buttons = popup(frame, title, lines, &buttons).1;
    }

    if app.help {
        app.buttons = draw_help(frame);
    }

    if let Some(input) = &app.input {
        let wrapped = wrap(&input.text, width);
        let (mut row, mut left) = (0, input.cursor.min(input.text.chars().count()));
        while row + 1 < wrapped.len() && left > wrapped[row].chars().count() {
            left -= wrapped[row].chars().count() + 1;
            row += 1;
        }
        let caret = (row as u16, left.min(wrapped[row].chars().count()) as u16);
        let mut lines: Vec<Line> = wrapped.into_iter().map(Line::raw).collect();
        lines.insert(0, Line::raw(""));
        lines.insert(0, Line::styled(input.title.clone(), heading));
        lines.push(Line::raw(""));
        if let Some((_, names)) = app.mention(input) {
            let mut offer = vec![Span::styled("tab ", Style::new().fg(accent()).add_modifier(Modifier::BOLD))];
            offer.extend(names.iter().take(6).enumerate().map(|(i, name)| Span::styled(format!("@{name}  "), if i == 0 { Style::new().add_modifier(Modifier::BOLD) } else { dim })));
            lines.extend([Line::from(offer), Line::raw("")]);
        }
        lines.push(Line::styled("Not sent until you publish the review.", dim));
        let buttons = [("enter", "Save", KeyCode::Enter), ("esc", "Cancel", KeyCode::Esc)];
        let (inner, areas) = popup(frame, " Comment ", lines, &buttons);
        app.buttons = areas;
        frame.set_cursor_position(Position::new(inner.x + caret.1, inner.y + 2 + caret.0));
    }

    match &app.finish {
        Some(Finish::Menu(selected)) => {
            let points = app.points();
            let replies = points.iter().filter(|p| p.reply_to.is_some()).count();
            let counts = format!("{} points to raise, {replies} of them replies in existing threads", points.len());
            let mut lines = vec![Line::styled("Publish your review", heading), Line::styled(counts, dim), Line::raw("")];
            for (i, label) in FINISH.iter().enumerate() {
                let style = if i == *selected { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
                lines.push(Line::styled(format!(" {}  {label} ", i + 1), style));
            }
            lines.extend([Line::raw(""), Line::styled("What goes out: findings you agreed with, decisions you pushed back on, and your comments. You see the exact text before anything is sent.", dim)]);
            let buttons = [("enter", "Go", KeyCode::Enter), ("esc", "Cancel", KeyCode::Esc)];
            let (inner, mut areas) = popup(frame, " Publish review ", lines, &buttons);
            for i in 0..FINISH.len() {
                areas.push((Rect::new(inner.x, inner.y + 3 + i as u16, inner.width, 1), KeyCode::Char((b'1' + i as u8) as char)));
            }
            app.buttons = areas;
        }
        Some(Finish::Preview(choice, body)) => {
            let room = frame.area().height.saturating_sub(14) as usize;
            let mut shown: Vec<String> = body.lines().flat_map(|line| wrap(line, width)).collect();
            let hidden = shown.len().saturating_sub(room);
            shown.truncate(room);
            let mut lines = vec![Line::styled(FINISH[*choice], heading), Line::styled("This is exactly what goes to the merge request:", dim), Line::raw("")];
            lines.extend(shown.into_iter().map(Line::raw));
            if hidden > 0 {
                lines.push(Line::styled(format!("… and {hidden} more lines"), dim));
            }
            let buttons = [("y", "Post to the merge request", KeyCode::Char('y')), ("n", "Back", KeyCode::Char('n'))];
            app.buttons = popup(frame, " Post this review? ", lines, &buttons).1;
        }
        None => {}
    }

    if let Some((mode, selected)) = app.menu {
        let what = match (mode, app.item()) {
            (Mode::Verify, _) if !app.checked.is_empty() => format!("Verify the {} picked claims by running them", app.checked.len()),
            (Mode::Verify, Some(item)) => format!("Verify {} by running it", item.id),
            (Mode::Verify, None) => "Verify this section's unproven findings by running them".into(),
            (Mode::Artifact, _) if !app.checked.is_empty() => format!("Make an artifact for the {} picked claims", app.checked.len()),
            (Mode::Artifact, Some(item)) => format!("Make an artifact that shows {}", item.id),
            (Mode::Artifact, None) => "Make artifacts for this section's findings".into(),
            (Mode::Ask, Some(item)) => format!("Ask about {}", item.id),
            (Mode::Ask, None) => "Ask about this part of the change".into(),
        };
        let mut lines = vec![Line::styled(what, heading), Line::raw("")];
        let options = app.options();
        for (i, (label, _)) in options.iter().enumerate() {
            let style = if i == selected { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
            lines.push(Line::styled(format!(" {}  {label} ", i + 1), style));
        }
        let buttons = [("enter", "Go", KeyCode::Enter), ("esc", "Cancel", KeyCode::Esc)];
        let (inner, mut areas) = popup(frame, [" Ask ", " Verify with ", " Make an artifact with "][mode as usize], lines, &buttons);
        for i in 0..options.len().min(9) {
            areas.push((Rect::new(inner.x, inner.y + 2 + i as u16, inner.width, 1), KeyCode::Char((b'1' + i as u8) as char)));
        }
        app.buttons = areas;
    }

    if let Some((filter, selected)) = &app.palette {
        app.buttons = draw_palette(frame, &COMMANDS, filter, *selected, app.palette_cursor);
    }
}

pub fn selected_text(buffer: &mut ratatui::buffer::Buffer, left: u16, right: u16, a: Position, b: Position, wrapped: bool) -> String {
    let (start, end) = if (a.y, a.x) <= (b.y, b.x) { (a, b) } else { (b, a) };
    let mut text = String::new();
    let mut row_of = |y: u16, from: u16, to: u16, mark: bool| {
        let mut row = String::new();
        for x in from..=to {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                row.push_str(cell.symbol());
                if mark {
                    cell.modifier.toggle(Modifier::REVERSED);
                }
            }
        }
        row.trim_end().to_string()
    };
    for y in start.y..=end.y {
        let from = if y == start.y { start.x } else { left };
        let to = if y == end.y { end.x } else { right };
        text += &row_of(y, from, to, true);
        if y < end.y {
            let (full, next) = (row_of(y, left, right, false), row_of(y + 1, left, right, false));
            let word = next.split_whitespace().next().map_or(0, |w| w.chars().count());
            let (used, width) = (full.chars().count(), (right.saturating_sub(left) + 1) as usize);
            text += match (wrapped, word) {
                (true, 1..) if used == width => "",
                (true, 1..) if used > 0 && used + 1 + word > width => " ",
                _ => "\n",
            };
        }
    }
    text
}

const HELP: &str = "# What this is
revui walks you through a merge request. An agent writes a guide: the change split into sections in reading order, with findings and decisions attached to the code they are about. You read it, check what matters, and decide.
The guide never hides code. Anything it left out is listed as \"no section\".

# Reading the guide list
Numbered rows are parts of the diff: hunks that belong together, the core of the change first and low-signal churn last. Select one to see why it exists and its code.
Rows under a part are what the review raised about that code. A part with none had nothing to raise.
B blocker · F should fix · S suggestion · Q question or a decision the change made · G good call

# A review, start to finish
1. Pick a merge request from the list. g writes a guide when it has none.
2. Read the sections top to bottom.
3. On each finding, a says you agree and x dismisses it. On a decision, a accepts the choice and x pushes back.
4. Not convinced? v has an agent try to reproduce it, i asks for a diagram or screenshot, t just asks a question.
5. c adds your own comment, on a finding or on a line of the diff (tab to the diff, move to the line, c).
6. p publishes your review. You see exactly what would be posted first, and confirm.

# Getting around
tab moves between panes, enter goes into one, o opens the file or the artifact.
/ lists every command. , opens settings. q or esc goes back. ctrl-c quits.";

pub fn draw_help(frame: &mut Frame) -> Vec<(Rect, KeyCode)> {
    let width = frame.area().width.saturating_sub(4).min(84).saturating_sub(6).max(10) as usize;
    let mut lines = Vec::new();
    for raw in HELP.lines() {
        match raw.strip_prefix("# ") {
            Some(heading) => {
                if !lines.is_empty() {
                    lines.push(Line::raw(""));
                }
                lines.push(Line::styled(heading.to_string(), Style::new().fg(accent()).add_modifier(Modifier::BOLD)));
            }
            None => lines.extend(wrap(raw, width).into_iter().map(Line::raw)),
        }
    }
    popup(frame, " revui ", lines, &[("esc", "Close", KeyCode::Esc)]).1
}

static POPUP: std::sync::Mutex<Rect> = std::sync::Mutex::new(Rect::ZERO);

pub fn popup_selection(frame: &mut Frame, selection: Option<(Position, Position)>) -> Option<String> {
    let area = *POPUP.lock().unwrap();
    let (a, b) = selection.filter(|(a, b)| area.width > 0 && a != b)?;
    let inside = |p: Position| Position::new(p.x.clamp(area.x, area.right() - 1), p.y.clamp(area.y, area.bottom().saturating_sub(1)));
    Some(selected_text(frame.buffer_mut(), area.x, area.right() - 1, inside(a), inside(b), true))
}

pub fn clear_popup() {
    *POPUP.lock().unwrap() = Rect::ZERO;
}

pub fn edit(text: &mut String, cursor: &mut usize, key: event::KeyEvent) -> bool {
    let mut chars: Vec<char> = text.chars().collect();
    let at = (*cursor).min(chars.len());
    let held = |modifier: KeyModifiers| key.modifiers.contains(modifier);
    let (ctrl, alt, cmd) = (held(KeyModifiers::CONTROL), held(KeyModifiers::ALT), held(KeyModifiers::SUPER) || held(KeyModifiers::META));
    let word_left = |chars: &[char]| {
        let mut i = at;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    };
    let word_right = |chars: &[char]| {
        let mut i = at;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        i
    };
    let next = match key.code {
        KeyCode::Home => 0,
        KeyCode::End => chars.len(),
        KeyCode::Char('a') if ctrl => 0,
        KeyCode::Char('e') if ctrl => chars.len(),
        KeyCode::Left if cmd => 0,
        KeyCode::Right if cmd => chars.len(),
        KeyCode::Left if alt || ctrl => word_left(&chars),
        KeyCode::Right if alt || ctrl => word_right(&chars),
        KeyCode::Char('b') if alt => word_left(&chars),
        KeyCode::Char('f') if alt => word_right(&chars),
        KeyCode::Left => at.saturating_sub(1),
        KeyCode::Right => (at + 1).min(chars.len()),
        KeyCode::Backspace if cmd => {
            chars.drain(..at);
            0
        }
        KeyCode::Char('u') if ctrl => {
            chars.drain(..at);
            0
        }
        KeyCode::Char('k') if ctrl => {
            chars.truncate(at);
            at
        }
        KeyCode::Backspace if alt || ctrl => {
            let start = word_left(&chars);
            chars.drain(start..at);
            start
        }
        KeyCode::Char('w') if ctrl => {
            let start = word_left(&chars);
            chars.drain(start..at);
            start
        }
        KeyCode::Backspace if at > 0 => {
            chars.remove(at - 1);
            at - 1
        }
        KeyCode::Delete if at < chars.len() => {
            chars.remove(at);
            at
        }
        KeyCode::Char(c) if !ctrl && !alt && !cmd => {
            chars.insert(at, c);
            at + 1
        }
        _ => return false,
    };
    (*text, *cursor) = (chars.into_iter().collect(), next);
    true
}

pub fn ago(iso: &str) -> String {
    let number = |range: std::ops::Range<usize>| iso.get(range).and_then(|part| part.parse::<i64>().ok());
    let (Some(year), Some(month), Some(day)) = (number(0..4), number(5..7), number(8..10)) else { return iso.to_string() };
    let (hour, minute) = (number(11..13).unwrap_or(0), number(14..16).unwrap_or(0));
    let (y, m) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let days = 365 * y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400) + (153 * m + 2) / 5 + day - 719_469;
    let then = days * 86_400 + hour * 3_600 + minute * 60;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(then, |d| d.as_secs() as i64);
    let names = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let date = format!("{} {day}", names[(month as usize).clamp(1, 12) - 1]);
    match now - then {
        seconds if seconds < 90 => "just now".to_string(),
        seconds if seconds < 3_600 => format!("{}m ago", seconds / 60),
        seconds if seconds < 86_400 => format!("{}h ago", seconds / 3_600),
        seconds if seconds < 14 * 86_400 => format!("{}d ago ({date})", seconds / 86_400),
        _ => date,
    }
}

pub type Command = (&'static str, &'static str, KeyCode);

pub fn draw_palette(frame: &mut Frame, all: &[Command], filter: &str, selected: usize, cursor: usize) -> Vec<(Rect, KeyCode)> {
    let width = frame.area().width.saturating_sub(4).min(84).saturating_sub(6).max(10) as usize;
    let dim = Style::new().fg(Color::DarkGray);
    let mut lines = vec![Line::from(vec![Span::styled("/ ", dim), Span::raw(filter.to_string())]), Line::raw("")];
    let matches = matching(all, filter);
    let room = (frame.area().height.saturating_sub(12) as usize).max(3);
    let first = selected.saturating_sub(room - 1).min(matches.len().saturating_sub(room));
    for (i, (label, key, _)) in matches.iter().enumerate().skip(first).take(room) {
        let style = if i == selected { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
        let fits = width.saturating_sub(key.chars().count() + 4);
        let label: String = if label.chars().count() > fits { format!("{}…", label.chars().take(fits.saturating_sub(1)).collect::<String>()) } else { label.to_string() };
        let pad = width.saturating_sub(label.chars().count() + key.chars().count() + 2);
        lines.push(Line::styled(format!(" {label}{}{key} ", " ".repeat(pad)), style));
    }
    if matches.len() > room {
        lines.push(Line::styled(format!(" {} more: keep typing", matches.len() - room), dim));
    }
    let buttons = [("enter", "Run", KeyCode::Enter), ("esc", "Close", KeyCode::Esc)];
    let (inner, mut areas) = popup(frame, " Commands ", lines, &buttons);
    frame.set_cursor_position(Position::new(inner.x + 2 + cursor.min(filter.chars().count()) as u16, inner.y));
    for (i, (_, _, code)) in matches.iter().enumerate().skip(first).take(room) {
        areas.push((Rect::new(inner.x, inner.y + 2 + (i - first) as u16, inner.width, 1), *code));
    }
    areas
}

pub fn palette_key(all: &[Command], palette: &mut Option<(String, usize)>, code: KeyCode) -> Option<KeyCode> {
    let (mut filter, selected) = palette.take()?;
    let last = matching(all, &filter).len().saturating_sub(1);
    match code {
        KeyCode::Esc => {}
        KeyCode::Enter => return matching(all, &filter).get(selected).map(|c| c.2),
        KeyCode::Down => *palette = Some((filter, (selected + 1).min(last))),
        KeyCode::Up => *palette = Some((filter, selected.saturating_sub(1))),
        KeyCode::Backspace => {
            filter.pop();
            *palette = Some((filter, 0));
        }
        KeyCode::Char(c) => {
            filter.push(c);
            *palette = Some((filter, 0));
        }
        _ => *palette = Some((filter, selected)),
    }
    None
}

pub fn matching(all: &[Command], filter: &str) -> Vec<Command> {
    let filter = filter.to_lowercase();
    let wanted = |label: &str| filter.split_whitespace().all(|term| label.to_lowercase().contains(term));
    all.iter().copied().filter(|(label, _, _)| wanted(label)).collect()
}

const COMMANDS: [Command; 30] = [
    ("Go one step in: a pane, then the artifact or file; a thread opens on the web", "enter", KeyCode::Enter),
    ("Ask the companion agent about this", "t", KeyCode::Char('t')),
    ("Ask: choose where the agent opens", "T", KeyCode::Char('T')),
    ("Verify this with an agent", "v", KeyCode::Char('v')),
    ("Verify: choose where the agent opens", "V", KeyCode::Char('V')),
    ("Comment on this (a finding, a diff line, or reply in a thread)", "c", KeyCode::Char('c')),
    ("Comment on the whole merge request", "C", KeyCode::Char('C')),
    ("Publish your review to the merge request", "p", KeyCode::Char('p')),
    ("Reset your review: discard every mark and unpublished comment", "R R", KeyCode::Char('R')),
    ("Show or hide the diff pane", "d", KeyCode::Char('d')),
    ("Page down in the focused pane", "u", KeyCode::Char('u')),
    ("Page up in the focused pane", "U", KeyCode::Char('U')),
    ("Put the diff beside the detail pane, or under it", "s", KeyCode::Char('s')),
    ("Open the merge request in the browser", "m", KeyCode::Char('m')),
    ("Make an artifact for this finding (diagram, screenshot, recording)", "i", KeyCode::Char('i')),
    ("Artifact: choose where the agent opens", "I", KeyCode::Char('I')),
    ("Generate a guide for this merge request", "g", KeyCode::Char('g')),
    ("Agree with this finding (it goes out when you publish)", "a", KeyCode::Char('a')),
    ("Dismiss this finding (it is not raised)", "x", KeyCode::Char('x')),
    ("Evidence: next", "e", KeyCode::Char('e')),
    ("Evidence: pick for verifying", "space", KeyCode::Char(' ')),
    ("Evidence: zoom the image", "z", KeyCode::Char('z')),
    ("Evidence: show or hide how it was checked", "ctrl-o", KeyCode::F(9)),
    ("Open the file in your editor (in the artifact pane: the artifact)", "o", KeyCode::Char('o')),
    ("Open every file in this part of the diff", "O", KeyCode::Char('O')),
    ("Show the agent and editor tabs (keep ctrl held, q again: next tab)", "ctrl-q", KeyCode::F(12)),
    ("Switch pane", "tab", KeyCode::Tab),
    ("Resize panes", "H L J K < >", KeyCode::Null),
    ("Settings: edit the config file", ",", KeyCode::Char(',')),
    ("Back to the merge request list", "q", KeyCode::Char('q')),
];

pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let mut word: Vec<char> = word.chars().collect();
        let last = lines.last_mut().unwrap();
        let used = last.chars().count();
        if used > 0 && used + 1 + word.len() <= width {
            last.push(' ');
            last.extend(&word);
            continue;
        }
        if used > 0 {
            lines.push(String::new());
        }
        while word.len() > width {
            let rest = word.split_off(width);
            lines.last_mut().unwrap().extend(&word);
            lines.push(String::new());
            word = rest;
        }
        lines.last_mut().unwrap().extend(&word);
    }
    lines
}

fn editor_area(screen: Rect) -> Rect {
    Rect { y: screen.y + 1, height: screen.height.saturating_sub(1), ..screen }
}

type Button = (&'static str, &'static str, KeyCode);

pub fn popup(frame: &mut Frame, title: &'static str, lines: Vec<Line>, buttons: &[Button]) -> (Rect, Vec<(Rect, KeyCode)>) {
    let width = frame.area().width.saturating_sub(4).min(84);
    let inner_width = width.saturating_sub(6).max(1) as usize;
    let height = lines.iter().map(|l| l.width().max(1).div_ceil(inner_width) as u16).sum::<u16>() + 6;
    let [_, row, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(height), Constraint::Fill(1)]).areas(frame.area());
    let [_, area, _] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(width), Constraint::Fill(1)]).areas(row);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Yellow))
        .title(Span::styled(title, Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(area);
    *POPUP.lock().unwrap() = inner;
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);

    let mut areas = Vec::new();
    let mut x = inner.x;
    let y = inner.bottom().saturating_sub(1);
    for (i, &(key, label, code)) in buttons.iter().enumerate() {
        let color = if i == 0 { Color::Green } else { Color::Gray };
        let text = format!(" {key} · {label} ");
        let button = Rect::new(x, y, text.chars().count() as u16, 1).intersection(inner);
        frame.render_widget(Span::styled(text, Style::new().fg(Color::Black).bg(color).add_modifier(Modifier::BOLD)), button);
        areas.push((button, code));
        x = button.right() + 2;
    }
    (inner, areas)
}

fn file_at(dir: &Path, github: bool, repo: &str, sha: &str, file: &str, folder: &str, absent: bool) -> Option<PathBuf> {
    let path = dir.join(folder).join(file);
    if !path.exists() {
        let content = if absent { String::new() } else { mrs::raw_file(repo, github, sha, file)? };
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, content).ok()?;
        let mut permissions = std::fs::metadata(&path).ok()?.permissions();
        permissions.set_readonly(true);
        let _ = std::fs::set_permissions(&path, permissions);
    }
    Some(path)
}

fn open_file(terminal: &mut DefaultTerminal, app: &mut App, wait_for_editor: bool) -> std::io::Result<()> {
    let (script, modal, more) = mrs::open_settings();
    if !modal {
        let quiet = std::process::Stdio::null;
        if let Err(e) = app.command(&script, Mode::Ask, true).stdin(quiet()).stdout(quiet()).stderr(quiet()).spawn() {
            app.note = format!("Could not run the open command ({e}): {script}");
        }
        return Ok(());
    }
    let area = editor_area(terminal.size()?.into());
    match (app.tabs.terms.iter().any(|tab| tab.name == "editor"), more) {
        (true, Some(more)) => {
            let tries = if wait_for_editor { 20 } else { 1 };
            let added = (0..tries).any(|attempt| {
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
                let ran = app.command(&more, Mode::Ask, true).stdin(std::process::Stdio::null()).output();
                ran.is_ok_and(|out| out.status.success())
            });
            if !added {
                app.note = format!("Could not add the file to the open editor: {more}");
            }
        }
        (true, None) => {}
        (false, _) => match term::Term::spawn(&app.command(&script, Mode::Ask, true), area.height, area.width) {
            Ok(term) => app.tabs.terms.push(Tab { name: "editor".into(), term, born: std::time::Instant::now(), typing: None }),
            Err(e) => app.note = format!("Could not start the editor: {e}"),
        },
    }
    if let Some(i) = app.tabs.terms.iter().position(|tab| tab.name == "editor") {
        (app.tabs.active, app.tabs.shown) = (i, true);
    }
    Ok(())
}

fn post(terminal: &mut DefaultTerminal, app: &mut App, choice: usize, body: String) -> std::io::Result<()> {
    let Some(source) = app.bundle.review.sources.first().filter(|s| !s.repo.is_empty() && !s.reference.is_empty()) else {
        app.note = "This bundle does not name a merge request to post to.".into();
        return Ok(());
    };
    let (repo, reference, head) = (source.repo.clone(), source.reference.clone(), source.head_sha.clone());
    let points = app.points();
    let raised_items: Vec<String> = points.iter().filter_map(|point| point.item.clone()).collect();
    let leftover_body = |app: &App, kept: &[&Point]| app.compose(kept);
    let general: Vec<&Point> = points.iter().filter(|p| p.reply_to.is_none() && (p.file.is_none() || p.line.is_none())).collect();
    let placed: Vec<&Point> = points.iter().filter(|p| p.reply_to.is_none() && p.file.is_some() && p.line.is_some()).collect();
    let general_body = (!general.is_empty()).then(|| leftover_body(app, &general));
    let jobs: Vec<(Option<String>, Option<(String, u32, bool, Option<u32>)>, String)> = points
        .iter()
        .map(|p| {
            let text = if p.heading.is_empty() { p.text.clone() } else { format!("**{}**\n\n{}", p.heading, p.text) };
            (p.reply_to.clone(), p.file.clone().zip(p.line).map(|(f, l)| (f, l, p.old, p.start)), text)
        })
        .collect();
    let threads_wanted = choice == 2 && !placed.is_empty();
    app.busy = Some("Posting to the merge request".into());
    let work = move || {
        let mut problems = Vec::new();
        let mut failed_inline = Vec::new();
        for (reply_to, place, text) in &jobs {
            let result = match (reply_to, place, threads_wanted) {
                (Some(thread), _, _) => mrs::post_reply(&repo, &reference, thread, text),
                (None, Some((file, line, old, start)), true) => {
                    let posted = mrs::post_on_line(&repo, &reference, &head, file, *start, *line, *old, text);
                    if posted.is_err() {
                        failed_inline.push(format!("- `{file}:{line}`: {}", text.replace('\n', "\n  ")));
                    }
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                problems.push(e);
            }
        }
        let note = match threads_wanted {
            false => Some(body),
            true => {
                let mut summary = general_body.unwrap_or_default();
                if !failed_inline.is_empty() {
                    summary += &format!("\n\nThese could not be placed on the diff:\n\n{}", failed_inline.join("\n"));
                }
                (!summary.trim().is_empty()).then_some(summary)
            }
        };
        if let Some(Err(e)) = note.map(|text| mrs::post_note(&repo, &reference, &text)) {
            problems.push(e);
        }
        if choice == 1 {
            if let Err(e) = mrs::approve(&repo, &reference) {
                problems.push(format!("approve: {e}"));
            }
        }
        problems
    };
    let problems = wait(terminal, |frame| draw(app, frame), work)?;
    app.busy = None;
    match problems.first() {
        Some(problem) => app.note = format!("Posting hit {} problems, check the merge request before retrying: {problem}", problems.len()),
        None => {
            for note in &mut app.state.notes {
                note.posted = true;
            }
            app.state.published.extend(raised_items);
            let how = ["published as one comment", "published and approved", "published as threads"][choice.min(2)];
            app.state.finished = Some(Finished { how: how.into(), posted: true });
            let _ = app.bundle.save_state(&app.state);
            app.note = "Published to the merge request.".into();
            app.fetch_threads();
            app.rebuild_rows();
        }
    }
    Ok(())
}

fn launch(terminal: &mut DefaultTerminal, app: &mut App, mode: Mode, target: Target) {
    let agents = mrs::agents();
    let (index, mut reuse) = match target {
        Target::Reuse(index) => (index, true),
        Target::New(index) => (index, false),
    };
    let Some(agent) = agents.get(index) else { return };
    if agent.modal {
        let area = editor_area(terminal.size().map(Rect::from).unwrap_or_default());
        let existing = app.tabs.terms.iter().position(|tab| tab.name == agent.name);
        let saved = app.session().filter(|s| s.agent == agent.name && !s.id.is_empty());
        match (existing, reuse, &saved, &agent.resume) {
            (Some(i), true, _, _) => {
                let command = app.command(&agent.run, mode, false);
                let prompt = command.get_envs().find(|(key, _)| *key == "REVUI_PROMPT").and_then(|(_, value)| value);
                app.tabs.terms[i].term.type_text(&prompt.unwrap_or_default().to_string_lossy());
                app.tabs.active = i;
            }
            (_, resuming, saved, resume) => {
                let resumed = match (resuming, saved, resume) {
                    (true, Some(saved), Some(resume)) => {
                        let mut command = app.command(resume, mode, false);
                        command.current_dir(&saved.cwd);
                        Some(command)
                    }
                    _ => None,
                };
                let command = resumed.unwrap_or_else(|| {
                    let cwd = std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned();
                    let session = Session { agent: agent.name.clone(), handle: String::new(), id: uuid(), cwd };
                    let _ = std::fs::write(app.bundle.dir.join("session.json"), serde_json::to_string(&session).unwrap_or_default());
                    app.command(&agent.run, mode, true)
                });
                existing.map(|i| app.tabs.terms.remove(i));
                let sends_itself = command.get_args().any(|arg| arg.to_string_lossy().contains("REVUI_PROMPT"));
                let prompt = command.get_envs().find(|(key, _)| *key == "REVUI_PROMPT").and_then(|(_, value)| value);
                let typing = (!sends_itself).then(|| prompt.unwrap_or_default().to_string_lossy().into_owned());
                match term::Term::spawn(&command, area.height, area.width) {
                    Ok(term) => app.tabs.terms.push(Tab { name: agent.name.clone(), term, born: std::time::Instant::now(), typing }),
                    Err(e) => return app.note = format!("Could not start the {} agent: {e}", agent.name),
                }
                app.tabs.active = app.tabs.terms.len() - 1;
            }
        }
        app.tabs.shown = true;
        app.checked.clear();
        return;
    }
    loop {
        let script = match (&agent.send, reuse) {
            (Some(send), true) => send,
            _ => {
                reuse = false;
                &agent.run
            }
        };
        let mut command = app.command(script, mode, !reuse);
        command.stdin(std::process::Stdio::null());
        app.busy = Some(format!("{} the {} agent", if reuse { "Sending to" } else { "Starting" }, agent.name));
        let ran = wait(terminal, |frame| draw(app, frame), move || command.output());
        app.busy = None;
        let result = match ran.and_then(|output| output) {
            Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
            Ok(out) => {
                let said = String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
                Err(said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_string())
            }
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(_) if reuse => {
                app.checked.clear();
                app.note = format!("Sent to the {} companion.", agent.name);
            }
            Ok(handle) => {
                let session = Session { agent: agent.name.clone(), handle, id: String::new(), cwd: String::new() };
                let _ = std::fs::write(app.bundle.dir.join("session.json"), serde_json::to_string(&session).unwrap_or_default());
                app.checked.clear();
                app.note = format!("Started a companion: {}.", agent.name);
            }
            Err(_) if reuse => {
                reuse = false;
                continue;
            }
            Err(e) => app.note = format!("Could not start the {} agent: {e}", agent.name),
        }
        return;
    }
}

pub fn copy(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let _ = write!(std::io::stdout(), "\x1b]52;c;{encoded}\x07").and_then(|_| std::io::stdout().flush());
    if cfg!(target_os = "macos") {
        let pbcopy = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn();
        if let Ok(mut child) = pbcopy {
            let _ = child.stdin.take().map(|mut stdin| stdin.write_all(text.as_bytes()));
            let _ = child.wait();
        }
    }
}

fn shot(target: &str, out: &str, size: &str) -> Result<(), String> {
    let candidates = [
        std::env::var("REVUI_CHROME").unwrap_or_default(),
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
        "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
        "google-chrome".into(),
        "chromium".into(),
    ];
    let found = |name: &&String| match name.contains('/') {
        true => Path::new(name.as_str()).exists(),
        false => !name.is_empty() && std::process::Command::new(name.as_str()).arg("--version").output().is_ok(),
    };
    let chrome = candidates.iter().find(found).ok_or("no Chrome or Chromium found: set REVUI_CHROME to its path")?;
    let url = match target.contains("://") {
        true => target.to_string(),
        false => format!("file://{}", std::fs::canonicalize(target).map_err(|e| format!("{target}: {e}"))?.display()),
    };
    let _ = std::fs::remove_file(out);
    let ran = std::process::Command::new(chrome)
        .args(["--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=2", "--virtual-time-budget=8000"])
        .arg(format!("--window-size={}", size.replace('x', ",")))
        .arg(format!("--screenshot={out}"))
        .arg(url)
        .output()
        .map_err(|e| e.to_string())?;
    match Path::new(out).exists() {
        true => Ok(()),
        false => Err(format!("Chrome wrote no screenshot: {}", String::from_utf8_lossy(&ran.stderr).lines().last().unwrap_or(""))),
    }
}

fn diagram(source: &str, out: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(source).map_err(|e| format!("{source}: {e}"))?;
    let escaped = text.replace('&', "&amp;").replace('<', "&lt;");
    let page = std::env::temp_dir().join(format!("revui-diagram-{}.html", std::process::id()));
    let html = format!(
        "<!doctype html><meta charset=utf-8><body style='margin:0;background:#16181d;padding:32px'><pre class=mermaid>{escaped}</pre>\
         <script type=module>import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.esm.min.mjs';\
         mermaid.initialize({{startOnLoad:true,theme:'dark',flowchart:{{useMaxWidth:false}},sequence:{{useMaxWidth:false}}}});</script>"
    );
    std::fs::write(&page, html).map_err(|e| e.to_string())?;
    let captured = shot(&page.to_string_lossy(), out, "2400x1600");
    let _ = std::fs::remove_file(&page);
    captured?;
    let picture = image::open(out).map_err(|e| e.to_string())?.to_rgba8();
    let background = *picture.get_pixel(0, 0);
    let inked: Vec<(u32, u32)> = picture.enumerate_pixels().filter(|(_, _, p)| **p != background).map(|(x, y, _)| (x, y)).collect();
    let (Some(left), Some(right)) = (inked.iter().map(|p| p.0).min(), inked.iter().map(|p| p.0).max()) else {
        return Err("the diagram rendered empty: check the Mermaid source".into());
    };
    let (top, bottom) = (inked.iter().map(|p| p.1).min().unwrap_or(0), inked.iter().map(|p| p.1).max().unwrap_or(0));
    let pad = 32;
    let (x, y) = (left.saturating_sub(pad), top.saturating_sub(pad));
    let (w, h) = ((right + pad).min(picture.width() - 1) - x, (bottom + pad).min(picture.height() - 1) - y);
    image::imageops::crop_imm(&picture, x, y, w, h).to_image().save(out).map_err(|e| e.to_string())
}

static COPIED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

pub fn copied(count: usize) -> String {
    *COPIED.lock().unwrap() = Some(std::time::Instant::now());
    format!("Copied {count} characters.")
}

pub fn fade(message: &mut String) {
    if message.starts_with("Copied ") && COPIED.lock().unwrap().is_none_or(|at| at.elapsed().as_millis() >= 1200) {
        message.clear();
    }
}

static ACCENT: std::sync::Mutex<Color> = std::sync::Mutex::new(Color::Cyan);

pub fn accent() -> Color {
    *ACCENT.lock().unwrap()
}

pub fn set_accent(name: &str) {
    *ACCENT.lock().unwrap() = name.trim().parse().unwrap_or(Color::Cyan);
}

pub fn spinner() -> Span<'static> {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let frames = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];
    let frame = START.get_or_init(std::time::Instant::now).elapsed().as_millis() / 120;
    Span::styled(frames[frame as usize % frames.len()], Style::new().fg(accent()).add_modifier(Modifier::BOLD))
}

pub fn wait<T: Send>(
    terminal: &mut DefaultTerminal,
    mut draw: impl FnMut(&mut Frame),
    work: impl FnOnce() -> T + Send,
) -> std::io::Result<T> {
    std::thread::scope(|scope| {
        let worker = scope.spawn(work);
        while !worker.is_finished() {
            terminal.draw(&mut draw)?;
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        Ok(worker.join().expect("the background work panicked"))
    })
}

pub fn enter() -> DefaultTerminal {
    let terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), event::EnableMouseCapture);
    terminal
}

pub fn leave() {
    let _ = ratatui::crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
    ratatui::restore();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::cursor::Show);
}

fn picker() -> Picker {
    let speaks_kitty = ["TERM_PROGRAM", "TERM"]
        .iter()
        .any(|v| std::env::var(v).is_ok_and(|s| s.contains("ghostty") || s.contains("kitty")));
    let opted_out = std::env::var("REVUI_IMAGES").is_ok_and(|s| s == "halfblocks");
    let cell = ratatui::crossterm::terminal::window_size().ok().filter(|size| size.width > 0 && size.height > 0 && size.columns > 0 && size.rows > 0);
    let known = cell.filter(|_| speaks_kitty && !opted_out).map(|size| (size.width / size.columns, size.height / size.rows));
    #[allow(deprecated)]
    let mut picker = match known {
        Some(font) => Picker::from_fontsize(font.into()),
        None => {
            let asked = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
            while event::poll(std::time::Duration::from_millis(60)).unwrap_or(false) {
                let _ = event::read();
            }
            asked
        }
    };
    if picker.protocol_type() == ProtocolType::Halfblocks && speaks_kitty && !opted_out {
        picker.set_protocol_type(ProtocolType::Kitty);
    }
    picker.set_background_color(Some([0, 0, 0, 0]));
    picker
}

pub fn review(terminal: &mut DefaultTerminal, bundle: Bundle, images: Picker, tabs: Tabs) -> std::io::Result<Tabs> {
    let mut guide = bundle.dir.join("review.json");
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut stamp = modified(&guide);
    let mut app = App::new(bundle, images.clone());
    app.tabs = tabs;
    if app.bundle.successor.is_some() {
        app.ask_to_generate();
    }
    let mut redraw = true;
    let mut queued: Option<Event> = None;
    let mut covered = false;
    loop {
        if covered != (app.layered() && app.evidence_path().is_some()) {
            covered = !covered;
            terminal.clear()?;
            redraw = true;
        }
        if redraw {
            terminal.draw(|frame| draw(&mut app, frame))?;
        }
        redraw = true;
        if let Some(text) = app.clipboard.take() {
            copy(&text);
        }
        if let Some(text) = app.tabs.clipboard.take() {
            copy(&text);
        }
        let settings = app.tabs.has("settings");
        let failed = app.tabs.tick();
        if settings && !app.tabs.has("settings") {
            mrs::apply_accent();
        }
        for name in failed {
            app.note = format!("{name} exited right away with an error. Check its command in the config (press ,).");
            if !["editor", "settings"].contains(&name.as_str()) {
                let _ = std::fs::remove_file(app.bundle.dir.join("session.json"));
            }
        }
        app.absorb_highlights();
        fade(&mut app.note);
        let code = match app.pending.take() {
            Some(code) => code,
            None => {
                if queued.is_none() && !event::poll(std::time::Duration::from_millis(if app.tabs.shown { 30 } else if app.incoming.is_some() || app.threads_rx.is_some() { 40 } else if app.note.starts_with("Copied ") { 150 } else if app.busy.is_some() { 100 } else { 500 }))? {
                    redraw = !app.tabs.shown || app.tabs.terms.iter().fold(false, |changed, tab| tab.term.changed() | changed);
                    app.tick();
                    let newer = app.bundle.successor.clone().filter(|dir| mrs::has_guide(dir));
                    if let Some(bundle) = newer.and_then(|dir| Bundle::load(&dir).ok()) {
                        let (job, tabs) = (app.job.take(), std::mem::take(&mut app.tabs));
                        guide = bundle.dir.join("review.json");
                        stamp = modified(&guide);
                        app = App::new(bundle, images.clone());
                        (app.job, app.tabs) = (job, tabs);
                        app.note = "Loaded the guide for the current head.".into();
                    }
                    if modified(&guide) != stamp {
                        stamp = modified(&guide);
                        if let Ok(mut bundle) = Bundle::load(&app.bundle.dir) {
                            bundle.successor = app.bundle.successor.clone();
                            let (job, tabs) = (app.job.take(), std::mem::take(&mut app.tabs));
                            app = App::new(bundle, images.clone());
                            (app.job, app.tabs) = (job, tabs);
                            app.note = "Guide loaded.".into();
                        }
                    }
                    continue;
                }
                let moving = |event: &Event| matches!(event, Event::Mouse(m) if matches!(m.kind, MouseEventKind::Drag(_) | MouseEventKind::Moved));
                let mut event = match queued.take() {
                    Some(event) => event,
                    None => event::read()?,
                };
                while moving(&event) && event::poll(std::time::Duration::ZERO)? {
                    let next = event::read()?;
                    match moving(&next) {
                        true => event = next,
                        false => {
                            queued = Some(next);
                            break;
                        }
                    }
                }
                if app.tabs.shown && !app.tabs.terms.is_empty() {
                    app.tabs.handle(event);
                    continue;
                }
                let offered = app.input.as_ref().and_then(|input| app.mention(input));
                if let (Event::Key(key), Some(input)) = (&event, app.input.as_mut()) {
                    let quitting = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                    if let (KeyEventKind::Press, KeyCode::Tab, Some((at, names))) = (key.kind, key.code, &offered) {
                        let mut chars: Vec<char> = input.text.chars().collect();
                        let name: Vec<char> = format!("@{} ", names[0]).chars().collect();
                        chars.splice(*at..input.cursor.min(chars.len()), name.iter().copied());
                        (input.text, input.cursor) = (chars.into_iter().collect(), at + name.len());
                        continue;
                    }
                    if key.kind == KeyEventKind::Press && !quitting && !matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                        edit(&mut input.text, &mut input.cursor, *key);
                        continue;
                    }
                }
                if let (Event::Key(key), Some((filter, selected))) = (&event, app.palette.as_mut()) {
                    let quitting = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                    if key.kind == KeyEventKind::Press && !quitting && !matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Up | KeyCode::Down) {
                        let before = filter.clone();
                        edit(filter, &mut app.palette_cursor, *key);
                        if *filter != before {
                            *selected = 0;
                        }
                        continue;
                    }
                }
                let layered = app.confirm.is_some() || app.menu.is_some() || app.palette.is_some() || app.input.is_some() || app.finish.is_some() || app.help;
                match event {
                    Event::Mouse(m) if layered => {
                        let at = Position::new(m.column, m.row);
                        let hit = app.buttons.iter().find(|(area, _)| area.contains(at)).map(|&(_, code)| code);
                        match (m.kind, hit) {
                            (MouseEventKind::Down(_), Some(code)) => {
                                let run = app.palette.is_some() && !matches!(code, KeyCode::Enter | KeyCode::Esc);
                                if run {
                                    app.palette = None;
                                }
                                app.pending = Some(code);
                            }
                            (MouseEventKind::Down(_), None) if app.back_area.contains(at) => {
                                (app.confirm, app.menu, app.palette, app.input, app.finish, app.help) = (None, None, None, None, None, false);
                                app.pending = Some(KeyCode::Char('q'));
                            }
                            (MouseEventKind::Down(_), None) => (app.selection, app.modal_selection) = (None, Some((at, at))),
                            (MouseEventKind::Drag(_), _) => app.modal_selection = app.modal_selection.map(|(start, _)| (start, at)),
                            (MouseEventKind::Up(_), _) => match app.modal_selection {
                                Some((start, end)) if start != end => app.copy = true,
                                _ => app.modal_selection = None,
                            },
                            _ => {}
                        }
                        continue;
                    }
                    Event::Mouse(m) => {
                        app.mouse(m);
                        continue;
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            leave();
                            std::process::exit(0);
                        }
                        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyCode::F(12),
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyCode::F(9),
                        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::Char(c.to_ascii_uppercase()),
                        code => code,
                    },
                    _ => continue,
                }
            }
        };
        app.modal_selection = None;
        if app.help {
            app.help = false;
            continue;
        }
        if let Some(mut input) = app.input.take() {
            match code {
                KeyCode::Esc => {}
                KeyCode::Enter => app.submit_note(input),
                KeyCode::Backspace => {
                    input.text.pop();
                    app.input = Some(input);
                }
                KeyCode::Char(c) => {
                    input.text.push(c);
                    app.input = Some(input);
                }
                _ => app.input = Some(input),
            }
            continue;
        }
        match app.finish.take() {
            Some(Finish::Menu(selected)) => {
                let chosen = match code {
                    KeyCode::Enter => Some(selected),
                    KeyCode::Char(n @ '1'..='4') => Some(n as usize - '1' as usize),
                    KeyCode::Char('j') | KeyCode::Down => {
                        app.finish = Some(Finish::Menu((selected + 1).min(FINISH.len() - 1)));
                        None
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.finish = Some(Finish::Menu(selected.saturating_sub(1)));
                        None
                    }
                    KeyCode::Esc | KeyCode::Char('n' | 'q') => None,
                    _ => {
                        app.finish = Some(Finish::Menu(selected));
                        None
                    }
                };
                if let Some(choice) = chosen {
                    let points = app.points();
                    let body = app.compose(&points.iter().filter(|p| p.reply_to.is_none()).collect::<Vec<_>>());
                    match choice {
                        3 => {
                            app.clipboard = Some(body);
                            app.note = "Review text copied. Nothing was posted.".into();
                        }
                        _ => app.finish = Some(Finish::Preview(choice, body)),
                    }
                }
                continue;
            }
            Some(Finish::Preview(choice, body)) => {
                match code {
                    KeyCode::Char('y') => post(terminal, &mut app, choice, body)?,
                    KeyCode::Char('n' | 'q') | KeyCode::Esc => app.finish = Some(Finish::Menu(choice)),
                    _ => app.finish = Some(Finish::Preview(choice, body)),
                }
                continue;
            }
            None => {}
        }
        if app.palette.is_some() {
            app.pending = palette_key(&COMMANDS, &mut app.palette, code);
            continue;
        }
        if let Some((mode, selected)) = app.menu.take() {
            let options = app.options();
            let last = options.len().saturating_sub(1);
            match code {
                KeyCode::Char('j') | KeyCode::Down => app.menu = Some((mode, (selected + 1).min(last))),
                KeyCode::Char('k') | KeyCode::Up => app.menu = Some((mode, selected.saturating_sub(1))),
                KeyCode::Enter => launch(terminal, &mut app, mode, options[selected].1),
                KeyCode::Char(n @ '1'..='9') if (n as usize - '1' as usize) <= last => {
                    launch(terminal, &mut app, mode, options[n as usize - '1' as usize].1)
                }
                KeyCode::Esc | KeyCode::Char('n' | 'q') => {}
                _ => app.menu = Some((mode, selected)),
            }
            continue;
        }
        if app.confirm.is_some() {
            app.key(code);
            continue;
        }
        match code {
            KeyCode::Char('/') => (app.palette, app.palette_cursor) = (Some((String::new(), 0)), 0),
            KeyCode::Char('?') => app.help = true,
            KeyCode::F(12) if app.tabs.terms.is_empty() => app.note = "No agent or editor is open: t starts an agent, enter opens a file.".into(),
            KeyCode::F(12) => app.tabs.open(),
            KeyCode::Char(n @ '1'..='9') if (n as usize - '1' as usize) < app.tabs.terms.len() => {
                (app.tabs.active, app.tabs.shown) = (n as usize - '1' as usize, true);
            }
            KeyCode::Char(',') => {
                if let Err(e) = app.tabs.run("settings", &mrs::config_editor(), terminal.size()?.into()) {
                    app.note = format!("Could not open the config in your editor: {e}");
                }
            }
            KeyCode::Char(c @ ('t' | 'v' | 'i' | 'T' | 'V' | 'I')) => {
                let mode = match c.to_ascii_lowercase() {
                    'v' => Mode::Verify,
                    'i' => Mode::Artifact,
                    _ => Mode::Ask,
                };
                if mode != Mode::Ask && app.bundle.review.items.is_empty() {
                    app.note = "Nothing to work on: this merge request has no findings yet.".into();
                } else if c.is_ascii_uppercase() {
                    app.menu = Some((mode, 0));
                } else {
                    let target = app.options()[0].1;
                    launch(terminal, &mut app, mode, target);
                }
            }
            KeyCode::Enter if app.focus == 0 && matches!(app.rows.get(app.sel), Some(Row::Thread(_) | Row::Heading)) => {
                let url = app.bundle.review.sources.first().map_or("", |s| s.url.as_str());
                let anchor = match app.rows.get(app.sel) {
                    Some(&Row::Thread(i)) => app.threads[i].anchor.clone(),
                    _ => String::new(),
                };
                match url.is_empty() {
                    true => app.note = "This bundle names no merge request.".into(),
                    false => {
                        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                        let _ = std::process::Command::new(opener).arg(format!("{url}{anchor}")).spawn();
                    }
                }
            }
            KeyCode::Enter if app.focus == 0 => app.focus = 1,
            KeyCode::Enter if app.focus == 1 && app.item().is_some_and(|item| !item.evidence.is_empty()) => app.toggle_evidence(false),
            KeyCode::Enter | KeyCode::Char('o') if !app.artifact_at_cursor() => match app.open_target() {
                Some(_) => open_file(terminal, &mut app, false)?,
                None => app.note = "This is not on a file, so there is nothing to open in the editor.".into(),
            },
            KeyCode::Char('O') => {
                let files = app.section_files();
                if files.len() > 1 {
                    let fetch = app.prefetch(&files);
                    app.busy = Some(format!("Fetching {} files", files.len()));
                    wait(terminal, |frame| draw(&mut app, frame), fetch)?;
                    app.busy = Some(format!("Opening {} files", files.len()));
                    terminal.draw(|frame| draw(&mut app, frame))?;
                    app.busy = None;
                }
                for (n, target) in files.iter().enumerate() {
                    app.open_at = Some(*target);
                    open_file(terminal, &mut app, n > 0)?;
                }
                app.open_at = None;
                if files.is_empty() {
                    app.note = "There are no files in this part of the diff to open.".into();
                }
            }
            code => {
                if !app.key(code) {
                    app.tabs.terms.retain(|tab| tab.name == "settings" || tab.name.starts_with("guide"));
                    return Ok(app.tabs);
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    mrs::apply_accent();
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("probe"), _) => {
            let picker = picker();
            println!("{:?} {:?} {:?}", picker.protocol_type(), picker.font_size(), picker.capabilities());
            let _ = ratatui::crossterm::terminal::enable_raw_mode();
            let held = ratatui::crossterm::terminal::supports_keyboard_enhancement();
            let _ = ratatui::crossterm::terminal::disable_raw_mode();
            println!("key release events (hold ctrl, q for the next tab): {held:?}");
            return Ok(());
        }
        (Some("format"), _) => return Ok(print!("{}", include_str!("../FORMAT.md"))),
        (Some("fetch"), Some(url)) => return Ok(println!("{}", mrs::fetch(url)?.display())),
        (Some("hunks"), Some(dir)) => return Ok(print!("{}", Bundle::load(Path::new(dir))?.hunks())),
        (Some("check"), Some(dir)) => {
            let (ok, report) = Bundle::load(Path::new(dir))?.check();
            print!("{report}");
            std::process::exit(if ok { 0 } else { 1 });
        }
        (Some("shot"), Some(target)) => {
            let out = args.get(2).ok_or("usage: revui shot <url or html file> <out.png> [WIDTHxHEIGHT]")?;
            return Ok(shot(target, out, args.get(3).map_or("1280x800", String::as_str))?);
        }
        (Some("diagram"), Some(source)) => {
            return Ok(diagram(source, args.get(2).ok_or("usage: revui diagram <file.mmd> <out.png>")?)?);
        }
        (Some("reset"), _) => {
            match mrs::reset_config() {
                Some(backup) => println!("Settings reset. Your old config is at {}\nRun revui to set up again.", backup.display()),
                None => println!("There is no config to reset. Run revui to set up."),
            }
            return Ok(());
        }
        (Some("setup"), Some(flag)) if flag == "--guide" => return Ok(print!("{SETUP}")),
        (Some("setup"), _) => {
            if !mrs::setup_with_agent() {
                println!("Could not start a coding agent. Paste this into yours:\n\n  {SETUP_PROMPT}");
            }
            return Ok(());
        }
        (Some("skill"), Some(action)) if action == "install" => {
            return Ok(println!("installed revui, revui-drive and revui-guide in {}", mrs::install_skill()?.display()));
        }
        (Some("skill"), Some(which)) if which == "drive" => return Ok(print!("{DRIVE}")),
        (Some("skill"), Some(which)) if which == "guide" => return Ok(print!("{GUIDE}")),
        (Some("skill"), _) => return Ok(print!("{SKILL}")),
        (Some("drive"), dir) => return Ok(print!("{}", mrs::drive_report(dir.map(Path::new)))),
        (Some("doctor"), _) => {
            use std::io::IsTerminal;
            let images = match std::io::stdin().is_terminal() {
                true => format!("{:?}", picker().protocol_type()),
                false => "not checked, run this in the terminal you will use".into(),
            };
            let (ok, report) = mrs::doctor(&images);
            print!("{report}");
            std::process::exit(if ok { 0 } else { 1 });
        }
        (Some("help" | "--help" | "-h"), _) => {
            println!(
                "revui                   pick an open merge request\n\
                 revui <dir>             open a bundle\n\
                 revui <dir> --export    print the reviewer's verdicts\n\
                 revui fetch <mr url>    create the bundle for a merge request, print its directory\n\
                 revui hunks <dir>       list the diff's files and hunk indexes\n\
                 revui check <dir>       validate a bundle, exit 1 on problems\n\
                 revui shot <url|file> <out.png> [WxH]   screenshot a page in headless Chrome\n\
                 revui diagram <file.mmd> <out.png>      render a Mermaid diagram to an image\n\
                 revui format            print the bundle format\n\
                 revui skill             print the guide agents follow to read and update a bundle\n\
                 revui skill guide       print the guide for reviewing a change and writing its guide\n\
                 revui skill drive       print the guide for running and driving the product in the background\n\
                 revui skill install     install all three as Claude Code skills\n\
                 revui drive [dir]       what to drive a browser or app with here, and the notes for that bundle's repo\n\
                 revui doctor            check the setup, exit 1 when something needs fixing\n\
                 revui setup             have a coding agent set revui up for you\n\
                 revui setup --guide     print the setup guide that agent follows\n\
                 revui reset             set the config aside (kept as a backup) to start setup again"
            );
            return Ok(());
        }
        _ => {}
    }
    let dir = args.iter().find(|a| !a.starts_with("--"));
    let bundle = dir.map(|d| Bundle::load(Path::new(d))).transpose()?;
    if let (Some(bundle), true) = (&bundle, args.iter().any(|a| a == "--export")) {
        print!("{}", bundle.export(&bundle.load_state()));
        return Ok(());
    }
    let mut terminal = enter();
    let images = picker();
    let result = match bundle {
        Some(bundle) => review(&mut terminal, bundle, images, Tabs::default()).map(|_| ()),
        None => mrs::browse(&mut terminal, images),
    };
    leave();
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn demo_bundle_surfaces_hunks_the_guide_left_out() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/demo");
        let bundle = Bundle::load(&dir).unwrap();
        assert!(bundle.has_unsectioned());
        assert_eq!(bundle.groups.last().unwrap().len(), 1);

        let mut app = App::new(bundle, Picker::halfblocks());
        app.panes = Panes::default();
        assert!(matches!(app.rows[app.sel], Row::Overview) && !app.diff_shown(), "a guide opens on its overview");
        let overview: String = detail(&app, 80).iter().map(|(line, _)| format!("{line}\n")).collect();
        assert!(overview.contains(&app.bundle.review.summary[..20]) && overview.contains("Read in this order") && overview.contains("1. "));
        let mut terminal = Terminal::new(TestBackend::new(140, 70)).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let second = app.detail_parts.iter().position(|part| *part == Some(1)).expect("the overview lists the second part");
        app.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: app.detail_area.x + 8, row: app.detail_area.y + 1 + second as u16, modifiers: KeyModifiers::NONE });
        assert!(matches!(app.rows[app.sel], Row::Group(1)) && app.focus == 0, "clicking a part in the overview goes to it");
        app.select(0);
        let finding = app.rows.iter().position(|r| matches!(r, Row::Item(_))).unwrap();
        app.select(finding);
        let (lines, anchor, _) = app.diffed.clone();
        assert!(lines[anchor].to_string().contains("attempt"));
        while app.incoming.is_some() {
            app.absorb_highlights();
        }
        assert!(app.diffed.0[app.diffed.1].spans.len() > 3, "the anchored line is split into highlighted tokens");
        for extension in ["ts", "tsx", "tf", "java", "md", "json", "yml"] {
            assert!(syntax().0.find_syntax_by_extension(extension).is_some(), "no syntax for .{extension}");
        }
        app.key(KeyCode::BackTab);
        assert_eq!(app.focus, 2, "shift-tab from the guide wraps back to the diff");
        app.focus = 1;
        let pictured = app.item().unwrap().evidence.iter().position(|evidence| evidence.path.is_some()).unwrap();
        app.evidence = pictured;
        assert!(app.artifact_at_cursor(), "o in the detail pane opens the artifact");
        app.focus = 2;
        assert!(!app.artifact_at_cursor(), "in the diff pane it opens the file");
        app.focus = 0;
        assert!(app.artifact_at_cursor(), "with an artifact on screen, o opens it from the guide list too");
        app.focus = 1;
        app.key(KeyCode::Tab);
        assert_eq!(app.focus, 3, "tab goes from the detail pane to the artifact when there is one");
        app.key(KeyCode::Tab);
        assert_eq!(app.focus, 2);
        app.focus = 0;
        app.focus = 1;
        app.detail_area = Rect::new(40, 3, 60, 8);
        app.key(KeyCode::Char('j'));
        assert_eq!(app.evidence, 1, "j in the detail pane moves to the next evidence entry");
        let rows = detail(&app, 58);
        let cursor = rows.iter().position(|(line, _)| line.to_string().starts_with("> ")).unwrap() as u16;
        assert!((app.detail_scroll..app.detail_scroll + 6).contains(&cursor), "the selected entry stays in view");
        app.key(KeyCode::Char('k'));
        app.detail_scroll = 0;
        app.focus = 0;
        app.key(KeyCode::Char(' '));
        assert!(app.checked.is_empty(), "space does nothing outside the detail pane");
        app.focus = 1;
        app.key(KeyCode::Char(' '));
        app.focus = 0;

        let mut terminal = Terminal::new(TestBackend::new(140, 70)).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let screen = format!("{}", terminal.backend());
        assert!(screen.contains("no section"));
        let [_, text, diff] = app.areas;
        assert!(app.panes.side && !app.side && diff.y == text.bottom(), "with an artifact showing, the diff goes under the detail pane");
        assert!(app.picture_area.x == text.right() && app.picture_area.bottom() == diff.y && diff.width > text.width, "and spans under the artifact beside it");
        app.key(KeyCode::Char('s'));
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        assert!(app.panes.side_with_artifact && app.side && app.areas[2].x >= app.picture_area.right(), "s puts the diff beside the artifact");
        app.key(KeyCode::Char('s'));
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        assert!(!app.side && app.panes.side, "and back, without touching the layout used when there is no artifact");
        app.key(KeyCode::Char('s'));
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let (text_before, diff_before) = (app.areas[1], app.areas[2]);
        let seam = app.picture_area.x;
        let drag = |app: &mut App, kind, column| app.mouse(MouseEvent { kind, column, row: text_before.y + 3, modifiers: KeyModifiers::NONE });
        drag(&mut app, MouseEventKind::Down(MouseButton::Left), seam);
        drag(&mut app, MouseEventKind::Drag(MouseButton::Left), seam + 10);
        drag(&mut app, MouseEventKind::Up(MouseButton::Left), seam + 10);
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        assert!(app.areas[1].width > text_before.width && app.areas[2] == diff_before, "dragging a divider resizes only the two panes beside it");
        app.key(KeyCode::Char('s'));
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        assert!(!screen.contains("What it says") && screen.contains("· Evidence (2)"));
        assert!(screen.contains("observed") && screen.contains("observed ▣  ✓") && !screen.contains("[ ]") && screen.contains("1 picked · v verify · i artifact") && fit(&["space/enter pick", "ctrl-o details", "j/k evidence"], 40) == " space/enter pick · ctrl-o details ");
        let env: Vec<_> = app.command("true", Mode::Verify, false).get_envs().map(|(_, v)| v.unwrap().to_string_lossy().into_owned()).collect();
        assert!(env.iter().any(|e| e.starts_with("Verify these claims: B1") && e.contains("Orders list after one checkout")));
        let row = app.detail_area.y + 1 + app.detail_rows.iter().position(|n| *n == Some(0)).unwrap() as u16;
        let click = |app: &mut App, row: u16| {
            app.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 45, row, modifiers: KeyModifiers::NONE });
            app.mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: 45, row, modifiers: KeyModifiers::NONE });
        };
        let before = detail(&app, 80).len();
        let fold = app.detail_area.y + 1 + app.detail_folds.iter().position(|fold| *fold).unwrap() as u16;
        click(&mut app, fold);
        assert!(app.opened.len() == 1 && app.checked.len() == 1 && detail(&app, 80).len() > before, "clicking the details row opens it");
        click(&mut app, row);
        assert!(app.checked.is_empty(), "clicking the picked evidence line unpicks it");

        let mut short = Terminal::new(TestBackend::new(140, 12)).unwrap();
        short.draw(|f| draw(&mut app, f)).unwrap();
        assert!(format!("{}", short.backend()).contains('█'), "a pane with more than fits shows a scrollbar thumb");
        assert!(!screen.contains('█'), "and none when everything fits");
        let mut narrow = Terminal::new(TestBackend::new(30, 20)).unwrap();
        narrow.draw(|f| draw(&mut app, f)).unwrap();
        assert_eq!(app.panes.guide, Panes::default().guide, "a narrow window does not shrink the saved guide width");
        app.panes.guide = 24;
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let long = app.row_heights.iter().position(|height| *height > 1).expect("a narrow guide pane wraps its long rows");
        let above: u16 = app.row_heights[..long].iter().sum();
        app.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row: app.areas[0].y + 2 + above, modifiers: KeyModifiers::NONE });
        assert_eq!(app.sel, long, "clicking the second line of a wrapped row selects that row");
    }

    #[test]
    fn a_merge_request_without_a_guide_opens_as_its_plain_diff() {
        let dir = std::env::temp_dir().join(format!("revui-test-{}", std::process::id()));
        let patch = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/demo/mr.patch")).unwrap();
        mrs::sample().write_bundle(&dir, &patch).unwrap();
        let mut app = App::new(Bundle::load(&dir).unwrap(), Picker::halfblocks());
        std::fs::remove_dir_all(&dir).unwrap();
        app.panes = Panes { side: false, ..Panes::default() };
        assert_eq!(app.bundle.groups.len(), 2);

        let mut terminal = Terminal::new(TestBackend::new(140, 20)).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let screen = format!("{}", terminal.backend());
        assert!(screen.contains("api !7 · Retry order submission"));
        assert!(screen.contains("https://example.com/shop/api/-/merge_requests/7 · head 5d6e7f8a · @sam · retry-submit → main"));

        let command = app.command("true", Mode::Verify, true);
        let env: Vec<_> = command.get_envs().map(|(k, v)| format!("{}={}", k.display(), v.unwrap().display())).collect();
        assert!(env.contains(&"REVUI_FILE_URL=https://example.com/shop/api/-/blob/5d6e7f8a9b0c/src/submit.ts#L10".to_string()));
        assert!(env.iter().any(|e| e.starts_with("REVUI_PROMPT=Verify every finding") && e.contains(&*dir.to_string_lossy()) && e.len() < 260));

        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let mouse = |kind, column, row| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
        app.mouse(click(3, 0));
        assert_eq!(app.pending.take(), Some(KeyCode::Char('q')), "the back button at the top left leaves the review");
        let pressed: Vec<KeyCode> = app.action_buttons.clone().into_iter().map(|(button, _)| {
            app.mouse(click(button.x + 1, button.y));
            app.pending.take().unwrap()
        }).collect();
        assert_eq!((pressed[0], pressed.contains(&KeyCode::Char('g'))), (KeyCode::Char('p'), true), "the publish and generate buttons act like their keys");
        assert!(pressed.contains(&KeyCode::Char('m')), "the merge request's name in the header opens it on the web");
        assert!(pressed.contains(&KeyCode::Char('?')) && pressed.contains(&KeyCode::Char('c')) && pressed.contains(&KeyCode::Tab), "and so does a click on a hint");
        let row = Rect::new(0, 0, 40, 1);
        assert_eq!(hint_areas("j/k move · enter/o open · , settings · f a very long hint that does not fit", row), [(Rect::new(12, 0, 12, 1), KeyCode::Enter), (Rect::new(27, 0, 10, 1), KeyCode::Char(','))]);
        assert!(format!("{}", terminal.backend()).contains(" g · Generate guide ") && format!("{}", terminal.backend()).contains(" p · Publish review "));
        app.mouse(click(41, 3));
        app.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 138, 4));
        app.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 138, 4));
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let copied = app.clipboard.take().unwrap();
        assert!(copied.starts_with("No guide for this merge request yet") && copied.ends_with("one file per row."));
        assert!(!copied.contains('\n'));

        app.mouse(click(5, 4));
        assert_eq!((app.sel, app.focus), (1, 0));
        app.mouse(click(80, 15));
        assert_eq!(app.focus, 2);

        app.key(KeyCode::Char('g'));
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let screen = format!("{}", terminal.backend());
        assert!(screen.contains("Generate a guide") && screen.contains("y · Start") && screen.contains("n · Cancel"));
        app.key(KeyCode::Char('z'));
        assert!(app.confirm.is_some());
        let (start, _) = app.buttons[0];
        assert_eq!(app.buttons[0].1, KeyCode::Char('y'));
        assert!(start.width > 0);
        app.key(KeyCode::Char('n'));
        app.palette = Some(("verify".into(), 0));
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        assert_eq!(matching(&COMMANDS, "verify where").len(), 1);
        let mut shown = Rows::new();
        body(&mut shown, "What breaks. The helper returns early.\n\nFailure path.\n1. First send\n# Suggest\nCount answered rows.", 60);
        let text: Vec<String> = shown.iter().map(|(line, _)| line.to_string()).collect();
        assert_eq!(text, ["What breaks", "The helper returns early.", "", "Failure path", "1. First send", "Suggest", "Count answered rows."]);
        assert_eq!(ago("2001-02-03T04:05:06.000Z"), "Feb 3");
        let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 - 2 * 3_600;
        let (days, seconds) = (epoch.div_euclid(86_400), epoch.rem_euclid(86_400));
        let z = days + 719_468;
        let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
        let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
        let stamp = format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:00Z", seconds / 3_600, seconds % 3_600 / 60);
        assert_eq!(ago(&stamp), "2h ago", "{stamp}");
        let press = |code, modifiers| event::KeyEvent::new(code, modifiers);
        let (mut text, mut cursor) = ("fix the retry bug".to_string(), 17);
        edit(&mut text, &mut cursor, press(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(cursor, 14, "option-left jumps a word");
        edit(&mut text, &mut cursor, press(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!((text.as_str(), cursor), ("fix the bug", 8), "option-backspace deletes a word");
        edit(&mut text, &mut cursor, press(KeyCode::Char('a'), KeyModifiers::CONTROL));
        edit(&mut text, &mut cursor, press(KeyCode::Char('!'), KeyModifiers::NONE));
        assert_eq!((text.as_str(), cursor), ("!fix the bug", 1), "line start, then typing inserts at the cursor");
        let colored = |text: &str| rich(text, Style::new()).into_iter().filter(|s| s.style.fg.is_some()).map(|s| s.content.into_owned()).collect::<Vec<_>>();
        let long = Line::from(vec![Span::raw("+ "), Span::styled("const a = 1;", Style::new().fg(Color::Red)), Span::raw(" // and a long trailing comment")]);
        let rows = wrap_code(long.clone(), 20);
        assert!(rows.len() == 3 && rows.iter().all(|row| row.width() <= 20) && rows[1].to_string().starts_with("  "));
        assert_eq!(rows.iter().enumerate().map(|(i, row)| row.to_string()[if i == 0 { 0 } else { 2 }..].to_string()).collect::<String>(), long.to_string());
        assert_eq!(wrap_code(long.clone(), 200).len(), 1);
        assert_eq!(colored("fix(ENG-542): see auth.tf:295-301 and !1045, not UTF-8 or A-1"), ["ENG-542", "auth.tf:295-301", "!1045,"]);
        assert_eq!(colored("(https://linear.app/x/issue/ENG-542/y) asks"), ["(https://linear.app/x/issue/ENG-542/y)"]);
        app.palette = None;
        app.confirm = Some(String::new());
        app.key(KeyCode::Char('n'));
        assert!(app.confirm.is_none() && app.job.is_none());
    }

    #[test]
    #[ignore]
    fn bench_moving_through_a_real_guide() {
        let named = std::env::var("REVUI_BENCH").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/examples/demo").into());
        for name in named.split(':') {
            let Ok(bundle) = Bundle::load(Path::new(name)) else { continue };
            let start = std::time::Instant::now();
            let mut app = App::new(bundle, Picker::halfblocks());
            println!("{name}: open {:?}, {} rows", start.elapsed(), app.rows.len());
            let start = std::time::Instant::now();
            while app.incoming.is_some() {
                app.absorb_highlights();
            }
            println!("  all highlighting finished in the background after {:?}", start.elapsed());
            let mut terminal = Terminal::new(TestBackend::new(220, 60)).unwrap();
            for row in 0..app.rows.len() {
                let t = std::time::Instant::now();
                app.select(row);
                let selected = t.elapsed();
                let t = std::time::Instant::now();
                terminal.draw(|f| draw(&mut app, f)).unwrap();
                let first = t.elapsed();
                let t = std::time::Instant::now();
                terminal.draw(|f| draw(&mut app, f)).unwrap();
                println!("  row {row}: select {selected:?}, first draw {first:?}, redraw {:?}, {} diff lines", t.elapsed(), app.diffed.0.len());
            }
        }
    }

    #[test]
    fn notes_land_on_the_diff_and_in_the_review_text() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/demo");
        let dir = std::env::temp_dir().join(format!("revui-notes-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        for file in ["review.json", "mr.patch", "assets/duplicate-orders.png"] {
            std::fs::copy(source.join(file), dir.join(file)).unwrap();
        }
        let mut app = App::new(Bundle::load(&dir).unwrap(), Picker::halfblocks());
        app.threads_rx = None;
        app.panes = Panes::default();
        let finding = app.rows.iter().position(|r| matches!(r, Row::Item(_))).unwrap();
        app.select(finding);
        app.key(KeyCode::Char('a'));
        let mut reason = app.input.take().expect("agreeing offers a note");
        reason.text = "A reason that should go when the mark does".into();
        app.submit_note(reason);
        app.key(KeyCode::Char('a'));
        assert!(app.state.notes.is_empty() && app.input.is_none(), "clearing the mark removes the comment that came with it");
        app.key(KeyCode::Char('x'));
        app.key(KeyCode::Char('R'));
        assert_eq!(app.state.verdicts.len(), 1, "one R only asks");
        app.key(KeyCode::Char('j'));
        app.key(KeyCode::Char('k'));
        app.key(KeyCode::Char('R'));
        assert_eq!(app.state.verdicts.len(), 1, "another key in between starts the asking over");
        app.key(KeyCode::Char('R'));
        assert!(app.state.verdicts.is_empty(), "R twice discards every mark");
        app.key(KeyCode::Char('a'));
        let mut reason = app.input.take().expect("agreeing offers a note");
        reason.text = "Seen this in prod last month".into();
        app.submit_note(reason);

        let mut terminal = Terminal::new(TestBackend::new(140, 60)).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let (row, yes, _) = app.verdict_row.expect("a finding shows its agree and dismiss buttons");
        let (left, top) = (app.detail_area.x + 1, app.detail_area.y + 1);
        let click = |app: &mut App, column: u16, row: u16| app.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE });
        click(&mut app, left + 2, top + row as u16);
        assert_eq!(app.pending.take(), Some(KeyCode::Char('a')));
        click(&mut app, left + yes + 3, top + row as u16);
        assert_eq!(app.pending.take(), Some(KeyCode::Char('x')), "the two buttons under a finding's title act like a and x");
        let comment = app.detail_notes.iter().position(|note| note.is_some()).expect("the comment shows under the finding");
        click(&mut app, left + 4, top + comment as u16);
        let editing = app.input.take().expect("clicking a comment opens it for editing");
        assert_eq!((editing.text.as_str(), editing.edit), ("Seen this in prod last month", Some(0)));
        let own = app.rows.iter().position(|row| matches!(row, Row::Note(_))).unwrap();
        let above: u16 = app.row_heights[app.list.offset()..own].iter().sum();
        let at = (app.areas[0].x + 6, app.areas[0].y + 1 + above);
        let tap = |app: &mut App, row: u16| {
            for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
                app.mouse(MouseEvent { kind, column: at.0, row, modifiers: KeyModifiers::NONE });
            }
        };
        app.focus = 0;
        tap(&mut app, at.1);
        assert!(app.input.is_none() && app.sel == own && app.focus == 0, "a click on a comment row selects it");
        tap(&mut app, at.1);
        assert_eq!(app.input.take().expect("a click on the selected comment opens it for editing").edit, Some(0));
        let finding = app.rows.iter().position(|row| matches!(row, Row::Item(_))).unwrap();
        let above: u16 = app.row_heights[app.list.offset()..finding].iter().sum();
        let row = app.areas[0].y + 1 + above;
        tap(&mut app, row);
        assert_eq!(app.pending.take(), None);
        tap(&mut app, row);
        assert_eq!(app.pending.take(), Some(KeyCode::Enter), "a click on any other selected row goes in, like enter");
        app.focus = 2;
        tap(&mut app, row);
        assert_eq!((app.pending.take(), app.focus), (None, 0), "coming back from another pane only focuses the guide");
        app.select(finding);

        app.focus = 2;
        app.areas[2] = Rect::new(40, 20, 100, 20);
        let added = app.diffed.2.iter().position(|mark| mark.sign == '+').unwrap();
        app.cursor = added;
        app.key(KeyCode::Char('n'));
        let mut input = app.input.take().unwrap();
        assert!(input.title.starts_with("Comment on src/submit.ts:"));
        input.text = "Needs an idempotency key".into();
        app.submit_note(input);
        let typing = |text: &str| Input { title: String::new(), text: text.into(), cursor: text.chars().count(), note: Note::default(), edit: None };
        app.bundle.review.sources[0].author = "@sam".into();
        assert_eq!(app.mention(&typing("thanks @s")), Some((7, vec!["sam".to_string()])));
        assert!(app.mention(&typing("mail@s")).is_none() && app.mention(&typing("@sam")).is_none() && app.mention(&typing("@zed")).is_none());
        app.picked_lines = Some((added, added + 2));
        app.key(KeyCode::Char('c'));
        let block = app.input.take().expect("c after dragging over lines comments on that block");
        let (first, last) = (block.note.start.expect("the block has a first line"), block.note.line.unwrap());
        assert!(last == first + 2 && block.title.ends_with(&format!(":{first}-{last}")) && app.picked_lines.is_none());
        assert_eq!(app.state.notes.len(), 2);
        assert!(app.diffed.0[added].to_string().starts_with("+●"), "the noted line carries a marker");
        assert!(app.rows.iter().any(|row| matches!(row, Row::Note(1))));

        app.threads = mrs::parse_threads(
            r#"[{"id": "abc", "notes": [{"system": false, "author": {"username": "sam"}, "body": "Why three attempts?",
                 "resolvable": true, "resolved": false, "position": {"new_path": "src/submit.ts", "new_line": 12, "old_line": null}}]},
                {"id": "sys", "notes": [{"system": true, "author": {"username": "bot"}, "body": "added 1 commit"}]}]"#,
        );
        app.rebuild_rows();
        assert_eq!(app.threads.len(), 1, "system-only discussions are skipped");
        let thread = app.rows.iter().position(|row| matches!(row, Row::Thread(_))).unwrap();
        app.select(thread);
        assert_eq!(app.group(), Some(0), "a thread selects the section that holds its line");
        app.focus = 0;
        app.key(KeyCode::Char('n'));
        let mut reply = app.input.take().unwrap();
        assert_eq!(reply.title, "Reply to @sam");
        assert!(app.rows.iter().any(|row| matches!(row, Row::Heading)));
        reply.text = "Agreed, see my note".into();
        app.submit_note(reply);

        let points = app.points();
        assert_eq!(points.len(), 3, "the agreed finding with its note folded in, the line note and the reply");
        assert!(points[0].text.ends_with("Seen this in prod last month"));
        assert!(points[0].text.starts_with("> **A retry can create the order twice**\n>\n> "), "the finding goes out as a quote");
        assert!(points[0].text.contains("\n\nSeen this in prod last month") && !points[0].text.contains("> Seen this"), "with the comment under it, unquoted");
        let body = app.compose(&points.iter().filter(|p| p.reply_to.is_none()).collect::<Vec<_>>());
        assert!(body.starts_with("Reviewed at `5d6e7f8a`."));
        assert!(body.contains("**A retry can create the order twice**") && body.contains("Needs an idempotency key"));
        assert!(!body.contains("Agreed, see my note"), "replies go to their thread, not into the note");
        assert!(!body.contains('—'));

        let mut terminal = Terminal::new(TestBackend::new(150, 44)).unwrap();
        app.finish = Some(Finish::Preview(0, body));
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let screen = format!("{}", terminal.backend());
        assert!(screen.contains("Post this review?") && screen.contains("y · Post to the merge request"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_guide_citing_code_outside_the_diff_is_rejected() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/bad-ref");
        assert!(Bundle::load(&dir).is_err_and(|e| e.contains("not in the diff")));
    }
}
