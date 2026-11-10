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
    #[serde(default)]
    repos: Vec<String>,
    #[serde(default)]
    groups: Vec<String>,
    host: Option<String>,
    generate: Option<String>,
    generate_modal: Option<bool>,
    claude: Option<String>,
    linear_key: Option<String>,
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
        (app("Orca.app"), "Orca", "its built-in browser, when this session runs inside Orca and has its browser tool", "your tool list"),
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
        let notes = home().join(".local/share/revui/notes").join(format!("{}.md", repo.replace('/', "__")));
        let text = fs::read_to_string(&notes).unwrap_or_default();
        out += &format!("\nNotes for {repo} ({}):\n{}\n", notes.display(), if text.trim().is_empty() { "none yet: append what you learn" } else { text.trim() });
    }
    out
}

fn config() -> Config {
    fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn generate_settings() -> (String, bool) {
    let config = config();
    match config.generate {
        Some(command) => (command, config.generate_modal.unwrap_or(false)),
        None => ("$REVUI_CLAUDE -- \"/revui-guide $REVUI_URL\"".into(), true),
    }
}

pub fn generate_command() -> String {
    generate_settings().0
}

type Projects = std::collections::HashMap<String, (String, String)>;

fn linear_key() -> Option<String> {
    std::env::var("LINEAR_API_KEY").ok().or(config().linear_key).filter(|key| !key.trim().is_empty())
}

fn parse_projects(text: &str) -> Result<Projects, String> {
    let reply: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("unreadable reply: {e}"))?;
    if let Some(message) = reply["errors"][0]["message"].as_str() {
        return Err(message.to_string());
    }
    let nodes = reply["data"]["issues"]["nodes"].as_array().ok_or("no issues in the reply")?;
    let text = |value: &serde_json::Value| value.as_str().unwrap_or("").to_string();
    Ok(nodes.iter().map(|node| (text(&node["identifier"]), (text(&node["project"]["name"]), text(&node["title"])))).collect())
}

