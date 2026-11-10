mod bundle;
mod draw;
mod input;
mod mrs;
mod tabs;
mod term;
#[cfg(test)]
mod tests;

use bundle::{Bundle, Finished, Item, Kind, Note, State};
use draw::{accent, ago, clear_popup, detail, draw, draw_help, draw_palette, editor_area, footer, hunk_lines, popup, popup_selection, rich, scrollbar, set_accent, spinner, ticket, wrap, wrap_code};
use input::{edit, palette_key, Command, COMMANDS};
use tabs::{Tab, Tabs};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::layout::{Constraint, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::ListState;
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use std::collections::HashMap;
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

}

fn items_in(bundle: &Bundle, section: Option<usize>) -> impl Iterator<Item = Row> + '_ {
    let items = bundle.review.items.iter().enumerate();
    items.filter(move |(_, i)| i.section == section).map(|(i, _)| Row::Item(i))
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
    let mut configuring = false;
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
        let failed = app.tabs.tick();
        let settings = app.tabs.has("settings");
        if std::mem::replace(&mut configuring, settings) && !settings {
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
                            let (row, focus, in_tab) = (app.rows.get(app.sel).copied(), app.focus, app.in_tab);
                            app = App::new(bundle, images.clone());
                            (app.job, app.tabs, app.in_tab) = (job, tabs, in_tab);
                            if let Some(same) = row.and_then(|row| app.rows.iter().position(|other| *other == row)) {
                                app.select(same);
                                app.focus = focus;
                            }
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
