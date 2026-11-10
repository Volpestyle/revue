use crate::bundle::{Kind, Note};
use crate::{App, Finish, Handle, Input, Row};
use ratatui::crossterm::event::{self, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

impl App {
    pub(crate) fn mouse(&mut self, m: MouseEvent) {
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

    pub(crate) fn key(&mut self, code: KeyCode) -> bool {
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

pub type Command = (&'static str, &'static str, KeyCode);

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

pub(crate) const COMMANDS: [Command; 30] = [
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
