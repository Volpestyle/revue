use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct Review {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    /// What the generator is reviewing right now. Set while the guide is still being written, gone when it is whole.
    #[serde(default)]
    pub working: String,
    pub sources: Vec<Source>,
    pub sections: Vec<Section>,
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Deserialize)]
pub struct Source {
    pub id: String,
    pub head_sha: String,
    pub patch: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub target: String,
}

#[derive(Deserialize)]
pub struct Section {
    pub title: String,
    #[serde(default)]
    pub tier: String,
    pub why: String,
    pub hunks: Vec<HunkRef>,
}

#[derive(Deserialize)]
pub struct HunkRef {
    pub source: Option<String>,
    pub file: String,
    pub hunk: Option<usize>,
}

#[derive(Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Finding,
    Decision,
    Good,
}

#[derive(Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Confirmed,
    Unreproduced,
    Resolved,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    Observed,
    Checked,
    Inferred,
}

#[derive(Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    #[serde(default)]
    pub severity: String,
    pub section: Option<usize>,
    pub title: String,
    pub body: String,
    pub at: Option<At>,
    pub status: Option<Status>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

#[derive(Deserialize)]
pub struct At {
    pub file: String,
    pub line: u32,
}

#[derive(Deserialize)]
pub struct Evidence {
    pub provenance: Provenance,
    pub caption: String,
    pub path: Option<String>,
    pub text: Option<String>,
    pub repro: Option<String>,
}

pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<String>,
}

pub struct FileDiff {
    pub source: String,
    pub path: String,
    pub hunks: Vec<Hunk>,
}

pub struct Bundle {
    pub dir: PathBuf,
    pub review: Review,
    pub files: Vec<FileDiff>,
    pub groups: Vec<Vec<(usize, usize)>>,
    pub successor: Option<PathBuf>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct State {
    pub verdicts: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub finished: Option<Finished>,
    #[serde(default)]
    pub published: BTreeSet<String>,
    /// Per item, whether its text goes out as a quote above your comments. Unset follows the verdict.
    #[serde(default)]
    pub quotes: BTreeMap<String, bool>,
}

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct Note {
    pub text: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    #[serde(default)]
    pub start: Option<u32>,
    #[serde(default)]
    pub old: bool,
    pub item: Option<String>,
    pub reply_to: Option<String>,
    #[serde(default)]
    pub posted: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Finished {
    pub how: String,
    pub posted: bool,
}

fn parse_patch(source: &str, text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let path = rest.rsplit_once(" b/").map_or(rest, |(_, b)| b);
            files.push(FileDiff { source: source.into(), path: path.into(), hunks: Vec::new() });
        } else if let Some(file) = files.last_mut() {
            if line.starts_with("@@") {
                let new_start = line
                    .split_once('+')
                    .and_then(|(_, r)| r.split([',', ' ']).next())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                let old_start = line.split_once('-').and_then(|(_, r)| r.split([',', ' ']).next()).and_then(|n| n.parse().ok()).unwrap_or(0);
                file.hunks.push(Hunk { header: line.into(), old_start, new_start, lines: Vec::new() });
            } else if let Some(hunk) = file.hunks.last_mut() {
                hunk.lines.push(line.into());
            }
        }
    }
    for file in &mut files {
        if file.hunks.is_empty() {
            file.hunks.push(Hunk {
                header: "(no text diff: binary, rename or mode change)".into(),
                old_start: 0,
                new_start: 0,
                lines: Vec::new(),
            });
        }
    }
    files
}

