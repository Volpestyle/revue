use super::*;
use crate::draw::{body, fit, hint_areas, syntax, Rows};
use crate::input::matching;
use ratatui::crossterm::event::{MouseButton, MouseEvent};
use ratatui::text::Span;
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