// ponytail: one request of 250 issues, page it when a list holds more tickets than that
fn linear_projects(key: &str, issues: &[String]) -> Result<Projects, String> {
    use std::io::Write;
    if key.contains(['"', '\\', '\n']) {
        return Err("the key has a quote, backslash or line break in it".into());
    }
    let mut teams: std::collections::BTreeMap<&str, Vec<u64>> = Default::default();
    for (team, number) in issues.iter().filter_map(|issue| issue.split_once('-')) {
        teams.entry(team).or_default().extend(number.parse::<u64>());
    }
    let filters: Vec<_> = teams.iter().map(|(team, numbers)| serde_json::json!({"team": {"key": {"eq": team}}, "number": {"in": numbers}})).collect();
    let body = serde_json::json!({
        "query": "query($f: IssueFilter) { issues(filter: $f, first: 250) { nodes { identifier title project { name } } } }",
        "variables": {"f": {"or": filters}},
    });
    let mut curl = Command::new("curl");
    curl.args(["-sS", "--max-time", "20", "-K", "-", "https://api.linear.app/graphql"]);
    curl.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = curl.spawn().map_err(|e| format!("curl: {e}"))?;
    let options = format!(
        "header = \"Authorization: {}\"\nheader = \"Content-Type: application/json\"\ndata = {}\n",
        key.trim(),
        serde_json::Value::String(body.to_string())
    );
    child.stdin.take().ok_or("curl: no stdin")?.write_all(options.as_bytes()).map_err(|e| format!("curl: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    parse_projects(&String::from_utf8_lossy(&out.stdout))
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
    let host = config.host.clone().unwrap_or_default();
    let github = config.repos.iter().filter(|repo| github_repo(repo, &host).is_some()).count();
    let gitlab = config.repos.len() - github + config.groups.len();
    let (has_glab, has_gh) = (on_path("glab"), on_path("gh"));
    let (generate, open) = (generate_command().replace("$REVUI_CLAUDE", &claude()), open_command());
    let mut agent = agents().remove(0);
    agent.run = agent.run.replace("$REVUI_CLAUDE", &claude());
    let settings = config_path().display().to_string();
    let default = |set: bool| if set { "" } else { " (the default)" };
    let mut checks = vec![(has_glab || has_gh, "a code host CLI is installed".to_string(), "install glab (GitLab) or gh (GitHub)".to_string())];
    if gitlab > 0 {
        checks.push((has_glab, "glab is installed".into(), "install the GitLab CLI: https://gitlab.com/gitlab-org/cli".into()));
        checks.push((glab(&["auth", "status"]).is_ok(), "glab is signed in".into(), "run: glab auth login".into()));
    }
    if github > 0 {
        checks.push((has_gh, "gh is installed".into(), "install the GitHub CLI: https://cli.github.com".into()));
        checks.push((gh(&["auth", "status"]).is_ok(), "gh is signed in".into(), "run: gh auth login".into()));
    }
    checks.extend([
        (
            !config.repos.is_empty() || !config.groups.is_empty(),
            format!("{} repos and {} groups to list merge requests from", config.repos.len(), config.groups.len()),
            format!("no repos yet: add \"repos\" to {settings}, or press e in revui"),
        ),
        (
            !writers.is_empty(),
            format!("a skill that writes guides is installed: {}", writers.join(", ")),
            format!("no skill that writes guides in {}: run `revui skill install`", skills.display()),
        ),
        (
            on_path(&first(&generate)),
            format!("g runs{}{}: {generate}", default(config.generate.is_some()), if generate_settings().1 { " in a tab here" } else { "" }),
            format!("g would run `{generate}`, but {} is not on PATH: set \"generate\" in {settings}", first(&generate)),
        ),
        (
            on_path(&first(&open)),
            format!("enter runs{}: {open}", default(config.open.is_some())),
            format!("enter would run `{open}`, but {} is not on PATH: set \"open\" in {settings}", first(&open)),
        ),
        (
            on_path(&first(&agent.run)),
            format!("c and v start the {} agent{}: {}", agent.name, default(config.agents.is_some()), agent.run),
            format!("c and v would run `{}`, but {} is not on PATH: set \"agents\" in {settings}", agent.run, first(&agent.run)),
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

pub fn fetch(url: &str) -> Result<PathBuf, String> {
    let wrong = "expected a GitLab merge request URL or a GitHub pull request URL";
    let number = |rest: &str| rest.chars().take_while(char::is_ascii_digit).collect::<String>();
    if let Some((project, rest)) = url.split_once("/pull/") {
        let repo = project.splitn(4, '/').nth(3).ok_or(wrong)?;
        let found = gh(&["pr", "view", &number(rest), "-R", repo, "--json", PR_FIELDS])?;
        let pr: serde_json::Value = serde_json::from_str(&found).map_err(|e| format!("gh pr view: {e}"))?;
        return from_github(&pr, repo).ok_or("gh pr view: an unexpected reply")?.materialize();
    }
    let (project, rest) = url.split_once("/-/merge_requests/").ok_or(wrong)?;
    let repo = project.splitn(4, '/').nth(3).ok_or(wrong)?;
    let mr: Mr = serde_json::from_str(&glab(&["mr", "view", &number(rest), "-R", repo, "-F", "json"])?)
        .map_err(|e| format!("glab mr view: {e}"))?;
    mr.materialize()
}

const PR_FIELDS: &str = "number,title,url,headRefOid,headRefName,baseRefName,updatedAt,author,reviewRequests,state";

fn from_github(pr: &serde_json::Value, repo: &str) -> Option<Mr> {
    let text = |key: &str| pr[key].as_str().unwrap_or("").to_string();
    let login = |user: &serde_json::Value| User { username: user["login"].as_str().or(user["name"].as_str()).unwrap_or("").to_string() };
    let number = pr["number"].as_u64()?;
    Some(Mr {
        iid: number,
        title: text("title"),
        web_url: text("url"),
        sha: pr["headRefOid"].as_str().map(String::from),
        source_branch: text("headRefName"),
        target_branch: text("baseRefName"),
        updated_at: text("updatedAt"),
        author: login(&pr["author"]),
        reviewers: pr["reviewRequests"].as_array().map(|all| all.iter().map(login).collect()).unwrap_or_default(),
        references: References { full: format!("{repo}#{number}") },
        state: if pr["state"] == "MERGED" { "merged".into() } else { "opened".into() },
        guide: false,
        older_guide: false,
    })
}

/// A config entry names a GitHub repo when it says so (`github.com/owner/repo`,
/// with or without `https://`), or when `"host": "github"` makes that the default.
fn github_repo(entry: &str, default_host: &str) -> Option<String> {
    let bare = entry.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
    match bare.strip_prefix("github.com/") {
        Some(repo) => Some(repo.to_string()),
        None if default_host == "github" && !bare.starts_with("gitlab.") => Some(bare.to_string()),
        None => None,
    }
}

fn gitlab_repo(entry: &str) -> String {
    let bare = entry.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
    bare.strip_prefix("gitlab.com/").unwrap_or(bare).to_string()
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
    updated_at: String,
    author: User,
    #[serde(default)]
    reviewers: Vec<User>,
    references: References,
    #[serde(default)]
    state: String,
    #[serde(skip)]
    guide: bool,
    #[serde(skip)]
    older_guide: bool,
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn config_path() -> PathBuf {
    home().join(".config/revui/config.json")
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

    fn issue(&self) -> String {
        if let Some((start, end)) = crate::ticket(&self.title) {
            return self.title[start..end].to_string();
        }
        let branch = self.source_branch.rsplit('/').next().unwrap_or("").to_uppercase();
        match crate::ticket(&branch) {
            Some((0, end)) => branch[..end].to_string(),
            _ => "No ticket".to_string(),
        }
    }

    fn issue_title(&self) -> String {
        let title = self.title.strip_prefix("Draft: ").unwrap_or(&self.title);
        match crate::ticket(title) {
            Some((_, end)) => title[end..].trim_start_matches([')', ':', ' ', '-', ',']).to_string(),
            None if self.issue() == "No ticket" => "merge requests with no ticket in the title or branch".to_string(),
            None => title.to_string(),
        }
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
        home().join(".local/share/revui").join(format!("{}-{}-{sha}", self.repo().replace('/', "__"), self.iid))
    }

    fn matches(&self, filter: &str) -> bool {
        let reviewers: Vec<_> = self.reviewers.iter().map(|r| r.username.as_str()).collect();
        let haystack = format!(
            "{} {} {} {} {} {}",
            self.repo(),
            self.reference(),
            self.title,
            self.author.username,
            self.source_branch,
            reviewers.join(" ")
        )
        .to_lowercase();
        filter.to_lowercase().split_whitespace().all(|term| haystack.contains(term))
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

fn fetch_all(merged: bool) -> (Vec<Mr>, String) {
    let config = config();
    let (mut all, mut message) = (Vec::new(), String::new());
    if config.repos.is_empty() && config.groups.is_empty() {
        message = format!("No repos configured. Press e to edit {}", config_path().display());
    }
    let host = config.host.clone().unwrap_or_default();
    let repos = config.repos.iter().map(|r| ("-R", r));
    for (flag, name) in repos.chain(config.groups.iter().map(|g| ("-g", g))) {
        let listed = match github_repo(name, &host).filter(|_| flag == "-R") {
            Some(repo) => {
                let state = if merged { "merged" } else { "open" };
                let found = gh(&["pr", "list", "-R", &repo, "--state", state, "-L", "100", "--search", "sort:updated-desc", "--json", PR_FIELDS]);
                let prs = found.and_then(|out| serde_json::from_str::<Vec<serde_json::Value>>(&out).map_err(|e| e.to_string()));
                prs.map(|prs| prs.iter().filter_map(|pr| from_github(pr, &repo)).collect())
            }
            None => {
                let project = gitlab_repo(name);
                let mut args = vec!["mr", "list", flag, project.as_str(), "-F", "json", "-P", "100", "-o", "updated_at", "-S", "desc"];
                args.extend(merged.then_some("--merged"));
                glab(&args).and_then(|out| serde_json::from_str::<Vec<Mr>>(&out).map_err(|e| e.to_string()))
            }
        };
        match listed {
            Ok(mrs) => all.extend(mrs),
            Err(e) => message = format!("{name}: {e}"),
        }
    }
    all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    all.dedup_by(|a, b| a.web_url == b.web_url);
    for mr in &mut all {
        mr.guide = has_guide(&mr.dir());
        mr.older_guide = !mr.guide && mr.earlier_guide().is_some();
    }
    (all, message)
}

#[derive(Clone)]
enum Entry {
    Project(String),
    Issue(String),
    Mr(usize),
}

#[derive(Default)]
struct MrList {
    tabs: crate::Tabs,
    busy: Option<String>,
    palette: Option<(String, usize)>,
    palette_cursor: usize,
    welcome: bool,
    view: usize,
    expanded: std::collections::HashSet<String>,
    folded: std::collections::HashSet<String>,
    linear: Option<String>,
    projects: Projects,
    asked: std::collections::HashSet<String>,
    lookup: Option<std::sync::mpsc::Receiver<Result<Projects, String>>>,
    loading: [bool; 2],
    cache: [Option<Vec<Mr>>; 2],
    fetches: Vec<std::sync::mpsc::Receiver<(bool, Vec<Mr>, String)>>,
    state_tabs: [Rect; 3],
    help: bool,
    selecting: Option<(Position, Position)>,
    copy: bool,
    clipboard: Option<String>,
    search_area: Rect,
    confirm: Option<Mr>,
    generating: Vec<(String, PathBuf)>,
    buttons: Vec<(Rect, KeyCode)>,
    actions: Vec<(Rect, KeyCode)>,
    mrs: Vec<Mr>,
    filter: String,
    cursor: usize,
    filtering: bool,
    sel: usize,
    message: String,
    state: ListState,
    body: Rect,
}

impl MrList {
    fn visible(&self) -> Vec<&Mr> {
        let filter = self.filter.to_lowercase();
        let linear = |m: &Mr| self.projects.get(&m.issue()).is_some_and(|(project, title)| format!("{project} {title}").to_lowercase().contains(&filter));
        self.mrs.iter().filter(|m| m.matches(&self.filter) || m.issue().to_lowercase().contains(&filter) || (self.view == 2 && linear(m))).collect()
    }

    fn entries(&self) -> Vec<Entry> {
        let shown = self.visible();
        if self.view != 2 {
            return (0..shown.len()).map(Entry::Mr).collect();
        }
        let mut keys: Vec<String> = Vec::new();
        for mr in shown.iter().filter(|mr| mr.state != "merged") {
            let key = mr.issue();
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys.sort_by_key(|key| key == "No ticket");
        let mut projects: Vec<String> = Vec::new();
        for key in &keys {
            let project = self.project(key);
            if !projects.contains(&project) {
                projects.push(project);
            }
        }
        projects.sort_by_key(|project| project == "No project");
        let mut entries = Vec::new();
        for project in projects {
            if !self.projects.is_empty() {
                entries.push(Entry::Project(project.clone()));
            }
            for key in keys.iter().filter(|key| !self.folded.contains(&project) && self.project(key) == project) {
                entries.push(Entry::Issue(key.clone()));
                if self.expanded.contains(key) {
                    entries.extend(shown.iter().enumerate().filter(|(_, mr)| mr.issue() == *key).map(|(i, _)| Entry::Mr(i)));
                }
            }
        }
        entries
    }

    fn project(&self, issue: &str) -> String {
        self.projects.get(issue).map(|(project, _)| project.clone()).filter(|project| !project.is_empty()).unwrap_or_else(|| "No project".into())
    }

    fn toggle(&mut self) -> bool {
        let (set, key) = match self.entries().get(self.sel).cloned() {
            Some(Entry::Project(name)) => (&mut self.folded, name),
            Some(Entry::Issue(key)) => (&mut self.expanded, key),
            _ => return false,
        };
        if !set.remove(&key) {
            set.insert(key);
        }
        true
    }

    fn picked(&self) -> Option<Mr> {
        match self.entries().get(self.sel)? {
            Entry::Mr(i) => self.visible().get(*i).map(|mr| (*mr).clone()),
            _ => None,
        }
    }

    fn states(&self) -> &'static [bool] {
        [&[false][..], &[true][..], &[false, true][..]][self.view.min(2)]
    }

    fn rebuild(&mut self) {
        let mut pool: Vec<Mr> = self.states().iter().flat_map(|&merged| self.cache[merged as usize].clone().unwrap_or_default()).collect();
        pool.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        pool.dedup_by(|a, b| a.web_url == b.web_url);
        self.mrs = pool;
    }

    fn load(&mut self) {
        apply_accent();
        self.linear = linear_key();
        for &merged in self.states() {
            if std::mem::replace(&mut self.loading[merged as usize], true) {
                continue;
            }
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let (mrs, message) = fetch_all(merged);
                let _ = sender.send((merged, mrs, message));
            });
            self.fetches.push(receiver);
        }
    }

    fn show(&mut self, view: usize) {
        (self.view, self.sel) = (view, 0);
        self.message.clear();
        self.rebuild();
        self.load();
    }

    fn absorb(&mut self) {
        let mut done = Vec::new();
        self.fetches.retain(|receiver| match receiver.try_recv() {
            Ok(result) => {
                done.push(result);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        for (merged, mrs, message) in done {
            self.loading[merged as usize] = false;
            self.cache[merged as usize] = Some(mrs);
            if self.states().contains(&merged) {
                self.message = message;
                self.rebuild();
            }
        }
        match self.lookup.as_ref().map(|lookup| lookup.try_recv()) {
            Some(Ok(Ok(projects))) => self.projects.extend(projects),
            Some(Ok(Err(problem))) => self.message = format!("Linear projects: {problem}"),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return,
            _ => {}
        }
        self.lookup = None;
        let new: Vec<String> = self.mrs.iter().map(Mr::issue).filter(|issue| issue != "No ticket" && !self.asked.contains(issue)).collect();
        if let (2, Some(key), false) = (self.view, self.linear.clone(), new.is_empty()) {
            self.asked.extend(new.iter().cloned());
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || sender.send(linear_projects(&key, &new)));
            self.lookup = Some(receiver);
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        crate::clear_popup();
        self.draw_list(frame);
        if let Some(text) = crate::popup_selection(frame, self.selecting) {
            if self.copy {
                self.copy = false;
                self.message = crate::copied(text.chars().count());
                self.clipboard = Some(text);
            }
        }
    }

    fn draw_list(&mut self, frame: &mut Frame) {
        let tabs = if self.tabs.is_empty() { "" } else { " · ctrl-q tabs" };
        let around = format!("q/esc quit · / commands · ? help · , settings{tabs}");
        let keys = "j/k move · f search · o open MRs · m merged MRs · i by issue · enter open or expand · g guide · r refresh";
        let picked = self.picked();
        let wanted: Vec<(&str, &str, KeyCode, Color)> = match &picked {
            Some(mr) if mr.guide => vec![("enter", "Open guide", KeyCode::Enter, Color::Green)],
            Some(_) => vec![("enter", "Open diff", KeyCode::Enter, Color::Green), ("g", "Generate guide", KeyCode::Char('g'), crate::accent())],
            None => Vec::new(),
        };
        let [head, body, foot] = Layout::vertical([Constraint::Length(3), Constraint::Min(0), Constraint::Length(2)]).areas(frame.area());
        self.actions = crate::footer(frame, foot, &wanted, keys, &around);
        self.search_area = Rect { y: head.y + 1, height: 1, ..head };
        self.state_tabs = [Rect::new(head.x + 1, head.y, 6, 1), Rect::new(head.x + 8, head.y, 8, 1), Rect::new(head.x + 17, head.y, 8, 1)];
        let total = self.mrs.len();
        let sel = self.sel.min(self.entries().len().saturating_sub(1));
        (self.sel, self.body) = (sel, body);
        self.state.select(Some(sel));
        let entries = self.entries();
        let shown = self.visible();
        let nothing_matches = shown.is_empty();
        let dim = Style::new().fg(Color::DarkGray);

        let waiting = self.states().iter().any(|&merged| self.loading[merged as usize]);
        let loading = waiting.then(|| format!("Loading {}", ["open merge requests", "merged merge requests", "issues"][self.view.min(2)]));
        let status = match self.busy.clone().or(loading) {
            Some(busy) => vec![Span::raw(" "), crate::spinner(), Span::raw(format!(" {busy}…"))],
            None if self.message.is_empty() => Vec::new(),
            None => vec![Span::styled(format!(" {}", self.message), Style::new().fg(Color::Yellow))],
        };
        let field = match (self.filtering, self.filter.is_empty()) {
            (true, true) => Span::raw(" "),
            (_, false) => Span::raw(format!(" {} ", self.filter)),
            (false, true) => Span::styled("· f", dim),
        };
        let search = Line::from(vec![Span::styled(" Search ", Style::new().fg(crate::accent()).add_modifier(Modifier::BOLD)), field]);

        let mut title = vec![
            Span::raw(" "),
            Span::styled(" Open ", if self.view == 0 { Style::new().fg(Color::Black).bg(crate::accent()).add_modifier(Modifier::BOLD) } else { dim }),
            Span::raw(" "),
            Span::styled(" Merged ", if self.view == 1 { Style::new().fg(Color::Black).bg(Color::Magenta).add_modifier(Modifier::BOLD) } else { dim }),
            Span::raw(" "),
            Span::styled(" Issues ", if self.view == 2 { Style::new().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD) } else { dim }),
            Span::styled(if shown.len() == total { format!("  {total} merge requests") } else { format!("  {} of {total} merge requests", shown.len()) }, dim),
        ];
        if self.view == 2 && self.linear.is_none() {
            title.push(Span::styled("  · add a Linear key in settings to group by project too", dim));
        }
        let empty_status = match total {
            0 => Line::from(status),
            _ => {
                title.push(Span::raw("  "));
                title.extend(status);
                Line::raw("")
            }
        };
        frame.render_widget(ratatui::text::Text::from(vec![Line::from(title), search]), head);
        let modal = self.help || self.welcome || self.confirm.is_some() || self.palette.is_some();
        if self.filtering && !modal {
            let column = self.cursor.min(self.filter.chars().count()) as u16;
            frame.set_cursor_position(Position::new(head.x + 9 + column, head.y + 1));
        }

        let mr_row = |mr: &Mr, nested: bool| {
            let tag = match (mr.guide, mr.older_guide) {
                _ if self.generating.iter().any(|(url, _)| *url == mr.web_url) => {
                    let mut spinner = crate::spinner();
                    spinner.content = format!(" {} ...  ", spinner.content).into();
                    spinner
                }
                (true, _) => Span::styled(" guide ", Style::new().fg(Color::Green)),
                (_, true) => Span::styled(" older ", Style::new().fg(Color::Yellow)),
                _ => Span::styled(" diff  ", dim),
            };
            let repo = mr.repo().rsplit('/').next().unwrap_or("");
            let mut row = vec![Span::raw(if !nested { "" } else if self.projects.is_empty() { "     " } else { "       " }), tag];
            row.push(Span::styled(format!("{repo:<18.18} {:<7}", mr.reference()), Style::new().fg(crate::accent())));
            if nested && mr.state == "merged" {
                row.push(Span::styled("merged ", Style::new().fg(Color::Magenta)));
            }
            row.extend(crate::rich(&mr.title, Style::new()));
            row.push(Span::styled(format!("  @{} · updated {}", mr.author.username, crate::ago(&mr.updated_at)), dim));
            Line::from(row)
        };
        let rows: Vec<Line> = entries
            .iter()
            .map(|entry| match entry {
                Entry::Mr(i) => mr_row(shown[*i], self.view == 2),
                Entry::Project(name) => {
                    let mine: Vec<&&Mr> = shown.iter().filter(|mr| self.project(&mr.issue()) == *name).collect();
                    let mut issues: Vec<String> = mine.iter().map(|mr| mr.issue()).collect();
                    issues.sort();
                    issues.dedup();
                    let open = mine.iter().filter(|mr| mr.state != "merged").count();
                    let arrow = if self.folded.contains(name) { "▸" } else { "▾" };
                    Line::from(vec![
                        Span::styled(format!(" {arrow} {name}"), Style::new().fg(crate::accent()).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("  {} issue{} · {open} open · {} merged", issues.len(), if issues.len() == 1 { "" } else { "s" }, mine.len() - open), dim),
                    ])
                }
                Entry::Issue(key) => {
                    let mine: Vec<&&Mr> = shown.iter().filter(|mr| mr.issue() == *key).collect();
                    let open = mine.iter().filter(|mr| mr.state != "merged").count();
                    let guides = mine.iter().filter(|mr| mr.guide || mr.older_guide).count();
                    let arrow = if self.expanded.contains(key) { "▾" } else { "▸" };
                    let linear = self.projects.get(key).map(|(_, title)| title.clone()).filter(|title| !title.is_empty());
                    let name = linear.unwrap_or_else(|| mine.first().map_or(String::new(), |mr| mr.issue_title()));
                    let indent = if self.projects.is_empty() { "" } else { "  " };
                    let mut counts = vec![format!("{} open", open), format!("{} merged", mine.len() - open)];
                    counts.retain(|part| !part.starts_with("0 "));
                    counts.extend((guides > 0).then(|| format!("{guides} with a guide")));
                    let updated = mine.first().map_or(String::new(), |mr| crate::ago(&mr.updated_at));
                    Line::from(vec![
                        Span::styled(format!("{indent} {arrow} "), Style::new().fg(Color::Yellow)),
                        Span::styled(format!("{key:<10}"), Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                        Span::styled(name, Style::new().add_modifier(Modifier::BOLD)),
                        Span::styled(format!("  {} · updated {updated}", counts.join(" · ")), dim),
                    ])
                }
            })
            .collect();
        let list = List::new(rows).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let mut state = self.state.clone();
        frame.render_stateful_widget(list, body, &mut state);
        let bar = Rect { y: body.y.saturating_sub(1), height: body.height + 2, ..body };
        crate::scrollbar(frame, bar, entries.len(), state.offset(), true);
        let first_row = Rect { height: 1.min(body.height), ..body };
        if self.mrs.is_empty() {
            frame.render_widget(empty_status, first_row);
        } else if nothing_matches {
            frame.render_widget(Line::styled(format!(" No results for \"{}\"", self.filter), Style::new().fg(Color::DarkGray)), first_row);
        }

        if let Some((filter, selected)) = &self.palette {
            self.buttons = crate::draw_palette(frame, &COMMANDS, filter, *selected, self.palette_cursor);
        }
        if self.help {
            self.buttons = crate::draw_help(frame);
        }
        if let Some(mr) = &self.confirm {
            let heading = Style::new().fg(crate::accent()).add_modifier(Modifier::BOLD);
            let repo = mr.repo().rsplit('/').next().unwrap_or("");
            let mut lines = vec![Line::styled(format!("{repo} {}", mr.reference()), heading), Line::raw(mr.title.clone()), Line::raw("")];
            let note = match (mr.guide, mr.older_guide) {
                (true, _) => "⚠ This replaces the guide it already has.",
                (_, true) => "⚠ Its guide is for an earlier head. This updates it for the new commits.",
                _ => "",
            };
            if !note.is_empty() {
                lines.extend([Line::styled(note, Style::new().fg(Color::Yellow)), Line::raw("")]);
            }
            lines.extend([Line::styled("Starts an agent run: a few minutes, and it spends tokens.", dim), Line::raw("")]);
            let width = frame.area().width.saturating_sub(4).min(84).saturating_sub(8).max(10) as usize;
            for (i, part) in crate::wrap(&generate_command().replace("$REVUI_URL", &mr.web_url).replace("$REVUI_CLAUDE", &claude()), width).into_iter().enumerate() {
                let prompt = Span::styled(if i == 0 { "$ " } else { "  " }, dim);
                lines.push(Line::from(vec![prompt, Span::styled(part, Style::new().fg(Color::Green))]));
            }
            let buttons = [("y", "Start", KeyCode::Char('y')), ("n", "Cancel", KeyCode::Char('n'))];
            self.buttons = crate::popup(frame, " Generate a guide ", lines, &buttons).1;
        }
        if self.welcome {
            let lines = vec![
                Line::styled("Welcome to revui", Style::new().fg(crate::accent()).add_modifier(Modifier::BOLD)),
                Line::raw(""),
                Line::raw("It is not set up yet: it needs to know which repos you review,"),
                Line::raw("and where you like agents and files to open."),
                Line::raw(""),
                Line::styled("An agent can do this for you. It looks at your terminal and editor,", dim),
                Line::styled("asks which repos you review, and writes the config.", dim),
            ];
            let buttons = [("a", "Set up with an agent", KeyCode::Char('a')), ("e", "Edit the config myself", KeyCode::Char('e')), ("esc", "Later", KeyCode::Esc)];
            self.buttons = crate::popup(frame, " Setup ", lines, &buttons).1;
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
        let _ = fs::write(&path, "{\n  \"repos\": [],\n  \"groups\": []\n}\n");
    }
    let mut command = Command::new("sh");
    command.args(["-c", "${VISUAL:-${EDITOR:-vi}} \"$1\"", "sh"]).arg(path);
    command
}

const COMMANDS: [crate::Command; 12] = [
    ("Generate a guide for this merge request", "g", KeyCode::Char('g')),
    ("Reset settings: start over with a blank config (keeps a backup)", "", KeyCode::F(10)),
    ("Set up revui with an agent", "", KeyCode::F(11)),
    ("Open this merge request", "enter", KeyCode::Enter),
    ("Search the list", "f", KeyCode::Char('f')),
    ("Leave the search bar, then clear the search, then quit", "esc", KeyCode::Esc),
    ("Refresh the list", "r", KeyCode::Char('r')),
    ("Show open merge requests", "o", KeyCode::Char('o')),
    ("Show merged merge requests", "m", KeyCode::Char('m')),
    ("Show merge requests grouped by issue", "i", KeyCode::Char('i')),
    ("Settings: edit the config file", ", or e", KeyCode::Char(',')),
    ("Quit", "q", KeyCode::Char('q')),
];

pub fn browse(terminal: &mut DefaultTerminal, images: Picker) -> io::Result<()> {
    let mut list = MrList::default();
    let mut pending = None;
    list.welcome = !config_path().exists();
    list.load();
    let mut redraw = true;
    loop {
        list.absorb();
        crate::fade(&mut list.message);
        let (shown, settings) = (list.tabs.shown, list.tabs.has("settings") || list.tabs.has("setup"));
        for name in list.tabs.tick() {
            list.message = format!("{name} exited right away with an error. Check its command in the config.");
        }
        if settings && !(list.tabs.has("settings") || list.tabs.has("setup")) {
            list.load();
        }
        if redraw || shown != list.tabs.shown {
            terminal.draw(|f| {
                list.draw(f);
                list.tabs.draw(f, "back to the list");
            })?;
        }
        redraw = true;
        if let Some(text) = list.clipboard.take().or(list.tabs.clipboard.take()) {
            crate::copy(&text);
        }
        if list.tabs.shown {
            match event::poll(std::time::Duration::from_millis(30))? {
                true => list.tabs.handle(event::read()?),
                false => redraw = list.tabs.changed(),
            }
            continue;
        }
        let mut open = false;
        let waiting = !list.generating.is_empty() || !list.fetches.is_empty() || list.lookup.is_some() || list.message.starts_with("Copied ");
        if waiting && pending.is_none() && !event::poll(std::time::Duration::from_millis(120))? {
            let done: Vec<String> = list.generating.iter().filter(|(_, dir)| has_guide(dir)).map(|(url, _)| url.clone()).collect();
            list.generating.retain(|(url, _)| !done.contains(url));
            for mr in list.mrs.iter_mut().filter(|mr| done.contains(&mr.web_url)) {
                (mr.guide, mr.older_guide) = (true, false);
            }
            continue;
        }
        let code = match pending.take() {
            Some(code) => code,
            None => match event::read()? {
                Event::Mouse(m) => {
                    let at = Position::new(m.column, m.row);
                    let modal = list.help || list.welcome || list.confirm.is_some() || list.palette.is_some();
                    match m.kind {
                        MouseEventKind::Down(_) if modal => {
                            pending = list.buttons.iter().find(|(area, _)| area.contains(at)).map(|&(_, code)| code);
                            if list.palette.is_some() && pending.is_some_and(|code| !matches!(code, KeyCode::Enter | KeyCode::Esc)) {
                                list.palette = None;
                            }
                            list.selecting = pending.is_none().then_some((at, at));
                        }
                        MouseEventKind::Drag(_) if modal => list.selecting = list.selecting.map(|(start, _)| (start, at)),
                        MouseEventKind::Up(_) if modal => match list.selecting {
                            Some((start, end)) if start != end => list.copy = true,
                            _ => list.selecting = None,
                        },
                        _ if modal => {}
                        MouseEventKind::Down(_) if list.actions.iter().any(|(button, _)| button.contains(at)) => {
                            pending = list.actions.iter().find(|(button, _)| button.contains(at)).map(|&(_, code)| code);
                        }
                        MouseEventKind::Down(_) if list.state_tabs.iter().any(|tab| tab.contains(at)) => {
                            let wanted = list.state_tabs.iter().position(|tab| tab.contains(at)).unwrap_or(list.view);
                            pending = (wanted != list.view).then_some(KeyCode::Char(['o', 'm', 'i'][wanted]));
                        }
                        MouseEventKind::Down(_) if list.search_area.contains(at) => {
                            let clicked = (m.column as usize).saturating_sub(list.search_area.x as usize + 9);
                            (list.filtering, list.cursor) = (true, clicked.min(list.filter.chars().count()));
                        }
                        MouseEventKind::Down(_) if list.filtering => list.filtering = list.visible().is_empty() && !list.mrs.is_empty(),
                        MouseEventKind::ScrollDown => list.sel += 1,
                        MouseEventKind::ScrollUp => list.sel = list.sel.saturating_sub(1),
                        MouseEventKind::Down(_) if list.body.contains(at) => {
                            let row = list.state.offset() + (m.row - list.body.y) as usize;
                            if row < list.entries().len() {
                                pending = (list.sel == row).then_some(KeyCode::Enter);
                                list.sel = row;
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                Event::Key(key) if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                Event::Key(key) if key.kind == KeyEventKind::Press && list.palette.is_some() && !matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Up | KeyCode::Down) => {
                    if let Some((filter, selected)) = list.palette.as_mut() {
                        let before = filter.clone();
                        crate::edit(filter, &mut list.palette_cursor, key);
                        if *filter != before {
                            *selected = 0;
                        }
                    }
                    continue;
                }
                Event::Key(key) if key.kind == KeyEventKind::Press && list.filtering && !list.help && list.palette.is_none() && list.confirm.is_none() && !list.welcome => {
                    let none = list.visible().is_empty() && !list.mrs.is_empty();
                    match key.code {
                        KeyCode::Esc if none => (list.filter, list.cursor) = (String::new(), 0),
                        KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up if none => {}
                        KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Esc => (list.filtering, list.sel) = (false, 0),
                        KeyCode::Up => (list.filtering, list.sel) = (false, list.entries().len().saturating_sub(1)),
                        _ => {
                            crate::edit(&mut list.filter, &mut list.cursor, key);
                            list.sel = 0;
                        }
                    }
                    continue;
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyCode::F(12),
                    KeyCode::Char('/') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::Char('?'),
                    code => code,
                },
                _ => continue,
            },
        };
        list.selecting = None;
        if list.help {
            list.help = false;
            continue;
        }
        if let Some(mr) = list.confirm.take() {
            match code {
                KeyCode::Char('y') => {
                    list.busy = Some(format!("Fetching {}", mr.reference()));
                    let fetching = mr.clone();
                    let fetched = crate::wait(terminal, |frame| list.draw(frame), move || fetching.materialize())?;
                    list.busy = None;
                    let screen = terminal.size()?.into();
                    let started = fetched.and_then(|dir| {
                        let (script, modal) = generate_settings();
                        let mut sh = Command::new("sh");
                        sh.args(["-c", &script]).env("REVUI_CLAUDE", claude()).env("REVUI_URL", &mr.web_url).env("REVUI_DIR", &dir);
                        if modal {
                            let name = format!("guide {}", mr.reference());
                            list.tabs.close(&name);
                            list.tabs.run(&name, &sh, screen)?;
                            return Ok(dir);
                        }
                        let log = fs::File::create(dir.join("generate.log")).map_err(|e| e.to_string())?;
                        sh.stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
                        sh.spawn().map_err(|e| e.to_string())?;
                        Ok(dir)
                    });
                    match started {
                        Ok(dir) => list.generating.push((mr.web_url.clone(), dir)),
                        Err(e) => list.message = format!("Could not start the generator: {e}"),
                    }
                }
                KeyCode::Char('n' | 'q') | KeyCode::Esc => {}
                _ => list.confirm = Some(mr),
            }
            continue;
        }
        if list.welcome {
            match code {
                KeyCode::Char('a') => (list.welcome, pending) = (false, Some(KeyCode::F(11))),
                KeyCode::Char('e') => (list.welcome, pending) = (false, Some(KeyCode::Char(','))),
                KeyCode::Esc | KeyCode::Char('n' | 'q') => list.welcome = false,
                _ => {}
            }
            continue;
        }
        if list.palette.is_some() {
            pending = crate::palette_key(&COMMANDS, &mut list.palette, code);
            continue;
        }
        list.filtering = false;
        match code {
            KeyCode::Char('q') => return Ok(()),
            KeyCode::Char('/') => (list.palette, list.palette_cursor) = (Some((String::new(), 0)), 0),
            KeyCode::Char('?') => list.help = true,
            KeyCode::Char('f') | KeyCode::Tab | KeyCode::BackTab => (list.filtering, list.cursor) = (true, list.filter.chars().count()),
            KeyCode::Char('j') | KeyCode::Down => list.sel = (list.sel + 1) % list.entries().len().max(1),
            KeyCode::Char('k') | KeyCode::Up => {
                let count = list.entries().len().max(1);
                list.sel = (list.sel + count - 1) % count;
            }
            KeyCode::Esc if !list.filter.is_empty() => (list.filter, list.cursor) = (String::new(), 0),
            KeyCode::Esc => return Ok(()),
            KeyCode::Char('r') => list.load(),
            KeyCode::Char(view @ ('o' | 'm' | 'i')) => {
                let wanted = ['o', 'm', 'i'].iter().position(|key| *key == view).unwrap_or(0);
                if wanted != list.view {
                    list.show(wanted);
                }
            }
            KeyCode::Char('l' | ' ') | KeyCode::Right | KeyCode::Left | KeyCode::Char('h') => {
                list.toggle();
            }
            KeyCode::Char('g') => list.confirm = list.picked(),
            KeyCode::F(12) if list.tabs.is_empty() => list.message = "No tab is open: , opens settings in one.".into(),
            KeyCode::F(12) => list.tabs.open(),
            KeyCode::Char('e' | ',') => {
                if let Err(e) = list.tabs.run("settings", &config_editor(), terminal.size()?.into()) {
                    list.message = format!("Could not open the config in your editor: {e}");
                }
            }
            KeyCode::F(10) => {
                list.message = match reset_config() {
                    Some(backup) => format!("Settings reset. The old config is at {}", backup.display()),
                    None => "There was no config to reset.".into(),
                };
                list.mrs.clear();
                list.welcome = true;
            }
            KeyCode::F(11) => {
                let agent = agents().remove(0);
                let mut command = Command::new("sh");
                command.args(["-c", &agent.run]).env("REVUI_CLAUDE", claude()).env("REVUI_PROMPT", crate::SETUP_PROMPT).env("REVUI_SESSION_ID", crate::uuid());
                let typing = (agent.modal && !agent.run.contains("REVUI_PROMPT")).then(|| crate::SETUP_PROMPT.to_string());
                if let Err(e) = list.tabs.run_typing("setup", &command, terminal.size()?.into(), typing) {
                    list.message = format!("Could not start a coding agent ({e}). Paste this into yours: {}", crate::SETUP_PROMPT);
                }
            }
            KeyCode::Enter => open = true,
            _ => {}
        }
        if open {
            list.toggle();
        }
        let picked = list.picked().filter(|_| open);
        if let Some(mr) = picked {
            list.busy = Some(format!("Fetching {}", mr.reference()));
            let load = move || {
                let current = mr.materialize()?;
                let guided = (!has_guide(&current)).then(|| mr.earlier_guide()).flatten();
                let mut bundle = Bundle::load(guided.as_ref().unwrap_or(&current))?;
                bundle.successor = guided.is_some().then_some(current);
                Ok::<_, String>(bundle)
            };
            let opened = crate::wait(terminal, |frame| list.draw(frame), load)?;
            list.busy = None;
            match opened {
                Ok(bundle) => list.tabs = crate::review(terminal, bundle, images.clone(), std::mem::take(&mut list.tabs))?,
                Err(e) => {
                    list.message = e;
                    list.filter.clear();
                }
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
fn filter_terms_all_have_to_match() {
    let mr = sample();
    assert!(mr.matches("api retry"));
    assert!(mr.matches("ALEX !7"));
    assert!(!mr.matches("api payments"));
}

#[cfg(test)]
#[test]
fn the_list_asks_before_generating_a_guide() {
    let mut list = MrList { mrs: vec![sample()], confirm: Some(sample()), ..Default::default() };
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal.draw(|frame| list.draw(frame)).unwrap();
    let screen = format!("{}", terminal.backend());
    assert!(screen.contains("Generate a guide") && screen.contains("api !7") && screen.contains("y · Start"));
    assert_eq!(list.buttons[0].1, KeyCode::Char('y'));
    assert!(screen.contains("Search") && screen.contains("Search · f"));
    (list.confirm, list.help) = (None, true);
    let mut tall = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 44)).unwrap();
    tall.draw(|frame| list.draw(frame)).unwrap();
    let screen = format!("{}", tall.backend());
    assert!(screen.contains("A review, start to finish") && screen.contains("esc · Close"));
}

#[cfg(test)]
#[test]
fn issues_view_groups_merge_requests_by_ticket() {
    let with = |title: &str, branch: &str, url: &str, state: &str| {
        let mut mr = sample();
        (mr.title, mr.source_branch, mr.web_url, mr.state) = (title.into(), branch.into(), url.into(), state.into());
        mr
    };
    let api = with("feat(ENG-472): sync order outcomes", "x", "u1", "opened");
    let second = with("Draft: preserve outcomes", "eng-472-preserve-outcomes", "u2", "merged");
    let other = with("Tidy the readme", "tidy", "u3", "opened");
    assert_eq!((api.issue(), second.issue(), other.issue()), ("ENG-472".to_string(), "ENG-472".to_string(), "No ticket".to_string()));
    assert_eq!(api.issue_title(), "sync order outcomes");

    let shipped = with("fix(ENG-9): already merged", "x", "u4", "merged");
    let mut list = MrList { view: 2, mrs: vec![other, api, second, shipped], ..Default::default() };
    assert!(matches!(list.entries().as_slice(), [Entry::Issue(first), Entry::Issue(last)] if first == "ENG-472" && last == "No ticket"));
    list.expanded.insert("ENG-472".into());
    assert_eq!(list.entries().len(), 4, "an expanded issue lists its two merge requests, and an issue with nothing open is left out");
    list.sel = 1;
    assert_eq!(list.picked().map(|mr| mr.web_url), Some("u1".to_string()));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 20)).unwrap();
    terminal.draw(|frame| list.draw(frame)).unwrap();
    let screen = format!("{}", terminal.backend());
    assert!(screen.contains("▾ ENG-472") && screen.contains("1 open · 1 merged") && screen.contains("▸ No ticket"));
    assert!(screen.contains("add a Linear key in settings"));

    let reply = r#"{"data":{"issues":{"nodes":[{"identifier":"ENG-472","title":"Sync outcomes","project":{"name":"Checkout rework"}},{"identifier":"ENG-9","title":"Loose","project":null}]}}}"#;
    list.projects = parse_projects(reply).unwrap();
    assert_eq!(parse_projects(r#"{"errors":[{"message":"Authentication required"}]}"#).unwrap_err(), "Authentication required");
    assert!(matches!(list.entries().as_slice(), [Entry::Project(first), Entry::Issue(_), Entry::Mr(_), Entry::Mr(_), Entry::Project(last), Entry::Issue(_)] if first == "Checkout rework" && last == "No project"));
    list.filter = "checkout rew".into();
    assert_eq!(list.entries().len(), 4, "searching a project name keeps its issues");
    list.filter.clear();
    list.sel = 0;
    assert!(list.toggle() && list.entries().len() == 3, "folding a project hides its issues");
    terminal.draw(|frame| list.draw(frame)).unwrap();
    let screen = format!("{}", terminal.backend());
    assert!(screen.contains("▸ Checkout rework  1 issue · 1 open · 1 merged") && screen.contains("▾ No project"));
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
    assert_eq!((mr.repo(), mr.reference(), mr.state.as_str(), mr.github()), ("shop/api", "#12".to_string(), "merged", true));
    assert!(mr.matches("alex #12 retry") && mr.matches("platform") && !sample().github() && sample().reference() == "!7");
    assert_eq!(github_repo("https://github.com/shop/api/", ""), Some("shop/api".into()));
    assert_eq!((github_repo("shop/api", ""), github_repo("shop/api", "github")), (None, Some("shop/api".into())));
    assert_eq!((gitlab_repo("gitlab.com/group/sub/project"), gitlab_repo("group/project")), ("group/sub/project".into(), "group/project".into()));
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
    let named = std::env::var("REVUI_LIVE").expect("set REVUI_LIVE to owner/repo#12 or group/project!12");
    let at = named.rfind(['#', '!']).unwrap();
    let (repo, reference) = (&named[..at], &named[at..]);
    let found = threads(repo, reference);
    println!("{} threads", found.len());
    for thread in found.iter().take(6) {
        let first = &thread.notes[0];
        println!("  {} {:?}:{:?} old={} resolved={} replies={} {} {:?}", thread.anchor, thread.file, thread.line, thread.old, thread.resolved, thread.notes.len() - 1, first.0, first.1.chars().take(50).collect::<String>());
    }
    println!("dates {:?}", dates(repo, reference));
    let host = if on_github(reference) { "github" } else { "" };
    let listed = fetch_all_from(repo, host);
    println!("{} open; first: {:?}", listed.len(), listed.first().map(|mr| (mr.reference(), mr.title.clone(), mr.sha().to_string())));
    if let Some(mr) = listed.first() {
        let base = base_sha(repo, &mr.reference(), mr.sha());
        println!("base of {} is {base:?}", mr.reference());
        let readme = raw_file(repo, on_github(reference), mr.sha(), "README.md").unwrap_or_default();
        println!("README.md at head starts {:?}", readme.lines().next());
    }
}

#[cfg(test)]
fn fetch_all_from(repo: &str, host: &str) -> Vec<Mr> {
    match github_repo(repo, host) {
        Some(repo) => {
            let found = gh(&["pr", "list", "-R", &repo, "--state", "open", "-L", "5", "--search", "sort:updated-desc", "--json", PR_FIELDS]).unwrap();
            serde_json::from_str::<Vec<serde_json::Value>>(&found).unwrap().iter().filter_map(|pr| from_github(pr, &repo)).collect()
        }
        None => serde_json::from_str(&glab(&["mr", "list", "-R", repo, "-F", "json", "-P", "5"]).unwrap()).unwrap(),
    }
}