impl Bundle {
    pub fn load(dir: &Path) -> Result<Bundle, String> {
        let read = |p: PathBuf| fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()));
        let review: Review =
            serde_json::from_str(&read(dir.join("review.json"))?).map_err(|e| format!("review.json: {e}"))?;
        let mut files = Vec::new();
        for source in &review.sources {
            files.extend(parse_patch(&source.id, &read(dir.join(&source.patch))?));
        }
        let mut groups = Vec::new();
        let mut seen = HashSet::new();
        for section in &review.sections {
            let mut group = Vec::new();
            for r in &section.hunks {
                let fi = files
                    .iter()
                    .position(|f| f.path == r.file && r.source.as_ref().is_none_or(|s| *s == f.source))
                    .ok_or(format!("section '{}' cites {}, which is not in the diff", section.title, r.file))?;
                let count = files[fi].hunks.len();
                let hunks = match r.hunk {
                    Some(h) if h >= count => {
                        return Err(format!(
                            "section '{}' cites hunk {h} of {}, which has {count}",
                            section.title, r.file
                        ));
                    }
                    Some(h) => h..h + 1,
                    None => 0..count,
                };
                for hi in hunks {
                    if seen.insert((fi, hi)) {
                        group.push((fi, hi));
                    }
                }
            }
            groups.push(group);
        }
        let mut ids = HashSet::new();
        if let Some(item) = review.items.iter().find(|i| !ids.insert(&i.id)) {
            return Err(format!("two items share the id {}", item.id));
        }
        if let Some(item) = review.items.iter().find(|i| i.section.is_some_and(|s| s >= review.sections.len())) {
            return Err(format!("item {} names a section that does not exist", item.id));
        }
        for (fi, file) in files.iter().enumerate() {
            let loose: Vec<_> = (0..file.hunks.len()).map(|hi| (fi, hi)).filter(|k| !seen.contains(k)).collect();
            if !loose.is_empty() {
                groups.push(loose);
            }
        }
        Ok(Bundle { dir: dir.into(), review, files, groups, successor: None })
    }

    pub fn has_unsectioned(&self) -> bool {
        self.groups.len() > self.review.sections.len()
    }

    pub fn hunks(&self) -> String {
        let mut out = String::new();
        for file in &self.files {
            out += &format!("{}\n", file.path);
            for (i, hunk) in file.hunks.iter().enumerate() {
                let count = |sign: char| hunk.lines.iter().filter(|l| l.starts_with(sign)).count();
                out += &format!("  {i}  +{} -{}  {}\n", count('+'), count('-'), hunk.header);
            }
        }
        out
    }

    pub fn check(&self) -> (bool, String) {
        let sections = self.review.sections.len();
        let mut problems = Vec::new();
        if !self.review.working.trim().is_empty() {
            problems.push("still being written: remove `working` once the last part is reviewed".to_string());
        }
        for &(fi, hi) in self.groups[sections..].iter().flatten() {
            problems.push(format!("no section: {} hunk {hi}", self.files[fi].path));
        }
        for item in &self.review.items {
            for path in item.evidence.iter().filter_map(|e| e.path.as_ref()) {
                if !self.dir.join(path).exists() {
                    problems.push(format!("missing evidence file for {}: {path}", item.id));
                }
            }
            let (Some(at), Some(section)) = (&item.at, item.section) else { continue };
            let lands = self.groups[section].iter().any(|&(fi, hi)| {
                let hunk = &self.files[fi].hunks[hi];
                let length = hunk.lines.iter().filter(|l| !l.starts_with(['-', '\\'])).count() as u32;
                self.files[fi].path == at.file && (hunk.new_start..hunk.new_start + length).contains(&at.line)
            });
            if !lands {
                problems.push(format!("{} points at {}:{}, which is in none of its section's hunks", item.id, at.file, at.line));
            }
        }
        let verdict = if problems.is_empty() { "ok" } else { "not ok" };
        let mut out = format!("{verdict}: {sections} sections, {} items\n", self.review.items.len());
        for problem in &problems {
            out += &format!("  {problem}\n");
        }
        (problems.is_empty(), out)
    }

    pub fn load_state(&self) -> State {
        fs::read_to_string(self.dir.join("state.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_state(&self, state: &State) -> std::io::Result<()> {
        fs::write(self.dir.join("state.json"), serde_json::to_string_pretty(state)?)
    }

    pub fn export(&self, state: &State) -> String {
        let heads: Vec<_> = self.review.sources.iter().map(|s| &s.head_sha[..s.head_sha.len().min(8)]).collect();
        let mut out = format!("Guided review of {} at {}\n\n", self.review.title, heads.join(", "));
        if self.has_unsectioned() {
            let loose: usize = self.groups[self.review.sections.len()..].iter().map(Vec::len).sum();
            out += &format!("{loose} hunks were in no section.\n\n");
        }
        for item in &self.review.items {
            let verdict = state.verdicts.get(&item.id).map_or("no verdict", String::as_str);
            out += &format!("- {} {}: {verdict}\n", item.id, item.title);
        }
        out
    }
}
