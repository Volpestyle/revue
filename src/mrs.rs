use crate::bundle::Bundle;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListState};
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::picker::Picker;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{fs, io};

#[derive(Deserialize, Default)]
struct Config {
    generate: Option<String>,
    generate_modal: Option<bool>,
    claude: Option<String>,
    drive: Option<String>,
    accent: Option<String>,
    open: Option<String>,
    open_more: Option<String>,
    #[serde(default)]
    open_modal: bool,
    agents: Option<Vec<Agent>>,
}

#[derive(Deserialize, Clone)]
pub struct Agent {
    pub name: String,
    pub run: String,
    pub send: Option<String>,
    pub resume: Option<String>,
    #[serde(default)]
    pub foreground: bool,
    #[serde(default)]
    pub modal: bool,
}

pub fn agents() -> Vec<Agent> {
    let agent = Agent {
        name: "agent".into(),
        run: "$REVUI_CLAUDE --session-id \"$REVUI_SESSION_ID\"".into(),
        resume: Some("$REVUI_CLAUDE --resume \"$REVUI_SESSION_ID\"".into()),
        send: None,
        foreground: false,
        modal: true,
    };
    let mut agents = config().agents.filter(|agents| !agents.is_empty()).unwrap_or_else(|| vec![agent]);
    for agent in &mut agents {
        agent.modal |= std::mem::take(&mut agent.foreground);
    }
    agents
}

pub fn install_skill() -> Result<PathBuf, String> {
    let set = std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|dir| !dir.is_empty());
    let skills = set.map_or_else(|| home().join(".claude"), PathBuf::from).join("skills");
    for (name, text) in [("revui", crate::SKILL), ("revui-drive", crate::DRIVE), ("revui-guide", crate::GUIDE)] {
        let dir = skills.join(name);
        fs::create_dir_all(&dir).and_then(|_| fs::write(dir.join("SKILL.md"), text)).map_err(|e| e.to_string())?;
    }
    Ok(skills)
}

pub fn drive_report(bundle: Option<&Path>) -> String {
    let on_path = |program: &str| Command::new("sh").args(["-c", "command -v \"$1\" >/dev/null", "sh", program]).status().is_ok_and(|s| s.success());
    let app = |name: &str| ["/Applications", &home().join("Applications").to_string_lossy()].iter().any(|dir| Path::new(dir).join(name).exists());
    let tools = [
        (on_path("herdr-browser"), "herdr-browser", "Herdr Browser's in-app browser, when this session runs inside it ($HERDR_BROWSER is set)", "herdr-browser --skill"),
        (app("Orca.app"), "Orca", "its embedded browser, in a pane of the app with its own profile, when this session runs inside Orca", "orca skills get orca-cli (the CLI is Orca.app/Contents/Resources/bin/orca when it is not on PATH)"),
        (on_path("agent-browser"), "agent-browser", "headless browser", "agent-browser --help"),
        (on_path("playwright-cli"), "playwright-cli", "headless browser", "playwright-cli --help"),
        (on_path("playwright"), "playwright", "headless browser", "playwright --help"),
        (app("Google Chrome.app") || app("Chromium.app") || on_path("google-chrome") || on_path("chromium"), "Chrome", "headless with --headless=new and its own --user-data-dir; `revui shot` for a page with no sign-in", "revui help"),
        (on_path("agent-desktop"), "agent-desktop", "native desktop apps, without moving the pointer or taking focus", "agent-desktop skills get desktop"),
    ];
    let preference = drive();
    let mut out = format!("Preference: {}\n\nOn this machine:\n", if preference.is_empty() { "none set" } else { &preference });
    for (found, name, what, manual) in &tools {
        if *found {
            out += &format!("  {name:<15} {what}. Manual: {manual}\n");
        }
    }
    let missing: Vec<&str> = tools.iter().filter(|tool| !tool.0).map(|tool| tool.1).collect();
    out += &format!("  not found: {}\n", if missing.is_empty() { "nothing".to_string() } else { missing.join(", ") });
    let repo = bundle.and_then(|dir| Bundle::load(dir).ok()).and_then(|bundle| bundle.review.sources.first().map(|source| source.repo.clone()));
    if let Some(repo) = repo.filter(|repo| !repo.is_empty()) {
        let notes = data_dir().join("notes").join(format!("{}.md", repo.replace('/', "__")));
        let text = fs::read_to_string(&notes).unwrap_or_default();
        if !text.contains("## Evidence") {
            out += &format!("\nWhere evidence comes from for {repo}: not asked yet. Before the first run that needs a running product, ask the person where the change runs and where the baseline is read from, then record it under `## Evidence` in the notes file (`revui skill drive`, step 2).\n");
        }
        out += &format!("\nNotes for {repo} ({}):\n{}\n", notes.display(), if text.trim().is_empty() { "none yet: append what you learn" } else { text.trim() });
    }
    out
}

fn config() -> Config {
    fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// The generator the config names, and whether it runs in a tab. Without one, the review's own agent writes the guide.
pub fn generate_settings() -> (String, bool) {
    let config = config();
    match config.generate {
        Some(command) => (command, config.generate_modal.unwrap_or(false)),
        None => (format!("{}   # this review's agent reads $REVUI_URL and writes the guide", agents().remove(0).run), true),
    }
}

/// Whether the config names its own generator. Without one, the review's agent writes the guide.
pub fn generate_configured() -> bool {
    config().generate.is_some()
}

pub fn generate_command() -> String {
    generate_settings().0
}

pub fn apply_accent() {
    crate::set_accent(&config().accent.unwrap_or_default());
}

pub fn drive() -> String {
    config().drive.unwrap_or_default().trim().to_string()
}

pub fn claude() -> String {
    config().claude.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| "claude".into())
}

pub fn open_command() -> String {
    open_settings().0
}

pub fn open_settings() -> (String, bool, Option<String>) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let config = config();
    (config.open.unwrap_or_else(|| format!("{opener} \"$REVUI_FILE_URL\"")), config.open_modal, config.open_more)
}

