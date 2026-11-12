use crate::bundle::{Kind, Provenance, Status};
use crate::input::{matching, Command, COMMANDS};
use crate::{copied, mrs, share, App, Finish, Handle, Mark, Mode, Row, FINISH};
use crate::video::{clock, Video};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, Padding, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_image::{Resize, StatefulImage};
use syntect::easy::HighlightLines;
use std::path::PathBuf;

/// Long code lines continue on the next row under a blank gutter, so nothing runs off the pane.
pub(crate) fn wrap_code(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
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

pub(crate) fn hunk_lines(path: &str, hunk: &[String], highlight: bool) -> Vec<Line<'static>> {
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

pub(crate) fn syntax() -> &'static (syntect::parsing::SyntaxSet, syntect::highlighting::Theme) {
    static LOADED: std::sync::OnceLock<(syntect::parsing::SyntaxSet, syntect::highlighting::Theme)> = std::sync::OnceLock::new();
    LOADED.get_or_init(|| {
        let theme = two_face::theme::extra().get(two_face::theme::EmbeddedThemeName::TwoDark).clone();
        (two_face::syntax::extra_newlines(), theme)
    })
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
        "mp4" | "mov" | "webm" | "mkv" | "m4v" => ("▶", "recording"),
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

pub(crate) type Rows = Vec<(Line<'static>, Option<usize>)>;

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

pub(crate) fn body(rows: &mut Rows, text: &str, width: usize) {
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

pub(crate) fn detail(app: &App, width: usize) -> Rows {
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
            paragraph(&mut rows, &section.title, width, "", bold, None);
            let mut heading = Vec::new();
            if !section.tier.is_empty() {
                heading.extend([Span::styled(tier_label(&section.tier).to_string(), bold.fg(color)), Span::styled(" · ", dim)]);
            }
            let count = format!("{} file{}", files.len(), if files.len() == 1 { "" } else { "s" });
            let found = if raised { "" } else { " · nothing raised here" };
            heading.push(Span::styled(format!("{count}{found}"), dim));
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
        let commented = app.state.notes.iter().any(|note| !note.posted && note.item.as_ref() == Some(&item.id));
        let after = match (verdict.is_some(), app.quoted(&item.id), commented) {
            (false, _, _) => after,
            (_, true, true) => "  goes out quoted, with your comment",
            (_, true, false) => "  goes out quoted",
            (_, false, true) => "  only your comment goes out",
            (_, false, false) => "  nothing goes out",
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
            let fold = if open { "    ▾ details" } else { "    ▸ details" };
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

/// A pane keeps a one-cell edge all round, mostly blank: a thin rule on the left and, for a pane
/// stacked under another, along the top. Focus shows in the title, not in the edge.
fn pane(title: &str, focused: bool, left: bool, top: bool) -> Block<'static> {
    let label = match focused {
        true => Style::new().fg(accent()).add_modifier(Modifier::BOLD),
        false => Style::new().fg(Color::DarkGray),
    };
    let (side, rule) = (if left { "│" } else { " " }, if top { "─" } else { " " });
    let edge = ratatui::symbols::border::Set {
        top_left: match (left, top) {
            (true, true) => "├",
            (true, false) => "│",
            _ => rule,
        },
        top_right: rule,
        bottom_left: side,
        bottom_right: " ",
        vertical_left: side,
        vertical_right: " ",
        horizontal_top: rule,
        horizontal_bottom: " ",
    };
    Block::bordered().border_set(edge).border_style(Style::new().fg(Color::DarkGray)).title(Span::styled(format!(" {title} "), label))
}

fn draw_image(app: &mut App, frame: &mut Frame, area: Rect, path: PathBuf) {
    let shown = app.item().and_then(|item| item.evidence.iter().position(|e| e.path.as_ref().is_some_and(|p| path.ends_with(p))));
    let (_, kind) = artifact_kind(&path.to_string_lossy());
    let mut clock_shown = String::new();
    let mut failed = None;
    if kind == "recording" {
        let covered = app.layered();
        let video = app.videos.entry(path.clone()).or_insert_with(|| Video::start(&path));
        if let Some((at, file)) = video.frame().filter(|_| !covered) {
            if let Ok(decoded) = image::open(&file) {
                video.shown = Some(at);
                app.images.insert(path.clone(), Some(app.picker.new_resize_protocol(decoded)));
            }
        }
        failed = video.error();
        if let (true, Some(length)) = (video.playing(), video.length()) {
            clock_shown = format!(" · {} / {}", clock(video.position()), clock(length));
        }
    }
    let title = format!("{kind} · evidence {}{clock_shown}", shown.map_or(0, |n| n + 1));
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
    let block = pane(&title, app.focus == 3, true, false);
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
                ("recording", _) => match failed {
                    Some(why) => vec![Line::raw(format!("A recording: {name}")), Line::raw(""), Line::raw("Press o to play it."), Line::raw(why)],
                    None => vec![Line::raw(format!("A recording: {name}")), Line::raw(""), Line::raw("Loading its first frame.")],
                },
                _ => vec![Line::raw(format!("{name} cannot be shown here.")), Line::raw(""), Line::raw("Press o to open it.")],
            };
            frame.render_widget(Paragraph::new(lines), inner);
        }
    }
}

pub(crate) fn fit(keys: &[&str], width: u16) -> String {
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
    for (text, row) in placed.into_iter().filter(|(text, _)| !text.is_empty()) {
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
        (None, true) => {
            let mut line = vec![Span::raw(" ")];
            let working = review.working.trim();
            if !working.is_empty() {
                let found = match review.items.len() {
                    0 => "no findings yet".to_string(),
                    1 => "1 finding so far".to_string(),
                    n => format!("{n} findings so far"),
                };
                line.extend([spinner(), Span::styled(format!(" {working} · {found} · "), Style::new().fg(accent()))]);
            }
            line.extend([count("verdicts", app.state.verdicts.len(), review.items.len()), Span::styled(format!(" · {rest}"), dim)]);
            Line::from(line)
        }
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

pub(crate) fn draw(app: &mut App, frame: &mut Frame) {
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
    if app.focus == 0 && !app.outline {
        app.focus = 1;
    }
    let target = app.open_target().map_or("file".to_string(), |(file, line)| {
        format!("{}:{line}", app.bundle.files[file].path.rsplit('/').next().unwrap_or(""))
    });
    let generate: Vec<(&str, &str, KeyCode, Color)> = match (app.bundle.successor.is_some(), app.bundle.review.sections.is_empty()) {
        (true, _) => vec![("g", "Update guide", KeyCode::Char('g'), accent())],
        (_, true) => vec![("g", "Generate guide", KeyCode::Char('g'), accent())],
        _ => Vec::new(),
    };
    // One row: what the selection takes, then the three keys that find everything else.
    let verdict = match app.item().map(|item| item.kind) {
        Some(Kind::Finding) => "a agree · x dismiss · c comment",
        Some(Kind::Decision) => "a accept · x push back · c comment",
        _ => "c comment",
    };
    let recording = app.recording().is_some();
    let here = match (app.focus, app.rows.get(app.sel).copied()) {
        (0, Some(Row::Thread(_))) => "enter open on the web · c reply".to_string(),
        (0, Some(Row::Heading)) => "enter open on the web".to_string(),
        (0, Some(Row::Note(_))) => "c edit · x delete".to_string(),
        (0, Some(Row::Item(_))) => format!("{verdict} · enter go in"),
        (0, _) => format!("j/k move · enter go in · {verdict}"),
        (2, _) => format!("j/k line · c comment · o open {target} · s layout · d hide"),
        (3, _) if recording => "space play or pause · o open · z zoom".to_string(),
        (3, _) => "o open · z zoom".to_string(),
        _ if app.item().is_some() => format!("{verdict} · v verify · i artifact"),
        _ => verdict.to_string(),
    };
    let shown = if app.hide_diff { " · d show diff" } else { "" };
    let tabs = if app.tabs.terms.is_empty() { "" } else { " · ctrl-q agent" };
    let step = if app.focus == 0 { "" } else { "l next · h back · " };
    // A finding's row is the longest: the list of every card is one key away under / there.
    let contents = if app.item().is_some() { "" } else { " · b contents" };
    let actions = format!("{step}{here}{shown}{contents} · tab pane{tabs} · / commands · ? help");
    let [head, main, foot] = Layout::vertical([Constraint::Length(2), Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
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
    let guide = if app.outline { app.panes.guide.min(main.width.saturating_sub(6)).max(6) } else { 0 };
    let [left, right] = Layout::horizontal([Constraint::Length(guide), Constraint::Min(0)]).areas(main);
    let diff_hidden = !app.diff_shown();
    if diff_hidden && app.focus == 2 {
        app.focus = 1;
    }
    let image = app.evidence_path();
    for (path, video) in app.videos.iter_mut() {
        if image.as_ref() != Some(path) {
            video.pause();
        }
    }
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
        app.handles.extend(app.outline.then(|| (seam(right.x, main), Handle::Guide)));
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

    if app.outline {
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
            .block(pane("guide", app.focus == 0, false, false))
            .highlight_style(Style::new().bg(Color::Rgb(48, 54, 68)).add_modifier(Modifier::BOLD));
        app.list.select(Some(app.sel));
        frame.render_stateful_widget(list, left, &mut app.list);
        let above: u16 = app.row_heights.iter().take(app.list.offset()).sum();
        // The selection is a quiet band; a bar in the pane's left edge says whether the guide has the keys.
        let before: u16 = app.row_heights.iter().take(app.sel).skip(app.list.offset()).sum();
        let marker = if app.focus == 0 { accent() } else { Color::DarkGray };
        for y in (left.y + 1 + before..left.bottom().saturating_sub(1)).take(app.row_heights.get(app.sel).copied().unwrap_or(0) as usize) {
            if let Some(cell) = frame.buffer_mut().cell_mut(Position::new(left.x, y)).filter(|_| app.sel >= app.list.offset()) {
                cell.set_symbol("▌").set_fg(marker);
            }
        }
        scrollbar(frame, left, app.row_heights.iter().sum::<u16>() as usize, above as usize, app.focus == 0);
    }
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
    let keys: Vec<&str> = if picked == 0 { vec![] } else { vec![&count, "v verify", "i artifact"] };
    let style = match picked {
        0 => Style::new().fg(Color::DarkGray),
        _ => Style::new().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
    };
    let hint = Line::styled(fit(&keys, text_area.width), style);
    let block = pane(&app.card_title(), app.focus == 1, app.outline, false).title_bottom(hint);
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
        let files = app.section_files();
        let here = app.diffed.2.get(app.cursor.min(app.diffed.2.len().saturating_sub(1))).map(|mark| mark.file);
        let title = match here.and_then(|file| files.iter().position(|&(listed, _)| listed == file)).filter(|_| files.len() > 1) {
            Some(n) => format!("diff · file {} of {}", n + 1, files.len()),
            None => "diff".to_string(),
        };
        let block = pane(&title, app.focus == 2, true, bottom.y > top.y);
        frame.render_widget(Paragraph::new(lines).block(block), bottom);
        scrollbar(frame, bottom, app.diffed.0.len(), app.scroll as usize, app.focus == 2);
        // Where the rule over a stacked diff meets the divider of the picture above it.
        if let Some(cell) = frame.buffer_mut().cell_mut(Position::new(picture.x, bottom.y)).filter(|_| picture.width > 0 && bottom.y > top.y) {
            cell.set_symbol("┴");
        }
    }

    app.action_buttons.extend(footer(frame, foot, &generate, &actions, ""));

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
        let mut buttons = vec![("enter", "Save", KeyCode::Enter), ("esc", "Cancel", KeyCode::Esc)];
        match input.quote {
            Some(true) => buttons.push(("tab", "Quote the finding: yes", KeyCode::Tab)),
            Some(false) => buttons.push(("tab", "Quote the finding: no", KeyCode::Tab)),
            None => {}
        }
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
            (Mode::Guide, _) => "Review this merge request and write its guide".into(),
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
revui walks you through a merge request. Paste its link and an agent reviews it: a summary first, the change split into parts in reading order, then one finding at a time, each attached to the code it is about. You read, check what matters, and decide.
It never hides code. Anything the agent left out is shown as \"no part\".

# One card at a time
l goes to the next card and h back: the summary, then each part with the findings under it, then what was already said on the merge request and your own comments.
Findings arrive while the agent is still reviewing, so you can start reading at once. The top line says what it is on. b shows the list of every card.
B blocker · F should fix · S suggestion · Q question or a decision the change made · G good call

# A review, start to finish
1. Paste a merge request or pull request link. The agent starts in its own tab: ctrl-q shows it, and it may ask you to allow a command.
2. Read the summary, then step through with l.
3. On each finding, a says you agree and x dismisses it. On a decision, a accepts the choice and x pushes back.
4. Not convinced? t asks the agent that did the review, v has it reproduce the finding, i asks for a diagram or screenshot. The first time a check needs the product running, the agent asks where: a local stack, a preview deploy, staging.
5. c adds your own comment, on a finding or on a line of the diff (tab to the diff, move to the line, c).
6. p publishes your review. You see exactly what would be posted first, and confirm.

# Getting around
tab moves between panes, o opens the file or the artifact, ctrl-q switches to the agent and back.
/ lists every command. , opens settings. q or esc goes back to your reviews. ctrl-c quits.";

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

pub(crate) fn editor_area(screen: Rect) -> Rect {
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