pub fn doctor(images: &str) -> (bool, String) {
    let config = config();
    let on_path = |program: &str| {
        let found = Command::new("sh").args(["-c", "command -v \"$1\" >/dev/null", "sh", program]).status();
        found.is_ok_and(|s| s.success())
    };
    let first = |command: &str| command.split_whitespace().next().unwrap_or("").to_string();
    let set = std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|dir| !dir.is_empty());
    let skills = set.map_or_else(|| home().join(".claude"), PathBuf::from).join("skills");
    let writes_guides = |dir: PathBuf| fs::read_to_string(dir.join("SKILL.md")).is_ok_and(|text| text.contains("revui hunks"));
    let writers: Vec<String> = fs::read_dir(&skills).into_iter().flatten().flatten().filter(|entry| writes_guides(entry.path())).map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|name| !["revui", "revui-drive"].contains(&name.as_str())).collect();
    let (has_glab, has_gh) = (on_path("glab"), on_path("gh"));
    let (generate, open) = (generate_command().replace("$REVUI_CLAUDE", &claude()), open_command());
    let mut agent = agents().remove(0);
    agent.run = agent.run.replace("$REVUI_CLAUDE", &claude());
    let settings = config_path().display().to_string();
    let default = |set: bool| if set { "" } else { " (the default)" };
    let mut checks = vec![(has_glab || has_gh, "a code host CLI is installed".to_string(), "install glab (GitLab) or gh (GitHub)".to_string())];
    if has_glab {
        checks.push((glab(&["auth", "status"]).is_ok(), "glab is signed in, for GitLab merge requests".into(), "run: glab auth login".into()));
    }
    if has_gh {
        checks.push((gh(&["auth", "status"]).is_ok(), "gh is signed in, for GitHub pull requests".into(), "run: gh auth login".into()));
    }
    if config.generate.is_some() {
        checks.push((
            !writers.is_empty(),
            format!("a skill that writes guides is installed: {}", writers.join(", ")),
            format!("no skill that writes guides in {}: run `revui skill install`", skills.display()),
        ));
        checks.push((
            on_path(&first(&generate)),
            format!("g runs{}: {generate}", if generate_settings().1 { " in a tab here" } else { "" }),
            format!("g would run `{generate}`, but {} is not on PATH: fix \"generate\" in {settings}", first(&generate)),
        ));
    }
    checks.extend([
        (
            on_path(&first(&open)),
            format!("enter runs{}: {open}", default(config.open.is_some())),
            format!("enter would run `{open}`, but {} is not on PATH: set \"open\" in {settings}", first(&open)),
        ),
        (
            on_path(&first(&agent.run)),
            format!("pasting a link starts this agent, and t and v ask it{}: {}", default(config.agents.is_some()), agent.run),
            format!("the agent would run `{}`, but {} is not on PATH: set \"agents\" in {settings}", agent.run, first(&agent.run)),
        ),
    ]);
    let mut out = String::new();
    for (ok, good, fix) in &checks {
        out += &if *ok { format!("ok   {good}\n") } else { format!("fix  {fix}\n") };
    }
    out += &format!("     images: {images}\n");
    let driving = drive();
    out += &format!("     agents drive a browser or app with: {}\n", if driving.is_empty() { "not set (\"drive\" in the config)" } else { &driving });
    (checks.iter().all(|c| c.0), out)
}

/// The merge request or pull request a link names, as its host has it now.
fn lookup(url: &str) -> Result<Mr, String> {
    let wrong = "expected a GitLab merge request URL or a GitHub pull request URL";
    let number = |rest: &str| rest.chars().take_while(char::is_ascii_digit).collect::<String>();
    if let Some((project, rest)) = url.split_once("/pull/") {
        let repo = project.splitn(4, '/').nth(3).ok_or(wrong)?;
        let found = gh(&["pr", "view", &number(rest), "-R", repo, "--json", PR_FIELDS])?;
        let pr: serde_json::Value = serde_json::from_str(&found).map_err(|e| format!("gh pr view: {e}"))?;
        return from_github(&pr, repo).ok_or("gh pr view: an unexpected reply".to_string());
    }
    let (project, rest) = url.split_once("/-/merge_requests/").ok_or(wrong)?;
    let repo = project.splitn(4, '/').nth(3).ok_or(wrong)?;
    serde_json::from_str(&glab(&["mr", "view", &number(rest), "-R", repo, "-F", "json"])?).map_err(|e| format!("glab mr view: {e}"))
}

pub fn fetch(url: &str) -> Result<PathBuf, String> {
    lookup(url)?.materialize()
}

const PR_FIELDS: &str = "number,title,url,headRefOid,headRefName,baseRefName,author";

fn from_github(pr: &serde_json::Value, repo: &str) -> Option<Mr> {
    let text = |key: &str| pr[key].as_str().unwrap_or("").to_string();
    let author = &pr["author"];
    let number = pr["number"].as_u64()?;
    Some(Mr {
        iid: number,
        title: text("title"),
        web_url: text("url"),
        sha: pr["headRefOid"].as_str().map(String::from),
        source_branch: text("headRefName"),
        target_branch: text("baseRefName"),
        author: User { username: author["login"].as_str().or(author["name"].as_str()).unwrap_or("").to_string() },
        references: References { full: format!("{repo}#{number}") },
    })
}

pub fn file_url(url: &str, sha: &str, file: &str, line: u32) -> Option<String> {
    match (url.split_once("/pull/"), url.split_once("/-/")) {
        (Some((project, _)), _) => Some(format!("{project}/blob/{sha}/{file}#L{line}")),
        (_, Some((project, _))) => Some(format!("{project}/-/blob/{sha}/{file}#L{line}")),
        _ => None,
    }
}

fn number(reference: &str) -> &str {
    reference.trim_start_matches(['!', '#'])
}

fn on_github(reference: &str) -> bool {
    reference.starts_with('#')
}

#[derive(Deserialize, Clone)]
struct User {
    username: String,
}

#[derive(Deserialize, Clone)]
struct References {
    full: String,
}

#[derive(Deserialize, Clone)]
pub struct Mr {
    iid: u64,
    title: String,
    web_url: String,
    sha: Option<String>,
    source_branch: String,
    target_branch: String,
    author: User,
    references: References,
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// Where bundles, notes and pane sizes live. `REVUI_HOME` moves this and the config
/// into one folder, for a second setup or a sandbox that must not touch the real one.
pub fn data_dir() -> PathBuf {
    match std::env::var("REVUI_HOME").ok().filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("data"),
        None => home().join(".local/share/revui"),
    }
}

fn config_path() -> PathBuf {
    match std::env::var("REVUI_HOME").ok().filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("config.json"),
        None => home().join(".config/revui/config.json"),
    }
}

fn cli(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().lines().last().unwrap_or("it failed").into())
    }
}

pub fn glab(args: &[&str]) -> Result<String, String> {
    cli("glab", args)
}

pub fn gh(args: &[&str]) -> Result<String, String> {
    cli("gh", args)
}

#[derive(Clone, serde::Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub anchor: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub old: bool,
    pub resolved: bool,
    pub notes: Vec<(String, String, String)>,
}

fn author(user: &serde_json::Value, body: &str) -> String {
    let username = user["username"].as_str().unwrap_or("");
    let bot = username.starts_with("group_") || username.starts_with("project_") || user["bot"] == true;
    match (bot, user["name"].as_str()) {
        (true, Some(name)) if name.chars().any(char::is_alphanumeric) => format!("{name} (bot)"),
        (true, _) if body.contains("linear-linkback") => "@Linear".to_string(),
        (true, _) => "@bot".to_string(),
        _ => format!("@{username}"),
    }
}

pub fn parse_threads(json: &str) -> Vec<Thread> {
    let discussions: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    let mut threads = Vec::new();
    for discussion in discussions {
        let empty = Vec::new();
        let notes: Vec<&serde_json::Value> = discussion["notes"].as_array().unwrap_or(&empty).iter().filter(|n| n["system"] != true).collect();
        let Some(first) = notes.first() else { continue };
        let position = &first["position"];
        let (new_line, old_line) = (position["new_line"].as_u64(), position["old_line"].as_u64());
        let text = |value: &serde_json::Value| value.as_str().unwrap_or("").to_string();
        threads.push(Thread {
            id: text(&discussion["id"]),
            anchor: format!("#note_{}", first["id"].as_u64().unwrap_or(0)),
            file: position["new_path"].as_str().or(position["old_path"].as_str()).map(String::from),
            line: new_line.or(old_line).map(|n| n as u32),
            old: new_line.is_none() && old_line.is_some(),
            resolved: first["resolvable"] == true && notes.iter().all(|n| n["resolvable"] != true || n["resolved"] == true),
            notes: notes.iter().map(|n| (author(&n["author"], n["body"].as_str().unwrap_or("")), text(&n["body"]), text(&n["created_at"]))).collect(),
        });
    }
    threads
}

fn request(repo: &str, reference: &str) -> String {
    format!("projects/{}/merge_requests/{}", crate::url_encode(repo), number(reference))
}

const THREADS: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) {
  comments(first: 100) { nodes { databaseId author { login __typename } body createdAt } }
  reviews(first: 100) { nodes { databaseId author { login __typename } body createdAt } }
  reviewThreads(first: 100) { nodes { isResolved path line originalLine diffSide
    comments(first: 100) { nodes { databaseId author { login __typename } body createdAt } } } } } } }";

/// GitHub keeps three kinds of conversation on a pull request: plain comments,
/// review summaries, and threads on lines. Only the last can be replied to in place.
pub fn parse_github_threads(json: &str) -> Vec<Thread> {
    let reply: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let pr = &reply["data"]["repository"]["pullRequest"];
    let empty = Vec::new();
    let nodes = |value: &serde_json::Value| value["nodes"].as_array().unwrap_or(&empty).clone();
    let text = |value: &serde_json::Value| value.as_str().unwrap_or("").to_string();
    let note = |comment: &serde_json::Value| {
        let login = comment["author"]["login"].as_str().unwrap_or("ghost");
        let who = if comment["author"]["__typename"] == "Bot" { format!("{login} (bot)") } else { format!("@{login}") };
        (who, text(&comment["body"]), text(&comment["createdAt"]))
    };
    let mut threads = Vec::new();
    for (kind, anchor) in [("comments", "issuecomment-"), ("reviews", "pullrequestreview-")] {
        for comment in nodes(&pr[kind]).iter().filter(|comment| !text(&comment["body"]).trim().is_empty()) {
            let id = comment["databaseId"].as_u64().unwrap_or(0);
            threads.push(Thread { id: String::new(), anchor: format!("#{anchor}{id}"), file: None, line: None, old: false, resolved: false, notes: vec![note(comment)] });
        }
    }
    for thread in nodes(&pr["reviewThreads"]) {
        let comments = nodes(&thread["comments"]);
        let Some(first) = comments.first().and_then(|comment| comment["databaseId"].as_u64()) else { continue };
        threads.push(Thread {
            id: first.to_string(),
            anchor: format!("#discussion_r{first}"),
            file: thread["path"].as_str().map(String::from),
            line: thread["line"].as_u64().or(thread["originalLine"].as_u64()).map(|line| line as u32),
            old: thread["diffSide"] == "LEFT",
            resolved: thread["isResolved"] == true,
            notes: comments.iter().map(note).collect(),
        });
    }
    threads.sort_by(|a, b| a.notes[0].2.cmp(&b.notes[0].2));
    threads
}

pub fn threads(repo: &str, reference: &str) -> Vec<Thread> {
    if on_github(reference) {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        let fields = [format!("query={THREADS}"), format!("owner={owner}"), format!("name={name}"), format!("number={}", number(reference))];
        let asked = gh(&["api", "graphql", "-f", &fields[0], "-f", &fields[1], "-f", &fields[2], "-F", &fields[3]]);
        return asked.map(|json| parse_github_threads(&json)).unwrap_or_default();
    }
    let listed = glab(&["api", &format!("{}/discussions?per_page=100", request(repo, reference))]);
    listed.map(|json| parse_threads(&json)).unwrap_or_default()
}

pub fn dates(repo: &str, reference: &str) -> Option<(String, String)> {
    if on_github(reference) {
        let found = gh(&["pr", "view", number(reference), "-R", repo, "--json", "createdAt,updatedAt"]).ok()?;
        let pr: serde_json::Value = serde_json::from_str(&found).ok()?;
        return Some((pr["createdAt"].as_str()?.to_string(), pr["updatedAt"].as_str()?.to_string()));
    }
    let mr: serde_json::Value = serde_json::from_str(&glab(&["api", &request(repo, reference)]).ok()?).ok()?;
    Some((mr["created_at"].as_str()?.to_string(), mr["updated_at"].as_str()?.to_string()))
}

/// The commit the diff starts from, so an editor can show the file before and after.
pub fn base_sha(repo: &str, reference: &str, head: &str) -> Option<String> {
    if on_github(reference) {
        let found = gh(&["pr", "view", number(reference), "-R", repo, "--json", "baseRefOid"]).ok()?;
        let pr: serde_json::Value = serde_json::from_str(&found).ok()?;
        let compared = gh(&["api", &format!("repos/{repo}/compare/{}...{head}", pr["baseRefOid"].as_str()?)]).ok()?;
        let compared: serde_json::Value = serde_json::from_str(&compared).ok()?;
        return compared["merge_base_commit"]["sha"].as_str().map(String::from);
    }
    let versions = glab(&["api", &format!("{}/versions", request(repo, reference))]).ok();
    let versions: Vec<serde_json::Value> = versions.and_then(|out| serde_json::from_str(&out).ok()).unwrap_or_default();
    let exact = versions.iter().find(|v| v["head_commit_sha"] == head).and_then(|v| v["base_commit_sha"].as_str().map(String::from));
    exact.or_else(|| {
        let mr: serde_json::Value = serde_json::from_str(&glab(&["api", &request(repo, reference)]).ok()?).ok()?;
        mr["diff_refs"]["base_sha"].as_str().map(String::from)
    })
}

pub fn raw_file(repo: &str, github: bool, sha: &str, file: &str) -> Option<String> {
    match github {
        true => gh(&["api", "-H", "Accept: application/vnd.github.raw+json", &format!("repos/{repo}/contents/{file}?ref={sha}")]).ok(),
        false => {
            let endpoint = format!("projects/{}/repository/files/{}/raw?ref={sha}", crate::url_encode(repo), crate::url_encode(file));
            glab(&["api", &endpoint]).ok()
        }
    }
}

pub fn post_note(repo: &str, reference: &str, body: &str) -> Result<(), String> {
    let field = format!("body={body}");
    if on_github(reference) {
        return gh(&["api", &format!("repos/{repo}/issues/{}/comments", number(reference)), "-f", &field]).map(|_| ());
    }
    glab(&["api", "--method", "POST", &format!("{}/notes", request(repo, reference)), "-f", &field]).map(|_| ())
}

pub fn post_reply(repo: &str, reference: &str, thread: &str, body: &str) -> Result<(), String> {
    let field = format!("body={body}");
    if on_github(reference) {
        return match thread.is_empty() {
            true => post_note(repo, reference, body),
            false => gh(&["api", &format!("repos/{repo}/pulls/{}/comments/{thread}/replies", number(reference)), "-f", &field]).map(|_| ()),
        };
    }
    glab(&["api", "--method", "POST", &format!("{}/discussions/{thread}/notes", request(repo, reference)), "-f", &field]).map(|_| ())
}

pub fn post_on_line(repo: &str, reference: &str, head: &str, file: &str, start: Option<u32>, line: u32, old: bool, body: &str) -> Result<(), String> {
    if on_github(reference) {
        let found = gh(&["pr", "view", number(reference), "-R", repo, "--json", "headRefOid"])?;
        if !found.contains(head) {
            return Err("the pull request has moved on since this review".into());
        }
        let fields = [format!("body={body}"), format!("commit_id={head}"), format!("path={file}"), format!("line={line}"), format!("side={}", if old { "LEFT" } else { "RIGHT" })];
        let endpoint = format!("repos/{repo}/pulls/{}/comments", number(reference));
        let mut args = vec!["api".to_string(), endpoint, "-f".into(), fields[0].clone(), "-f".into(), fields[1].clone(), "-f".into(), fields[2].clone(), "-F".into(), fields[3].clone(), "-f".into(), fields[4].clone()];
        if let Some(start) = start {
            args.extend(["-F".to_string(), format!("start_line={start}"), "-f".into(), fields[4].replace("side=", "start_side=")]);
        }
        return gh(&args.iter().map(String::as_str).collect::<Vec<_>>()).map(|_| ());
    }
    // ponytail: GitLab gets the comment on the last line with the range in its text; use position[line_range] if that reads poorly
    let ranged = start.map(|start| format!("Lines {start}-{line}:\n\n{body}"));
    let body = ranged.as_deref().unwrap_or(body);
    let mr: serde_json::Value = serde_json::from_str(&glab(&["api", &request(repo, reference)])?).map_err(|e| e.to_string())?;
    let refs = &mr["diff_refs"];
    if refs["head_sha"] != head {
        return Err("the merge request has moved on since this review".into());
    }
    let sha = |key: &str| refs[key].as_str().unwrap_or("").to_string();
    let fields = [
        format!("body={body}"),
        "position[position_type]=text".to_string(),
        format!("position[base_sha]={}", sha("base_sha")),
        format!("position[start_sha]={}", sha("start_sha")),
        format!("position[head_sha]={}", sha("head_sha")),
        format!("position[new_path]={file}"),
        format!("position[old_path]={file}"),
        format!("position[{}]={line}", if old { "old_line" } else { "new_line" }),
    ];
    let mut args = vec!["api".to_string(), "--method".into(), "POST".into(), format!("{}/discussions", request(repo, reference))];
    for field in fields {
        args.extend(["-f".to_string(), field]);
    }
    glab(&args.iter().map(String::as_str).collect::<Vec<_>>()).map(|_| ())
}

pub fn approve(repo: &str, reference: &str) -> Result<(), String> {
    if on_github(reference) {
        return gh(&["pr", "review", number(reference), "-R", repo, "--approve"]).map(|_| ());
    }
    glab(&["mr", "approve", number(reference), "-R", repo]).map(|_| ())
}

pub fn has_guide(dir: &Path) -> bool {
    let review = fs::read_to_string(dir.join("review.json")).ok();
    let review = review.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    review.is_some_and(|v| v["sections"].as_array().is_some_and(|a| !a.is_empty()))
}

impl Mr {
    fn earlier_guide(&self) -> Option<PathBuf> {
        let current = self.dir();
        let prefix = format!("{}-{}-", self.repo().replace('/', "__"), self.iid);
        let siblings = fs::read_dir(current.parent()?).ok()?.filter_map(|entry| Some(entry.ok()?.path()));
        let guided = siblings.filter(|dir| {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            name.starts_with(&prefix) && *dir != current && has_guide(dir)
        });
        guided.max_by_key(|dir| fs::metadata(dir.join("review.json")).and_then(|m| m.modified()).ok())
    }

    fn repo(&self) -> &str {
        self.references.full.rsplit_once(['!', '#']).map_or("", |(repo, _)| repo)
    }

    fn github(&self) -> bool {
        self.references.full.contains('#')
    }

    fn reference(&self) -> String {
        format!("{}{}", if self.github() { '#' } else { '!' }, self.iid)
    }

    fn sha(&self) -> &str {
        self.sha.as_deref().unwrap_or("")
    }

    fn dir(&self) -> PathBuf {
        let sha = &self.sha()[..self.sha().len().min(8)];
        data_dir().join(format!("{}-{}-{sha}", self.repo().replace('/', "__"), self.iid))
    }

    pub fn write_bundle(&self, dir: &Path, patch: &str) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join("mr.patch"), patch)?;
        let review = serde_json::json!({
            "title": self.title,
            "sources": [{
                "id": "mr",
                "url": self.web_url,
                "reference": self.reference(),
                "repo": self.repo(),
                "author": format!("@{}", self.author.username),
                "branch": self.source_branch,
                "target": self.target_branch,
                "head_sha": self.sha(),
                "patch": "mr.patch",
            }],
            "sections": [],
        });
        fs::write(dir.join("review.json"), serde_json::to_string_pretty(&review)?)
    }

    fn materialize(&self) -> Result<PathBuf, String> {
        let dir = self.dir();
        if !dir.join("review.json").exists() {
            let patch = match self.github() {
                true => gh(&["pr", "diff", &self.iid.to_string(), "-R", self.repo()])?,
                false => glab(&["mr", "diff", &self.iid.to_string(), "-R", self.repo(), "--raw"])?,
            };
            self.write_bundle(&dir, &patch).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        if let (false, false, Some(earlier)) = (has_guide(&dir), dir.join("previous").exists(), self.earlier_guide()) {
            let _ = fs::write(dir.join("previous"), earlier.to_string_lossy().as_bytes());
        }
        Ok(dir)
    }
}

/// One merge request opened here before, read from its bundle on disk.
#[derive(Clone)]
struct Past {
    dir: PathBuf,
    url: String,
    repo: String,
    reference: String,
    title: String,
    author: String,
    guide: bool,
    items: usize,
    working: String,
    marked: usize,
    drafts: usize,
    done: Option<String>,
    touched: std::time::SystemTime,
}

impl Past {
    fn name(&self) -> String {
        format!("{}{}", self.repo.rsplit('/').next().unwrap_or(""), self.reference)
    }

    fn matches(&self, filter: &str) -> bool {
        let haystack = format!("{} {} {} {}", self.repo, self.reference, self.title, self.author).to_lowercase();
        filter.to_lowercase().split_whitespace().all(|term| haystack.contains(term))
    }
}

fn past(dir: &Path) -> Option<Past> {
    let read = |name: &str| fs::read_to_string(dir.join(name)).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let review = read("review.json")?;
    let state = read("state.json").unwrap_or_default();
    let source = &review["sources"][0];
    let text = |value: &serde_json::Value| value.as_str().unwrap_or("").trim().to_string();
    let count = |value: &serde_json::Value| value.as_array().map_or(0, Vec::len);
    let touched = ["review.json", "state.json", "session.json"].iter().filter_map(|name| fs::metadata(dir.join(name)).and_then(|m| m.modified()).ok()).max()?;
    Some(Past {
        dir: dir.into(),
        url: text(&source["url"]),
        repo: text(&source["repo"]),
        reference: text(&source["reference"]),
        title: text(&review["title"]),
        author: text(&source["author"]),
        guide: count(&review["sections"]) > 0,
        items: count(&review["items"]),
        working: text(&review["working"]),
        marked: state["verdicts"].as_object().map_or(0, |verdicts| verdicts.len()),
        drafts: state["notes"].as_array().into_iter().flatten().filter(|note| note["posted"] != true).count(),
        done: state["finished"]["how"].as_str().map(String::from),
        touched,
    })
}

/// Every merge request opened here, the latest first. One with several heads shows once: its newest guide, or its newest diff.
fn history(root: &Path) -> Vec<Past> {
    let mut all: Vec<Past> = fs::read_dir(root).into_iter().flatten().flatten().filter_map(|entry| past(&entry.path())).collect();
    all.sort_by(|a, b| (b.guide, b.touched).cmp(&(a.guide, a.touched)));
    let mut kept: Vec<Past> = Vec::new();
    for found in all {
        match kept.iter_mut().find(|other| !found.url.is_empty() && other.url == found.url) {
            Some(other) => other.touched = other.touched.max(found.touched),
            None => kept.push(found),
        }
    }
    kept.sort_by(|a, b| b.touched.cmp(&a.touched));
    kept
}

fn since(then: std::time::SystemTime) -> String {
    match then.elapsed().map_or(0, |passed| passed.as_secs()) {
        seconds if seconds < 90 => "just now".to_string(),
        seconds if seconds < 3_600 => format!("{}m ago", seconds / 60),
        seconds if seconds < 86_400 => format!("{}h ago", seconds / 3_600),
        seconds => format!("{}d ago", seconds / 86_400),
    }
}

/// The merge request or pull request link in a paste, if it holds one.
pub fn link(text: &str) -> Option<String> {
    let around = ['<', '>', '"', '\'', '(', ')', '[', ']', ',', '.'];
    let words = text.split_whitespace().map(|word| word.trim_matches(around));
    words.filter(|word| word.starts_with("http")).find(|word| word.contains("/pull/") || word.contains("/-/merge_requests/")).map(String::from)
}

/// The bundle to read for a link: the guide for its current head, or an earlier head's guide with the current one waiting behind it.
pub fn open_url(url: &str) -> Result<Bundle, String> {
    let mr = lookup(url)?;
    let current = mr.materialize()?;
    let guided = (!has_guide(&current)).then(|| mr.earlier_guide()).flatten();
    let mut bundle = Bundle::load(guided.as_ref().unwrap_or(&current))?;
    bundle.successor = guided.is_some().then_some(current);
    Ok(bundle)
}

#[derive(Default)]
struct Home {
    tabs: crate::Tabs,
    configuring: bool,
    past: Vec<Past>,
    scanned: Option<std::time::Instant>,
    field: String,
    cursor: usize,
    typing: bool,
    sel: usize,
    message: String,
    busy: Option<String>,
    help: bool,
    palette: Option<(String, usize)>,
    palette_cursor: usize,
    selecting: Option<(Position, Position)>,
    copy: bool,
    clipboard: Option<String>,
    buttons: Vec<(Rect, KeyCode)>,
    actions: Vec<(Rect, KeyCode)>,
    field_area: Rect,
    body: Rect,
    state: ListState,
}

impl Home {
    fn scan(&mut self) {
        self.past = history(&data_dir());
        self.scanned = Some(std::time::Instant::now());
    }

    /// What the field holds narrows the list, unless it is a link: that starts a review.
    fn visible(&self) -> Vec<&Past> {
        let filter = if link(&self.field).is_some() { "" } else { self.field.as_str() };
        self.past.iter().filter(|past| past.matches(filter)).collect()
    }

    fn picked(&self) -> Option<Past> {
        self.visible().get(self.sel).map(|past| (*past).clone())
    }

    /// An agent started here is still on it: its tab is named for the merge request.
    fn live(&self, past: &Past) -> bool {
        let name = format!(" {}", past.name());
        self.tabs.terms.iter().any(|tab| tab.name.ends_with(&name))
    }

    fn draw(&mut self, frame: &mut Frame) {
        crate::clear_popup();
        self.draw_home(frame);
        if let Some(text) = crate::popup_selection(frame, self.selecting) {
            if self.copy {
                self.copy = false;
                self.message = crate::copied(text.chars().count());
                self.clipboard = Some(text);
            }
        }
    }

    fn draw_home(&mut self, frame: &mut Frame) {
        let dim = Style::new().fg(Color::DarkGray);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let [head, field, body, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
        let shown: Vec<Past> = self.visible().into_iter().cloned().collect();
        let sel = self.sel.min(shown.len().saturating_sub(1));
        (self.sel, self.body) = (sel, body);
        self.field_area = Rect { y: field.y + 1, height: 1.min(field.height), ..field };

        let tabs = if self.tabs.is_empty() { "" } else { " · ctrl-q agents" };
        let keys = match (self.typing, self.past.is_empty()) {
            (true, true) => format!("enter start the review · , settings · esc quit{tabs}"),
            (true, false) => format!("enter start or open · ↓ your reviews · esc clear{tabs}"),
            _ => format!("j/k move · tab type a link or filter · r rescan · , settings{tabs} · / commands · ? help · q quit"),
        };
        let wanted: Vec<(&str, &str, KeyCode, Color)> = match (self.typing, shown.get(sel)) {
            (false, Some(_)) => vec![("enter", "Open", KeyCode::Enter, Color::Green)],
            _ => Vec::new(),
        };
        self.actions = crate::footer(frame, foot, &wanted, &keys, "");

        let total = self.past.len();
        let mut title = vec![Span::styled(" revui", bold.fg(crate::accent()))];
        title.push(Span::styled(
            match (total, shown.len() == total) {
                (0, _) => String::new(),
                (1, true) => "  1 review".to_string(),
                (_, true) => format!("  {total} reviews"),
                _ => format!("  {} of {total} reviews", shown.len()),
            },
            dim,
        ));
        match (&self.busy, self.message.is_empty()) {
            (Some(busy), _) => title.extend([Span::raw("  "), crate::spinner(), Span::raw(format!(" {busy}…"))]),
            (None, false) => title.push(Span::styled(format!("  {}", self.message), Style::new().fg(Color::Yellow))),
            _ => {}
        }
        frame.render_widget(Line::from(title), head);

        let ask = if total == 0 { "Paste a merge request or pull request link to start a review" } else { "Paste a merge request or pull request link to start a review, or type to filter" };
        let mark = Span::styled(" ▌ ", Style::new().fg(if self.typing { crate::accent() } else { Color::DarkGray }));
        let text = if self.field.is_empty() { Span::styled(ask, dim) } else { Span::raw(self.field.clone()) };
        frame.render_widget(Line::from(vec![mark, text]), self.field_area);
        let modal = self.help || self.palette.is_some();
        if self.typing && !modal && self.busy.is_none() {
            let column = self.cursor.min(self.field.chars().count()) as u16;
            frame.set_cursor_position(Position::new(self.field_area.x + 3 + column, self.field_area.y));
        }

        let rows: Vec<Line> = shown
            .iter()
            .map(|past| {
                let live = self.live(past);
                let tag = match (live, !past.working.is_empty(), &past.done, past.guide) {
                    (true, _, _, _) => {
                        let mut spinner = crate::spinner();
                        spinner.content = format!(" {} reviewing ", spinner.content).into();
                        spinner
                    }
                    (_, true, _, _) => Span::styled(" unfinished  ", Style::new().fg(Color::Yellow)),
                    (_, _, Some(_), _) => Span::styled(" ✓ done      ", Style::new().fg(Color::Green)),
                    (_, _, _, true) => Span::styled(" guide       ", Style::new().fg(Color::Green)),
                    _ => Span::styled(" diff        ", dim),
                };
                let found = match past.items {
                    1 => "1 finding".to_string(),
                    n => format!("{n} findings"),
                };
                let mut facts = Vec::new();
                match (live, past.guide) {
                    (true, false) => facts.push("the agent is reading the change".to_string()),
                    (true, true) if past.items == 0 => facts.push("no findings yet".to_string()),
                    (true, true) => facts.push(format!("{found} so far")),
                    (false, true) => facts.push(found),
                    _ => {}
                }
                facts.extend((past.guide && past.items > 0 && past.marked > 0).then(|| format!("{} of {} marked", past.marked, past.items)));
                facts.extend((past.drafts > 0).then(|| format!("{} unpublished", past.drafts)));
                facts.extend(past.done.as_ref().map(|how| format!("review {how}")));
                facts.extend([past.author.clone(), since(past.touched)]);
                facts.retain(|fact| !fact.is_empty());
                let mut row = vec![tag, Span::styled(format!("{:<24.24} ", past.name()), Style::new().fg(crate::accent()))];
                row.extend(crate::rich(&past.title, Style::new()));
                row.push(Span::styled(format!("  {}", facts.join(" · ")), dim));
                Line::from(row)
            })
            .collect();
        let count = rows.len();
        let band = if self.typing { Style::new().bg(Color::Rgb(48, 54, 68)) } else { Style::new().add_modifier(Modifier::REVERSED) };
        self.state.select(Some(sel));
        frame.render_stateful_widget(List::new(rows).highlight_style(band), body, &mut self.state);
        crate::scrollbar(frame, Rect { y: body.y.saturating_sub(1), height: body.height + 2, ..body }, count, self.state.offset(), !self.typing);
        if total == 0 {
            let lines = vec![
                Line::raw(""),
                Line::styled("   Nothing reviewed here yet.", bold),
                Line::raw(""),
                Line::styled("   Copy the link of a merge request or pull request and paste it.", dim),
                Line::styled("   An agent reviews it and writes the summary first, then one finding at a time.", dim),
                Line::styled("   You read each one, ask the agent about it or have it checked, and publish what you agree with.", dim),
            ];
            frame.render_widget(ratatui::text::Text::from(lines), body);
        } else if count == 0 {
            frame.render_widget(Line::styled(format!("   No review matches \"{}\"", self.field.trim()), dim), Rect { height: 1.min(body.height), ..body });
        }

        if let Some((filter, selected)) = &self.palette {
            self.buttons = crate::draw_palette(frame, &COMMANDS, filter, *selected, self.palette_cursor);
        }
        if self.help {
            self.buttons = crate::draw_help(frame);
        }
    }
}

pub fn reset_config() -> Option<PathBuf> {
    let path = config_path();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let backup = path.with_file_name(format!("config.json.{stamp}.bak"));
    fs::rename(&path, &backup).ok().map(|_| backup)
}

pub fn setup_with_agent() -> bool {
    let agent = agents().remove(0);
    let mut command = Command::new("sh");
    command.args(["-c", &agent.run]).env("REVUI_CLAUDE", claude()).env("REVUI_PROMPT", crate::SETUP_PROMPT).env("REVUI_SESSION_ID", crate::uuid());
    command.status().is_ok_and(|status| status.success())
}

pub fn config_editor() -> Command {
    let path = config_path();
    if !path.exists() {
        let _ = fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
        let _ = fs::write(&path, "{\n}\n");
    }
    let mut command = Command::new("sh");
    command.args(["-c", "${VISUAL:-${EDITOR:-vi}} \"$1\"", "sh"]).arg(path);
    command
}

const COMMANDS: [crate::Command; 7] = [
    ("Open this review", "enter", KeyCode::Enter),
    ("Type or paste a link to start a review, or filter your reviews", "tab", KeyCode::Tab),
    ("Rescan your reviews", "r", KeyCode::Char('r')),
    ("Settings: edit the config file", ", or e", KeyCode::Char(',')),
    ("Set up revui with an agent", "", KeyCode::F(11)),
    ("Reset settings: start over with a blank config (keeps a backup)", "", KeyCode::F(10)),
    ("Quit", "q", KeyCode::Char('q')),
];

/// The front door: paste a link to start a review, or pick up one you opened before.
pub fn browse(terminal: &mut DefaultTerminal, images: Picker, first: Option<String>) -> io::Result<()> {
    let mut home = Home::default();
    apply_accent();
    home.scan();
    home.typing = home.past.is_empty();
    let mut pending = None;
    let mut pasted = first;
    let mut redraw = true;
    loop {
        crate::fade(&mut home.message);
        let shown = home.tabs.shown;
        for name in home.tabs.tick() {
            home.message = format!("{name} exited right away with an error. Check its command in the config.");
        }
        let configuring = home.tabs.has("settings") || home.tabs.has("setup");
        if std::mem::replace(&mut home.configuring, configuring) && !configuring {
            apply_accent();
        }
        // A review in progress keeps landing findings: keep its row current.
        if !home.tabs.is_empty() && home.scanned.is_none_or(|at| at.elapsed().as_millis() > 1500) {
            home.scan();
        }
        if redraw || shown != home.tabs.shown {
            terminal.draw(|f| {
                home.draw(f);
                home.tabs.draw(f, "back to your reviews");
            })?;
        }
        redraw = true;
        if let Some(text) = home.clipboard.take().or(home.tabs.clipboard.take()) {
            crate::copy(&text);
        }
        if home.tabs.shown {
            match event::poll(std::time::Duration::from_millis(30))? {
                true => home.tabs.handle(event::read()?),
                false => redraw = home.tabs.changed(),
            }
            continue;
        }
        let waiting = !home.tabs.is_empty() || home.message.starts_with("Copied ");
        if waiting && pending.is_none() && pasted.is_none() && !event::poll(std::time::Duration::from_millis(150))? {
            continue;
        }
        let modal = home.help || home.palette.is_some();
        let code = match (pending.take(), pasted.is_some()) {
            (Some(code), _) => code,
            (None, true) => KeyCode::Null,
            (None, false) => match event::read()? {
                Event::Paste(_) if modal => continue,
                Event::Paste(text) => match link(&text) {
                    Some(url) => {
                        pasted = Some(url);
                        KeyCode::Null
                    }
                    None => {
                        let mut chars: Vec<char> = home.field.chars().collect();
                        let at = home.cursor.min(chars.len());
                        let clean: Vec<char> = text.chars().filter(|c| !c.is_control()).collect();
                        chars.splice(at..at, clean.iter().copied());
                        (home.field, home.cursor, home.typing, home.sel) = (chars.into_iter().collect(), at + clean.len(), true, 0);
                        continue;
                    }
                },
                Event::Mouse(m) => {
                    let at = Position::new(m.column, m.row);
                    match m.kind {
                        MouseEventKind::Down(_) if modal => {
                            pending = home.buttons.iter().find(|(area, _)| area.contains(at)).map(|&(_, code)| code);
                            if home.palette.is_some() && pending.is_some_and(|code| !matches!(code, KeyCode::Enter | KeyCode::Esc)) {
                                home.palette = None;
                            }
                            home.selecting = pending.is_none().then_some((at, at));
                        }
                        MouseEventKind::Drag(_) if modal => home.selecting = home.selecting.map(|(start, _)| (start, at)),
                        MouseEventKind::Up(_) if modal => match home.selecting {
                            Some((start, end)) if start != end => home.copy = true,
                            _ => home.selecting = None,
                        },
                        _ if modal => {}
                        MouseEventKind::Down(_) if home.actions.iter().any(|(button, _)| button.contains(at)) => {
                            home.typing = false;
                            pending = home.actions.iter().find(|(button, _)| button.contains(at)).map(|&(_, code)| code);
                        }
                        MouseEventKind::Down(_) if home.field_area.contains(at) => {
                            let clicked = (m.column as usize).saturating_sub(home.field_area.x as usize + 3);
                            (home.typing, home.cursor) = (true, clicked.min(home.field.chars().count()));
                        }
                        MouseEventKind::ScrollDown => home.sel += 1,
                        MouseEventKind::ScrollUp => home.sel = home.sel.saturating_sub(1),
                        MouseEventKind::Down(_) if home.body.contains(at) => {
                            let row = home.state.offset() + (m.row - home.body.y) as usize;
                            if row < home.visible().len() {
                                pending = (home.sel == row && !home.typing).then_some(KeyCode::Enter);
                                (home.sel, home.typing) = (row, false);
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                Event::Key(key) if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                Event::Key(key) if key.kind == KeyEventKind::Press && home.palette.is_some() && !matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Up | KeyCode::Down) => {
                    if let Some((filter, selected)) = home.palette.as_mut() {
                        let before = filter.clone();
                        crate::edit(filter, &mut home.palette_cursor, key);
                        if *filter != before {
                            *selected = 0;
                        }
                    }
                    continue;
                }
                Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) => KeyCode::F(12),
                Event::Key(key) if key.kind == KeyEventKind::Press && home.typing && !modal => {
                    let count = home.visible().len();
                    match key.code {
                        KeyCode::Esc if home.field.is_empty() && home.past.is_empty() => return Ok(()),
                        KeyCode::Esc => (home.field, home.cursor, home.typing, home.sel) = (String::new(), 0, home.past.is_empty(), 0),
                        KeyCode::Enter => match (link(&home.field), home.field.trim().is_empty(), count) {
                            (Some(url), _, _) => pasted = Some(url),
                            (_, true, 0) => {}
                            (_, true, _) => home.typing = false,
                            (_, _, 0) => home.message = "That is not a merge request or pull request link, and no review matches it.".into(),
                            _ => (home.typing, pending) = (false, Some(KeyCode::Enter)),
                        },
                        KeyCode::Down | KeyCode::Tab | KeyCode::BackTab if count > 0 => home.typing = false,
                        KeyCode::Up if count > 0 => (home.typing, home.sel) = (false, count - 1),
                        KeyCode::Down | KeyCode::Up | KeyCode::Tab | KeyCode::BackTab => {}
                        _ => {
                            crate::edit(&mut home.field, &mut home.cursor, key);
                            home.sel = 0;
                        }
                    }
                    continue;
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('/') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::Char('?'),
                    code => code,
                },
                _ => continue,
            },
        };
        home.selecting = None;
        if home.help {
            home.help = false;
            continue;
        }
        if home.palette.is_some() {
            pending = crate::palette_key(&COMMANDS, &mut home.palette, code);
            continue;
        }
        let count = home.visible().len().max(1);
        match code {
            KeyCode::Char('q') => return Ok(()),
            KeyCode::Esc if !home.field.is_empty() => (home.field, home.cursor, home.sel) = (String::new(), 0, 0),
            KeyCode::Esc => return Ok(()),
            KeyCode::Char('/') => (home.palette, home.palette_cursor) = (Some((String::new(), 0)), 0),
            KeyCode::Char('?') => home.help = true,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('f' | 'n' | 'i') => (home.typing, home.cursor) = (true, home.field.chars().count()),
            KeyCode::Char('j') | KeyCode::Down => home.sel = (home.sel + 1) % count,
            KeyCode::Char('k') | KeyCode::Up => home.sel = (home.sel + count - 1) % count,
            KeyCode::Char('r') => home.scan(),
            KeyCode::F(12) if home.tabs.is_empty() => home.message = "No agent is open: one starts when you paste a link.".into(),
            KeyCode::F(12) => home.tabs.open(),
            KeyCode::Char('e' | ',') => {
                if let Err(e) = home.tabs.run("settings", &config_editor(), terminal.size()?.into()) {
                    home.message = format!("Could not open the config in your editor: {e}");
                }
            }
            KeyCode::F(10) => {
                home.message = match reset_config() {
                    Some(backup) => format!("Settings reset. The old config is at {}", backup.display()),
                    None => "There was no config to reset.".into(),
                };
            }
            KeyCode::F(11) => {
                let agent = agents().remove(0);
                let mut command = Command::new("sh");
                command.args(["-c", &agent.run]).env("REVUI_CLAUDE", claude()).env("REVUI_PROMPT", crate::SETUP_PROMPT).env("REVUI_SESSION_ID", crate::uuid());
                let typing = (agent.modal && !agent.run.contains("REVUI_PROMPT")).then(|| crate::SETUP_PROMPT.to_string());
                if let Err(e) = home.tabs.run_typing("setup", &command, terminal.size()?.into(), typing) {
                    home.message = format!("Could not start a coding agent ({e}). Paste this into yours: {}", crate::SETUP_PROMPT);
                }
            }
            _ => {}
        }
        // A pasted link starts a review at once. A row opens what is there, after asking the host whether the head moved.
        let target = match (pasted.take(), code) {
            (Some(url), _) => Some((url, None, true)),
            (None, KeyCode::Enter) => home.picked().map(|past| (past.url, Some(past.dir), false)),
            _ => None,
        };
        if let Some((url, dir, start)) = target {
            let short = link(&url).and_then(|url| url.rsplit('/').next().map(String::from)).unwrap_or_default();
            home.busy = Some(format!("Fetching {}", if short.is_empty() { "the review".to_string() } else { format!("#{short}") }));
            let load = move || match (url.is_empty(), dir) {
                (true, Some(dir)) => Bundle::load(&dir),
                (_, Some(dir)) => open_url(&url).or_else(|problem| Bundle::load(&dir).map_err(|_| problem)),
                (_, None) => open_url(&url),
            };
            let opened = crate::wait(terminal, |frame| home.draw(frame), load)?;
            home.busy = None;
            match opened {
                Ok(bundle) => {
                    (home.field, home.cursor, home.typing) = (String::new(), 0, false);
                    home.tabs = crate::review(terminal, bundle, images.clone(), std::mem::take(&mut home.tabs), start)?;
                    home.scan();
                    home.sel = 0;
                }
                Err(e) => home.message = e,
            }
        }
    }
}

#[cfg(test)]
pub fn sample() -> Mr {
    serde_json::from_str(
        r#"{"iid": 7, "title": "Retry order submission", "web_url": "https://example.com/shop/api/-/merge_requests/7",
            "sha": "5d6e7f8a9b0c", "source_branch": "retry-submit", "target_branch": "main",
            "updated_at": "2026-11-09T16:23:34Z", "author": {"username": "sam"},
            "reviewers": [{"username": "alex"}], "references": {"full": "shop/api!7"}}"#,
    )
    .unwrap()
}

#[cfg(test)]
#[test]
fn a_link_is_found_in_whatever_was_pasted() {
    assert_eq!(link("  https://github.com/shop/api/pull/12/files#diff-1\n").as_deref(), Some("https://github.com/shop/api/pull/12/files#diff-1"));
    assert_eq!(link("see <https://gitlab.com/group/sub/project/-/merge_requests/7>, thanks").as_deref(), Some("https://gitlab.com/group/sub/project/-/merge_requests/7"));
    assert!(link("retry submit").is_none() && link("https://github.com/shop/api/issues/12").is_none());
}

#[cfg(test)]
#[test]
fn your_reviews_are_read_from_the_bundles_on_disk() {
    let root = std::env::temp_dir().join(format!("revui-history-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (old_head, new_head, other) = (root.join("shop__api-7-aaaaaaaa"), root.join("shop__api-7-bbbbbbbb"), root.join("shop__web-3-cccccccc"));
    for dir in [&old_head, &new_head] {
        sample().write_bundle(dir, "").unwrap();
    }
    let mut second = sample();
    (second.iid, second.title, second.web_url) = (3, "Tidy the cart".into(), "https://example.com/shop/web/-/merge_requests/3".into());
    second.references = References { full: "shop/web!3".into() };
    second.write_bundle(&other, "").unwrap();
    let guide = r#"{"title": "Retry order submission", "working": "Reviewing part 2 of 3: Retry the submit call",
        "sources": [{"id": "mr", "head_sha": "aaaaaaaa", "patch": "mr.patch", "url": "https://example.com/shop/api/-/merge_requests/7", "reference": "!7", "repo": "shop/api", "author": "@sam"}],
        "sections": [{"title": "Retry", "why": "x", "hunks": []}],
        "items": [{"id": "B1", "kind": "finding", "title": "t", "body": "b"}]}"#;
    fs::write(old_head.join("review.json"), guide).unwrap();
    fs::write(old_head.join("state.json"), r#"{"verdicts": {"B1": "agree"}, "notes": [{"text": "hm", "posted": false}]}"#).unwrap();

    let found = history(&root);
    assert_eq!(found.len(), 2, "two heads of one merge request show once");
    let api = found.iter().find(|past| past.reference == "!7").unwrap();
    assert!(api.guide && api.dir == old_head, "the head that has the guide is the one shown");
    assert_eq!((api.items, api.marked, api.drafts, api.name().as_str()), (1, 1, 1, "api!7"));
    assert!(api.working.starts_with("Reviewing part 2") && api.matches("API retry") && !api.matches("cart"));

    let mut home = Home { past: found, ..Default::default() };
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(150, 12)).unwrap();
    terminal.draw(|frame| home.draw(frame)).unwrap();
    let screen = format!("{}", terminal.backend());
    assert!(screen.contains("2 reviews") && screen.contains("Paste a merge request or pull request link"));
    assert!(screen.contains("unfinished") && screen.contains("api!7") && screen.contains("1 of 1 marked") && screen.contains("1 unpublished"));
    assert!(screen.contains("diff") && screen.contains("web!3") && screen.contains("Tidy the cart"));
    home.field = "cart".into();
    terminal.draw(|frame| home.draw(frame)).unwrap();
    let screen = format!("{}", terminal.backend());
    assert!(screen.contains("1 of 2 reviews") && !screen.contains("api!7"), "typing something that is not a link filters");
    home.field = "https://github.com/shop/api/pull/12".into();
    assert_eq!(home.visible().len(), 2, "a link in the field is not a filter");

    let mut empty = Home::default();
    terminal.draw(|frame| empty.draw(frame)).unwrap();
    assert!(format!("{}", terminal.backend()).contains("Nothing reviewed here yet"));
    let _ = fs::remove_dir_all(&root);
}

#[cfg(test)]
#[test]
fn github_pull_requests_map_onto_the_same_shapes() {
    let pr: serde_json::Value = serde_json::from_str(
        r#"{"number": 12, "title": "Retry order submission", "url": "https://github.com/shop/api/pull/12", "headRefOid": "5d6e7f8a9b0c",
            "headRefName": "retry-submit", "baseRefName": "main", "updatedAt": "2026-11-09T16:23:34Z", "author": {"login": "sam"},
            "reviewRequests": [{"login": "alex"}, {"name": "platform"}], "state": "MERGED"}"#,
    )
    .unwrap();
    let mr = from_github(&pr, "shop/api").unwrap();
    assert_eq!((mr.repo(), mr.reference(), mr.github()), ("shop/api", "#12".to_string(), true));
    assert!(!sample().github() && sample().reference() == "!7");
    assert_eq!(file_url("https://github.com/shop/api/pull/12", "abc", "src/a.ts", 4).unwrap(), "https://github.com/shop/api/blob/abc/src/a.ts#L4");
    assert_eq!(file_url("https://example.com/shop/api/-/merge_requests/7", "abc", "src/a.ts", 4).unwrap(), "https://example.com/shop/api/-/blob/abc/src/a.ts#L4");

    let threads = parse_github_threads(
        r#"{"data": {"repository": {"pullRequest": {
            "comments": {"nodes": [{"databaseId": 5, "author": {"login": "ci", "__typename": "Bot"}, "body": "Build passed", "createdAt": "2026-11-09T10:00:00Z"}]},
            "reviews": {"nodes": [{"databaseId": 6, "author": {"login": "alex", "__typename": "User"}, "body": "", "createdAt": "2026-11-09T11:00:00Z"}]},
            "reviewThreads": {"nodes": [{"isResolved": true, "path": "src/a.ts", "line": null, "originalLine": 9, "diffSide": "LEFT", "comments": {"nodes": [
                {"databaseId": 7, "author": {"login": "alex", "__typename": "User"}, "body": "Why three attempts?", "createdAt": "2026-11-09T12:00:00Z"},
                {"databaseId": 8, "author": {"login": "sam", "__typename": "User"}, "body": "Matches the client", "createdAt": "2026-11-09T13:00:00Z"}]}}]}}}}}"#,
    );
    assert_eq!(threads.len(), 2, "an empty review summary is not a thread");
    assert_eq!((threads[0].notes[0].0.as_str(), threads[0].anchor.as_str(), threads[0].id.as_str()), ("ci (bot)", "#issuecomment-5", ""));
    let inline = &threads[1];
    assert_eq!((inline.id.as_str(), inline.anchor.as_str(), inline.file.as_deref(), inline.line), ("7", "#discussion_r7", Some("src/a.ts"), Some(9)));
    assert!(inline.old && inline.resolved && inline.notes.len() == 2 && inline.notes[1].0 == "@sam");
}

#[cfg(test)]
#[test]
#[ignore]
fn live_code_host_reads() {
    let url = std::env::var("REVUI_LIVE").expect("set REVUI_LIVE to the URL of a merge request or pull request");
    let mr = lookup(&url).unwrap();
    let (repo, reference) = (mr.repo().to_string(), mr.reference());
    println!("{repo} {reference}: {:?} at {}", mr.title, mr.sha());
    let found = threads(&repo, &reference);
    println!("{} threads", found.len());
    for thread in found.iter().take(6) {
        let first = &thread.notes[0];
        println!("  {} {:?}:{:?} old={} resolved={} replies={} {} {:?}", thread.anchor, thread.file, thread.line, thread.old, thread.resolved, thread.notes.len() - 1, first.0, first.1.chars().take(50).collect::<String>());
    }
    println!("dates {:?}", dates(&repo, &reference));
    println!("base of {reference} is {:?}", base_sha(&repo, &reference, mr.sha()));
    let readme = raw_file(&repo, on_github(&reference), mr.sha(), "README.md").unwrap_or_default();
    println!("README.md at head starts {:?}", readme.lines().next());
}
