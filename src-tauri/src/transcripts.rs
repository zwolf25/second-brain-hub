//! Teams `.vtt` -> markdown, zero tokens. Rust port of the vtt-normalizer skill's
//! `normalize_vtt.py` (parse_cues / merge_cues / render), plus content-hash
//! duplicate detection and a `state.json` keyed by the source `.vtt` filename.

use crate::categorize::{self, Peer, Rules};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub ts: String,
    pub end_secs: u32,
    pub speaker: Option<String>,
    pub text: String,
}

/// "HH:MM:SS.mmm", "MM:SS.mmm" or with a comma -> ("HH:MM:SS", seconds).
fn parse_time(s: &str) -> Option<(String, u32)> {
    let s = s.trim().split(['.', ',']).next()?;
    let nums: Vec<u32> = s.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let (h, m, sec) = match nums.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        _ => return None,
    };
    Some((format!("{h:02}:{m:02}:{sec:02}"), h * 3600 + m * 60 + sec))
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").trim().to_string()
}

/// `<v Name>text</v>` / `<v.class Name>text` -> (Some(name), text).
fn split_voice(text: &str) -> (Option<String>, String) {
    let Some(rest) = text.strip_prefix("<v") else { return (None, text.to_string()) };
    if !rest.starts_with(|c: char| c.is_whitespace() || c == '.') {
        return (None, text.to_string());
    }
    let Some(close) = rest.find('>') else { return (None, text.to_string()) };
    let mut name = rest[..close].trim();
    if name.starts_with('.') {
        name = name.split_once(char::is_whitespace).map_or("", |(_, n)| n).trim();
    }
    let name = (!name.is_empty()).then(|| name.to_string());
    (name, rest[close + 1..].to_string())
}

/// Zoom-style `Name: text` lines; only accepted when most cues look that way.
fn split_colon_speaker(text: &str) -> Option<(String, String)> {
    let (name, rest) = text.split_once(": ")?;
    let ok = !name.is_empty() && name.len() <= 50 && !name.contains(['<', '>', ':']) && name.chars().any(|c| c.is_alphabetic());
    ok.then(|| (name.trim().to_string(), rest.trim().to_string()))
}

pub fn parse_cues(vtt: &str) -> Vec<Cue> {
    let vtt = vtt.strip_prefix('\u{feff}').unwrap_or(vtt).replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks: Vec<Vec<&str>> = vec![vec![]];
    for line in vtt.lines() {
        if line.trim().is_empty() {
            blocks.push(vec![]);
        } else {
            blocks.last_mut().unwrap().push(line);
        }
    }
    let mut cues = Vec::new();
    for block in blocks {
        let Some(i) = block.iter().position(|l| l.contains("-->")) else { continue };
        let (start, end) = block[i].split_once("-->").unwrap();
        let Some((ts, _)) = parse_time(start) else { continue };
        let end_secs = parse_time(end.split_whitespace().next().unwrap_or("")).map_or(0, |t| t.1);
        let raw = block[i + 1..].iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
        let (speaker, text) = split_voice(raw.trim());
        let text = strip_tags(&text);
        if !text.is_empty() {
            cues.push(Cue { ts, end_secs, speaker, text });
        }
    }
    if !cues.is_empty() && cues.iter().all(|c| c.speaker.is_none()) {
        let split: Vec<_> = cues.iter().map(|c| split_colon_speaker(&c.text)).collect();
        if split.iter().filter(|s| s.is_some()).count() * 2 >= cues.len() {
            for (c, s) in cues.iter_mut().zip(split) {
                if let Some((name, text)) = s {
                    c.speaker = Some(name);
                    c.text = text;
                }
            }
        }
    }
    cues
}

/// Joins consecutive cues from the same speaker; keeps the earliest timestamp.
pub fn merge_cues(cues: Vec<Cue>) -> Vec<Cue> {
    let mut merged: Vec<Cue> = Vec::new();
    for c in cues {
        match merged.last_mut() {
            Some(prev) if c.speaker.is_some() && prev.speaker == c.speaker => {
                prev.text.push(' ');
                prev.text.push_str(&c.text);
                prev.end_secs = c.end_secs;
            }
            _ => merged.push(c),
        }
    }
    merged
}

/// One "owner mentioned or speaking" line from the AI header.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Mention {
    pub ts: String,
    pub text: String,
}

/// AI header Teams/Felipe's fix prepends to a `.vtt`: summary, action items, owner mentions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Header {
    pub summary: Option<String>,
    pub actions: Vec<String>,
    pub mentions: Vec<Mention>,
    pub owner: Option<String>,
}

fn numbered_item(line: &str) -> Option<&str> {
    let (n, rest) = line.split_once(". ")?;
    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then_some(rest.trim())
}

/// Owner from the mentions heading: `(Wolf, Zac)` first, else the quoted name; "User name" is a placeholder.
fn header_owner(heading: &str) -> Option<String> {
    let inner = |open: char, close: char| {
        let a = heading.find(open)? + open.len_utf8();
        let b = a + heading[a..].find(close)?;
        Some(heading[a..b].trim().to_string())
    };
    [inner('(', ')'), inner('"', '"')]
        .into_iter()
        .flatten()
        .find(|n| !n.is_empty() && !n.eq_ignore_ascii_case("user name"))
}

/// Parses the AI header that precedes the first cue. `None` for old-style files (no
/// "High-Level Summary:") so they convert exactly as before. Never fails: unknown lines are ignored.
pub fn parse_header(vtt: &str) -> Option<Header> {
    let vtt = vtt.strip_prefix('\u{feff}').unwrap_or(vtt).replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = vtt.lines().collect();
    let first_cue = lines.iter().position(|l| l.contains("-->")).unwrap_or(lines.len());
    // Back up over a cue identifier line so it is not read as header text.
    let end = lines[..first_cue].iter().rposition(|l| l.trim().is_empty()).unwrap_or(0);
    // The header is written with literal backslash-n sequences as well as real newlines.
    let region = lines[..end].join("\n").replace("\\n", "\n");
    if !region.to_lowercase().contains("high-level summary:") {
        return None;
    }
    #[derive(PartialEq)]
    enum Sec { None, Summary, Actions, Mentions }
    let (mut sec, mut h, mut summary) = (Sec::None, Header::default(), Vec::<String>::new());
    for raw in region.lines() {
        let line = raw.trim();
        let low = line.to_lowercase();
        if line.is_empty() || low == "webvtt" || low == "note" {
            continue;
        }
        if low.starts_with("high-level summary") {
            sec = Sec::Summary;
            if let Some((_, rest)) = line.split_once(':') {
                summary.push(rest.trim().to_string());
            }
        } else if low.starts_with("key action") {
            sec = Sec::Actions;
        } else if low.starts_with("timestamp") && low.contains("mentioned or speaking") {
            sec = Sec::Mentions;
            h.owner = header_owner(line);
        } else if low.starts_with("if you need") || low.starts_with("feel free") {
            sec = Sec::None;
        } else {
            match sec {
                Sec::Summary => summary.push(line.to_string()),
                Sec::Actions => match numbered_item(line) {
                    Some(item) => h.actions.push(item.to_string()),
                    None => {
                        if let Some(last) = h.actions.last_mut() {
                            last.push(' ');
                            last.push_str(line);
                        }
                    }
                },
                Sec::Mentions => {
                    let item = line.trim_start_matches(['-', '*']).trim();
                    if line.len() == item.len() || low.contains("no direct speech") {
                        continue;
                    }
                    let (range, text) = item.split_once(": ").unwrap_or(("", item));
                    let ts = range.split_whitespace().next().and_then(parse_time).map(|t| t.0).unwrap_or_default();
                    h.mentions.push(Mention { ts, text: text.trim().to_string() });
                }
                Sec::None => {}
            }
        }
    }
    let summary = summary.join(" ").trim().to_string();
    h.summary = (!summary.is_empty()).then_some(summary);
    Some(h)
}

pub fn render(cues: &[Cue], source: &str, title: &str, date: Option<&str>, converted: &str, header: Option<&Header>) -> String {
    let kind = if header.is_some() { "transcript-ai" } else { "transcript" };
    let mut out = format!("---\nsource: {source}\ntype: {kind}\ntitle: {}\n", serde_json::to_string(title).unwrap());
    if let Some(d) = date {
        out.push_str(&format!("date: {d}\n"));
    }
    if let Some(o) = header.and_then(|h| h.owner.as_ref()) {
        out.push_str(&format!("owner: {}\n", serde_json::to_string(o).unwrap()));
    }
    out.push_str(&format!("converted: {converted}\n---\n\n# {title}\n"));
    if let Some(h) = header {
        if let Some(s) = &h.summary {
            out.push_str(&format!("\n## AI Summary\n\n{s}\n"));
        }
        if !h.actions.is_empty() {
            out.push_str("\n## Key Actions\n\n");
            for (i, a) in h.actions.iter().enumerate() {
                out.push_str(&format!("{}. {a}\n", i + 1));
            }
        }
        if !h.mentions.is_empty() {
            out.push_str("\n## Owner Mentions\n\n");
            for m in &h.mentions {
                let ts = if m.ts.is_empty() { String::new() } else { format!("[{}] ", m.ts) };
                out.push_str(&format!("- {ts}{}\n", m.text));
            }
        }
    }
    for c in cues {
        out.push_str(&format!("\n## [{}]\n\n", c.ts));
        if let Some(s) = &c.speaker {
            out.push_str(&format!("{s}:\n\n"));
        }
        out.push_str(&c.text);
        out.push('\n');
    }
    out
}

/// SHA-256 of speaker + cue text only: timestamps, whitespace, BOM and CRLF never matter.
pub fn content_hash(cues: &[Cue]) -> String {
    let mut h = Sha256::new();
    for c in cues {
        h.update(c.speaker.as_deref().unwrap_or("").as_bytes());
        h.update(b"\t");
        h.update(c.text.split_whitespace().collect::<Vec<_>>().join(" ").as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// `<title>_<YYYY-MM-DD>_Transcript.vtt` -> (display title, date). Teams' "Following-" prefix is dropped.
pub fn title_and_date(file_name: &str) -> (String, Option<String>) {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    let lower = stem.to_lowercase();
    let stem = if lower.ends_with("_transcript") { &stem[..stem.len() - "_transcript".len()] } else { stem };
    // Newer downloads carry a time too: `Title_2026-10-13_16-39_Transcript`.
    let stem = match stem.rsplit_once('_') {
        Some((t, hm)) if is_time(hm) && t.rsplit_once('_').is_some_and(|(_, d)| is_date(d)) => t,
        _ => stem,
    };
    let (stem, date) = match stem.rsplit_once('_') {
        Some((t, d)) if is_date(d) => (t, Some(d.to_string())),
        _ => (stem, None),
    };
    let t = stem.trim();
    let t = if t.to_lowercase().starts_with("following-") { &t[10..] } else { t };
    (t.split_whitespace().collect::<Vec<_>>().join(" "), date)
}

fn is_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5 && b.iter().enumerate().all(|(i, c)| if i == 2 { *c == b'-' } else { c.is_ascii_digit() })
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// Lowercase with Unicode dashes folded, for rule matching.
pub fn title_norm(title: &str) -> String {
    title.to_lowercase().replace(['\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}'], "-")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    pub title: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub md_path: Option<String>,
    pub vtt_mtime: u64,
    #[serde(default)]
    pub converted_at: Option<String>,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub duplicate_of: Option<String>,
    pub category: String,
    pub category_source: String,
    pub confidence: String,
    #[serde(default)]
    pub speakers: Vec<String>,
    #[serde(default)]
    pub duration_min: u32,
    #[serde(default)]
    pub words: u32,
    /// Off the default list (files untouched, still indexed). `hidden_reason` is "manual", "logged" or "auto".
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub hidden_reason: Option<String>,
    /// Sent to log-it. Set optimistically at launch (a deep link can't confirm completion).
    #[serde(default)]
    pub logged: bool,
    /// An auto "hide" rule already hid this row once; unhiding it sticks.
    #[serde(default)]
    pub auto_hid: bool,
    /// "manual" or "auto"; a moved file is never auto-moved again.
    #[serde(default)]
    pub moved_by: Option<String>,
    /// `duplicate_of` was set by the near-duplicate guard (>= NEAR_DUP_MIN shingle overlap), not an exact hash match.
    #[serde(default)]
    pub near: bool,
    /// Sent to `voc-library-ingest`. Set at launch (a deep link can't confirm); never hides the row.
    #[serde(default)]
    pub voc_ingested: bool,
    /// "Don't process as VoC": excluded from any automatic VoC run.
    #[serde(default)]
    pub voc_skip: bool,
    /// AI header (summary / actions / owner mentions); `None` for files without one.
    #[serde(default)]
    pub header: Option<Header>,
    /// Source was checked for an AI header; entries from before the header existed re-scan once.
    #[serde(default)]
    pub header_checked: bool,
    /// Raw-note stub in the personal `raw/` that points at this transcript for `/wiki-builder`.
    #[serde(default)]
    pub raw_note: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub entries: BTreeMap<String, Entry>,
    /// Manual per-series corrections: normalized title -> category. Applied to later occurrences.
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,
}

fn state_file(out: &Path) -> PathBuf {
    out.join("state.json")
}

/// Category renames: stored names are migrated on load so old state follows the rules file.
const RENAMED: &[(&str, &str)] = &[("PD / Policy Deployment Meetings", "PD / Strat Meetings")];

pub fn load_state(out: &Path) -> State {
    let mut st: State = std::fs::read_to_string(state_file(out)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let fix = |c: &mut String| {
        if let Some((_, new)) = RENAMED.iter().find(|(old, _)| old == c) {
            *c = new.to_string();
        }
    };
    st.entries.values_mut().for_each(|e| fix(&mut e.category));
    st.overrides.values_mut().for_each(fix);
    st
}

fn save_state(out: &Path, state: &State) -> Result<()> {
    let tmp = out.join("state.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(state)?)?;
    std::fs::rename(&tmp, state_file(out)).context("writing state.json")
}

pub fn today_utc() -> String {
    let days = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86400) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn mtime_secs(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ScanReport {
    pub processed: usize,
    pub converted: usize,
    pub skipped_unreadable: usize,
    /// Newly categorized transcripts that fell through to `Other` (feeds the review loop).
    #[serde(skip)]
    pub auto_other: Vec<Entry>,
}

fn vtt_files(src: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(src)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("vtt")))
        .collect();
    // Date-ascending so earlier occurrences categorize first and seed series inheritance.
    v.sort_by_key(|p| {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        (title_and_date(&name).1.unwrap_or_else(|| "9999".into()), name)
    });
    v
}

/// Speakers who appear in >= 3 distinct series are treated as internal (local only).
fn internal_names(state: &State, voc_category: &str) -> HashSet<String> {
    let mut series: HashMap<&str, HashSet<String>> = HashMap::new();
    for e in state.entries.values().filter(|e| e.category != voc_category) {
        for s in &e.speakers {
            series.entry(s).or_default().insert(title_norm(&e.title));
        }
    }
    series.into_iter().filter(|(_, t)| t.len() >= 3).map(|(s, _)| s.to_string()).collect()
}

fn process_one(path: &Path, name: &str, mtime: u64, rules: &Rules, state: &State, internal: &HashSet<String>) -> Result<Entry> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let cues = merge_cues(parse_cues(&text));
    let (title, date) = title_and_date(name);
    let tn = title_norm(&title);

    let mut speakers: Vec<String> = Vec::new();
    let mut words_by: Vec<(String, u32)> = Vec::new();
    let mut words = 0u32;
    for c in &cues {
        let n = c.text.split_whitespace().count() as u32;
        words += n;
        if let Some(s) = &c.speaker {
            match words_by.iter_mut().find(|(k, _)| k == s) {
                Some(w) => w.1 += n,
                None => {
                    speakers.push(s.clone());
                    words_by.push((s.clone(), n));
                }
            }
        }
    }
    let body_lower = cues.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ").to_lowercase();
    let others: Vec<(&Entry, String)> =
        state.entries.iter().filter(|(k, _)| k.as_str() != name).map(|(_, e)| (e, title_norm(&e.title))).collect();
    let peers: Vec<Peer> = others
        .iter()
        .map(|(e, n)| Peer { title_norm: n, speakers: &e.speakers, category: &e.category, source: &e.category_source })
        .collect();

    let v = categorize::categorize(
        rules,
        &categorize::Input {
            title_norm: &tn,
            speakers: &speakers,
            speaker_words: &words_by,
            body_lower: &body_lower,
            words,
            override_category: state.overrides.get(&tn).map(String::as_str),
        },
        &peers,
        internal,
    );
    Ok(Entry {
        title,
        date,
        md_path: None,
        vtt_mtime: mtime,
        converted_at: None,
        content_hash: if cues.is_empty() { String::new() } else { content_hash(&cues) },
        duplicate_of: None,
        category: v.category,
        category_source: v.source,
        confidence: v.confidence.to_string(),
        speakers,
        duration_min: cues.iter().map(|c| c.end_secs).max().unwrap_or(0).div_ceil(60),
        words,
        header: parse_header(&text),
        header_checked: true,
        ..Default::default()
    })
}

/// Earliest date wins as the canonical copy of identical content; the rest point at it.
fn assign_duplicates(state: &mut State) {
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for (name, e) in &state.entries {
        if !e.content_hash.is_empty() {
            groups.entry(e.content_hash.clone()).or_default().push(name.clone());
        }
    }
    for (_, mut names) in groups {
        names.sort_by_key(|n| (state.entries[n].date.clone().unwrap_or_else(|| "9999".into()), n.clone()));
        let canonical = names[0].clone();
        for (i, n) in names.iter().enumerate() {
            let e = state.entries.get_mut(n).unwrap();
            (e.duplicate_of, e.near) = ((i > 0).then(|| canonical.clone()), false);
        }
    }
}

/// Trivially different re-exports (same speakers, ~same length) that overlap this much count as duplicates.
const NEAR_DUP_MIN: f64 = 0.98;

fn shingles(text: &str) -> HashSet<u64> {
    use std::hash::{Hash, Hasher};
    let words: Vec<String> = merge_cues(parse_cues(text)).iter().flat_map(|c| c.text.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>()).collect();
    words
        .windows(5)
        .map(|w| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            w.hash(&mut h);
            h.finish()
        })
        .collect()
}

/// Second pass after the exact-hash grouping: a later canonical file with the same speaker set, a word count
/// within 2% and >= NEAR_DUP_MIN shingle Jaccard against an earlier canonical file points at it.
/// Files are read only for candidate pairs, so a quiet folder costs nothing.
// ponytail: O(n^2) over canonical entries with a cheap prefilter; add a sketch to state.json if folders reach thousands.
fn assign_near_duplicates(state: &mut State, src: &Path) {
    let mut canon: Vec<String> = state.entries.iter().filter(|(_, e)| e.duplicate_of.is_none() && !e.content_hash.is_empty()).map(|(n, _)| n.clone()).collect();
    canon.sort_by_key(|n| (state.entries[n].date.clone().unwrap_or_else(|| "9999".into()), n.clone()));
    let key = |e: &Entry| {
        let mut sp = e.speakers.clone();
        sp.sort();
        sp
    };
    let mut cache: HashMap<String, HashSet<u64>> = HashMap::new();
    let mut keepers: Vec<String> = Vec::new();
    for n in canon {
        let e = &state.entries[&n];
        let hit = keepers.iter().find(|k| {
            let o = &state.entries[*k];
            key(o) == key(e) && o.words.abs_diff(e.words) as f64 <= 0.02 * o.words.max(e.words) as f64
        });
        let mut found = None;
        if let Some(k) = hit.cloned() {
            for name in [&k, &n] {
                if !cache.contains_key(name) {
                    let Ok(t) = std::fs::read_to_string(src.join(name)) else { continue };
                    cache.insert(name.clone(), shingles(&t));
                }
            }
            if let (Some(a), Some(b)) = (cache.get(&k), cache.get(&n)) {
                let union = a.union(b).count();
                if union > 0 && a.intersection(b).count() as f64 / union as f64 >= NEAR_DUP_MIN {
                    found = Some(k);
                }
            }
        }
        match found {
            Some(k) => {
                let e = state.entries.get_mut(&n).unwrap();
                (e.duplicate_of, e.near) = (Some(k), true);
            }
            None => keepers.push(n),
        }
    }
}

fn write_index(out: &Path, state: &State) -> Result<()> {
    let mut rows: Vec<(&String, &Entry)> = state.entries.iter().filter(|(_, e)| e.duplicate_of.is_none()).collect();
    rows.sort_by(|a, b| b.1.date.cmp(&a.1.date).then(a.0.cmp(b.0)));
    let mut md = String::from("# Transcript index\n\n| Date | Title | Category | Speakers | Min | Words | File |\n|---|---|---|---|---|---|---|\n");
    for (_, e) in rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            e.date.as_deref().unwrap_or("unknown"),
            e.title.replace('|', "/"),
            e.category,
            e.speakers.len(),
            e.duration_min,
            e.words,
            e.md_path.as_deref().and_then(|p| Path::new(p).file_name()).map_or(String::new(), |f| f.to_string_lossy().to_string()),
        ));
    }
    let dups: Vec<_> = state.entries.iter().filter(|(_, e)| e.duplicate_of.is_some()).collect();
    if !dups.is_empty() {
        md.push_str("\n## Duplicates (identical content)\n\n");
        for (n, e) in dups {
            md.push_str(&format!("- {n} = {}\n", e.duplicate_of.as_ref().unwrap()));
        }
    }
    std::fs::write(out.join("index.md"), md).context("writing index.md")
}

/// One full pass: process new/changed `.vtt`, assign duplicates, convert canonical files
/// that have no `.md` yet, write `state.json` + `index.md`. Safe to re-run (a no-op when idle).
pub fn scan(src: &Path, out: &Path, rules: &Rules) -> Result<ScanReport> {
    std::fs::create_dir_all(out).context("creating transcripts output dir")?;
    let mut state = load_state(out);
    let mut report = ScanReport::default();
    let internal = internal_names(&state, &rules.voc_category);
    let mut reconvert: HashSet<String> = HashSet::new();

    for path in vtt_files(src) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let mtime = mtime_secs(&path);
        // Unchanged source, but state predates header parsing or the `_HH-MM` filename fix.
        let migrating = state.entries.get(&name).is_some_and(|e| {
            e.vtt_mtime >= mtime && (!e.header_checked || title_and_date(&name) != (e.title.clone(), e.date.clone()))
        });
        if !migrating && state.entries.get(&name).is_some_and(|e| e.vtt_mtime >= mtime) {
            continue;
        }
        // OneDrive cloud-only placeholders can fail to read; skip and retry next scan.
        match process_one(&path, &name, mtime, rules, &state, &internal) {
            Ok(mut entry) => {
                // Changed source: keep md_path so the same file is rewritten, but force reconversion below.
                if let Some(old) = state.entries.get(&name) {
                    entry.md_path = old.md_path.clone();
                    (entry.hidden, entry.hidden_reason, entry.logged, entry.moved_by, entry.auto_hid) = (old.hidden, old.hidden_reason.clone(), old.logged, old.moved_by.clone(), old.auto_hid);
                    (entry.voc_ingested, entry.voc_skip) = (old.voc_ingested, old.voc_skip);
                    if !migrating || entry.header.is_some() || old.title != entry.title {
                        reconvert.insert(name.clone());
                    }
                }
                if entry.category == "Other" && entry.category_source == "fallback" {
                    report.auto_other.push(entry.clone());
                }
                state.entries.insert(name, entry);
                report.processed += 1;
            }
            Err(e) => {
                tracing::warn!("transcript {name}: {e:#}");
                report.skipped_unreadable += 1;
            }
        }
    }
    assign_duplicates(&mut state);
    assign_near_duplicates(&mut state, src);

    let converted = today_utc();
    let names: Vec<String> = state.entries.keys().cloned().collect();
    for name in names {
        let e = &state.entries[&name];
        if e.duplicate_of.is_some() || (e.md_path.is_some() && !reconvert.contains(&name)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(src.join(&name)) else { continue };
        let cues = merge_cues(parse_cues(&text));
        let md = e.md_path.as_ref().map(PathBuf::from).unwrap_or_else(|| out.join(format!("{}.md", name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s))));
        std::fs::write(&md, render(&cues, &name, &e.title, e.date.as_deref(), &converted, e.header.as_ref()))?;
        let e = state.entries.get_mut(&name).unwrap();
        e.md_path = Some(md.to_string_lossy().to_string());
        e.converted_at = Some(converted.clone());
        report.converted += 1;
    }
    save_state(out, &state)?;
    write_index(out, &state)?;
    Ok(report)
}

/// What a manual category change replaced, for the audit log.
pub struct Change {
    pub entry: Entry,
    pub from: String,
    pub auto_source: String,
}

/// Sets `category` on each transcript (source `manual`) and remembers it for the whole series.
pub fn set_category(out: &Path, vtts: &[String], category: &str) -> Result<Vec<Change>> {
    let mut state = load_state(out);
    let mut changes = Vec::new();
    for v in vtts {
        let Some(e) = state.entries.get_mut(v) else { continue };
        let (from, auto_source) = (e.category.clone(), e.category_source.clone());
        e.category = category.to_string();
        e.category_source = "manual".into();
        e.confidence = "high".into();
        let tn = title_norm(&e.title);
        changes.push(Change { entry: e.clone(), from, auto_source });
        state.overrides.insert(tn, category.to_string());
    }
    save_state(out, &state)?;
    write_index(out, &state)?;
    Ok(changes)
}

/// Hide or unhide. Unhiding a logged transcript clears `logged` so it is eligible for log-it again.
pub fn set_hidden(out: &Path, vtts: &[String], hidden: bool) -> Result<()> {
    let mut state = load_state(out);
    for e in state.entries.iter_mut().filter(|(k, _)| vtts.contains(k)).map(|(_, e)| e) {
        e.hidden = hidden;
        e.hidden_reason = hidden.then(|| "manual".to_string());
        if !hidden {
            e.logged = false;
        }
    }
    save_state(out, &state)
}

/// Marks transcripts as sent to VoC (badge only; they stay visible and eligible for log-it).
pub fn mark_voc(out: &Path, vtts: &[String]) -> Result<()> {
    let mut state = load_state(out);
    state.entries.iter_mut().filter(|(k, _)| vtts.contains(k)).for_each(|(_, e)| e.voc_ingested = true);
    save_state(out, &state)
}

pub fn set_voc_skip(out: &Path, vtts: &[String], skip: bool) -> Result<()> {
    let mut state = load_state(out);
    state.entries.iter_mut().filter(|(k, _)| vtts.contains(k)).for_each(|(_, e)| e.voc_skip = skip);
    save_state(out, &state)
}

#[derive(Debug, Default, Serialize)]
pub struct MoveReport {
    pub moved: usize,
    pub skipped: usize,
    /// One message per unusable destination (missing folder), for the page banner.
    pub errors: Vec<String>,
}

/// Moves one file into `dest`, never overwriting (`name-2.md`, `name-3.md`...). A failed copy leaves the source alone.
fn move_file(from: &Path, dest: &Path) -> Result<PathBuf> {
    let stem = from.file_stem().map_or(String::new(), |s| s.to_string_lossy().to_string());
    let ext = from.extension().map_or(String::new(), |s| format!(".{}", s.to_string_lossy()));
    let mut to = dest.join(format!("{stem}{ext}"));
    for n in 2.. {
        if !to.exists() {
            break;
        }
        to = dest.join(format!("{stem}-{n}{ext}"));
    }
    if std::fs::rename(from, &to).is_err() {
        // Across volumes (or OneDrive): copy, verify, then remove the source.
        let bytes = std::fs::copy(from, &to).context("copying to destination")?;
        if bytes != std::fs::metadata(from)?.len() {
            let _ = std::fs::remove_file(&to);
            anyhow::bail!("copy was short; source kept");
        }
        std::fs::remove_file(from).context("removing source after copy")?;
    }
    Ok(to)
}

/// `plan` names the destination for each entry to move (None = leave it). Duplicates and missing files never move.
fn move_entries(out: &Path, by: &str, plan: impl Fn(&str, &Entry) -> Option<PathBuf>) -> Result<MoveReport> {
    let mut state = load_state(out);
    let mut report = MoveReport::default();
    for (name, e) in state.entries.iter_mut() {
        let Some(md) = e.md_path.clone().filter(|p| Path::new(p).is_file()) else { continue };
        if e.duplicate_of.is_some() {
            continue;
        }
        let Some(dest) = plan(name, e) else { continue };
        if Path::new(&md).parent() == Some(dest.as_path()) {
            continue;
        }
        if !dest.is_dir() {
            let msg = format!("Destination folder is missing: {}", dest.display());
            if !report.errors.contains(&msg) {
                report.errors.push(msg);
            }
            report.skipped += 1;
            continue;
        }
        match move_file(Path::new(&md), &dest) {
            Ok(to) => {
                e.md_path = Some(to.to_string_lossy().to_string());
                e.moved_by = Some(by.into());
                if let Some(stub) = &e.raw_note {
                    patch_stub_path(stub, &to);
                }
                report.moved += 1;
            }
            Err(err) => {
                tracing::warn!("move {name}: {err:#}");
                report.skipped += 1;
            }
        }
    }
    save_state(out, &state)?;
    write_index(out, &state)?;
    Ok(report)
}

/// Manual move: any converted, non-duplicate transcript, even one moved before.
pub fn move_selected(out: &Path, vtts: &[String], dest: &Path) -> Result<MoveReport> {
    anyhow::ensure!(dest.is_dir(), "Destination folder is missing: {}", dest.display());
    move_entries(out, "manual", |n, _| vtts.iter().any(|v| v == n).then(|| dest.to_path_buf()))
}

fn in_scope(rule: &crate::config::AutoRule, e: &Entry, today: u64) -> bool {
    let listed = rule.categories.contains(&e.category);
    let in_scope = if rule.mode == "exclude" { !listed } else { listed };
    // "hide" ages by the meeting date, so history converted today is still old; other rules wait after conversion.
    let aged_from = if rule.action == "hide" { e.date.as_deref().or(e.converted_at.as_deref()) } else { e.converted_at.as_deref() };
    let old_enough = aged_from.is_some_and(|d| today >= crate::audit::days_since_epoch(d) + rule.after_days);
    let since = rule.since.as_deref().is_none_or(|s| e.converted_at.as_deref().is_some_and(|d| d >= s));
    in_scope && old_enough && since
}

/// Index of the rule that claims `e`: the first enabled move rule that matches.
/// `preview` treats that rule as enabled, for the dry-run on a rule not yet switched on.
fn claiming_rule(rules: &[crate::config::AutoRule], e: &Entry, today: u64, preview: Option<usize>) -> Option<usize> {
    rules
        .iter()
        .enumerate()
        .find(|(i, r)| r.action == "move" && (r.enabled || preview == Some(*i)) && e.moved_by.is_none() && in_scope(r, e, today))
        .map(|(i, _)| i)
}

/// Files `rules[idx]` would move now (the dry-run count shown before saving); earlier rules win overlaps.
pub fn auto_move_preview(out: &Path, rules: &[crate::config::AutoRule], idx: usize) -> usize {
    let today = crate::audit::days_since_epoch(&today_utc());
    let action = rules.get(idx).map_or("", |r| r.action.as_str());
    load_state(out)
        .entries
        .values()
        .filter(|e| e.duplicate_of.is_none() && e.md_path.as_deref().is_some_and(|p| Path::new(p).is_file()))
        .filter(|e| match action {
            "log-it" => !e.logged && !e.hidden && in_scope(&rules[idx], e, today),
            "voc" => voc_eligible(&rules[idx], e, today),
            "hide" => !e.hidden && !e.auto_hid && in_scope(&rules[idx], e, today),
            _ => claiming_rule(rules, e, today, Some(idx)) == Some(idx),
        })
        .count()
}

/// Runs at the end of each scan. A manually moved file (`moved_by` set) is never touched again,
/// and a later category edit never moves a file back.
pub fn auto_move(out: &Path, rules: &[crate::config::AutoRule]) -> Result<MoveReport> {
    let today = crate::audit::days_since_epoch(&today_utc());
    move_entries(out, "auto", |_, e| claiming_rule(rules, e, today, None).and_then(|i| rules[i].dest.clone()))
}

/// Where the Hub leaves `voc-queue.json` for an ad hoc `/voc-library-ingest` (same relative path on macOS and Windows).
pub fn bridge_dir() -> Option<PathBuf> {
    crate::config::dirs_home().map(|h| h.join(".claude/second-brain-hub/transcripts"))
}

#[derive(Debug, Serialize)]
struct QueueItem<'a> {
    vtt: &'a str,
    path: &'a str,
    title: &'a str,
    date: &'a Option<String>,
    category: &'a str,
}

/// Characters Windows or OneDrive reject in a file name.
fn safe_name(s: &str) -> String {
    s.chars().take(80).map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '-' } else { c }).collect::<String>().trim().to_string()
}

/// Reference note for `/wiki-builder`: frontmatter pointing at the converted transcript, plus the AI header's summary and actions.
fn stub_text(vtt: &str, e: &Entry, md: &str, date: &str) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default(); // JSON strings are valid YAML
    let mut t = format!(
        "---\ndate: {date}\ntype: transcript-ref\ntags: [transcript-ref]\nsource-transcript: {}\nsource-vtt: {}\ncategory: {}\n---\n\n# {} (meeting transcript)\n\n> Reference note. The full text is at `source-transcript`; /wiki-builder reads it.\n",
        q(md),
        q(vtt),
        q(&e.category),
        e.title
    );
    if let Some(h) = &e.header {
        if let Some(s) = h.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            t += &format!("\n## Summary\n{}\n", s.trim());
        }
        if !h.actions.is_empty() {
            t += "\n## Action items\n";
            h.actions.iter().for_each(|a| t += &format!("- {a}\n"));
        }
    }
    t
}

/// Writes the stub into `raw`, never overwriting: a taken name gets the content-hash suffix, then a counter.
fn write_stub(raw: &Path, vtt: &str, e: &Entry, md: &str) -> Result<PathBuf> {
    use std::io::Write;
    let date = e.date.clone().or_else(|| e.converted_at.clone()).unwrap_or_else(today_utc);
    let base = format!("{date} - {} (transcript)", safe_name(&e.title));
    let text = stub_text(vtt, e, md, &date);
    let hash: String = e.content_hash.chars().take(6).collect();
    let mut n = 0u32;
    loop {
        let file = match n {
            0 => format!("{base}.md"),
            1 => format!("{base} {hash}.md"),
            _ => format!("{base} {hash}-{n}.md"),
        };
        let to = raw.join(file);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&to) {
            Ok(mut f) => {
                f.write_all(text.as_bytes()).context("writing raw note")?;
                return Ok(to);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(err) => return Err(err).context("creating raw note"),
        }
    }
}

/// Keeps a stub's `source-transcript:` line in step with a moved transcript.
fn patch_stub_path(stub: &str, md: &Path) {
    let Ok(text) = std::fs::read_to_string(stub) else { return };
    let line = format!("source-transcript: {}", serde_json::to_string(&md.to_string_lossy()).unwrap_or_default());
    let lines: Vec<&str> = text.lines().map(|l| if l.starts_with("source-transcript:") { line.as_str() } else { l }).collect();
    let _ = std::fs::write(stub, lines.join("\n") + "\n");
}

/// Writes a raw-note stub for every entry `wanted` accepts and marks it logged (which hides the row).
/// Returns (written, skipped). Duplicates, unconverted rows and entries whose stub still exists are skipped.
fn stub_entries(out: &Path, raw: &Path, wanted: impl Fn(&str, &Entry) -> bool) -> Result<(usize, usize)> {
    anyhow::ensure!(raw.is_dir(), "Raw folder is missing: {}", raw.display());
    let mut state = load_state(out);
    let (mut written, mut skipped) = (0, 0);
    for (name, e) in state.entries.iter_mut().filter(|(n, e)| wanted(n, e)) {
        let md = e.md_path.clone().filter(|p| Path::new(p).is_file());
        if e.duplicate_of.is_some() || md.is_none() || e.raw_note.as_deref().is_some_and(|p| Path::new(p).is_file()) {
            skipped += 1;
            continue;
        }
        match write_stub(raw, name, e, &md.unwrap_or_default()) {
            Ok(to) => {
                e.raw_note = Some(to.to_string_lossy().to_string());
                (e.logged, e.hidden, e.hidden_reason) = (true, true, Some("logged".into()));
                written += 1;
            }
            Err(err) => {
                tracing::warn!("raw note for {name}: {err:#}");
                skipped += 1;
            }
        }
    }
    if written > 0 {
        save_state(out, &state)?;
    }
    Ok((written, skipped))
}

/// Manual "Add to Second Brain" for the chosen transcripts.
pub fn add_to_raw(out: &Path, raw: &Path, vtts: &[String]) -> Result<(usize, usize)> {
    stub_entries(out, raw, |n, _| vtts.iter().any(|v| v == n))
}

/// Auto rules: visible, not-yet-added transcripts that an enabled "log-it" rule covers. Runs after auto-move so the stub
/// records the final path; a later move patches it. Returns the number written.
pub fn auto_add_to_raw(out: &Path, raw: &Path, rules: &[crate::config::AutoRule]) -> Result<usize> {
    let today = crate::audit::days_since_epoch(&today_utc());
    let stub_rules: Vec<_> = rules.iter().filter(|r| r.action == "log-it" && r.enabled).collect();
    if stub_rules.is_empty() {
        return Ok(0);
    }
    stub_entries(out, raw, |_, e| !e.logged && !e.hidden && e.raw_note.is_none() && stub_rules.iter().any(|r| in_scope(r, e, today))).map(|(w, _)| w)
}

/// Auto rules: hides visible transcripts an enabled "hide" rule covers (meeting dated `after_days` ago or more). Each row is
/// auto-hidden once (`auto_hid`), so a later Unhide sticks. Runs last in a scan, after stubs, so nothing is hidden before
/// an "Add to Second Brain" rule has seen it. Returns the number hidden.
pub fn auto_hide(out: &Path, rules: &[crate::config::AutoRule]) -> Result<usize> {
    let today = crate::audit::days_since_epoch(&today_utc());
    let hide_rules: Vec<_> = rules.iter().filter(|r| r.action == "hide" && r.enabled).collect();
    if hide_rules.is_empty() {
        return Ok(0);
    }
    let mut state = load_state(out);
    let mut n = 0;
    for e in state.entries.values_mut().filter(|e| !e.hidden && !e.auto_hid && hide_rules.iter().any(|r| in_scope(r, e, today))) {
        (e.hidden, e.hidden_reason, e.auto_hid) = (true, Some("auto".into()), true);
        n += 1;
    }
    if n > 0 {
        save_state(out, &state)?;
    }
    Ok(n)
}

/// VoC runs only on rows the categorizer filed under a rule's categories, never `Other`, and never ones the user
/// skipped or already sent. Hidden/logged state is irrelevant: VoC and log-it are independent.
fn voc_eligible(rule: &crate::config::AutoRule, e: &Entry, today: u64) -> bool {
    !e.voc_ingested && !e.voc_skip && e.category != "Other" && in_scope(rule, e, today)
}

/// Writes `voc-queue.json`, same shape as the log-it queue plus the per-run cap the skill must honor.
pub fn write_voc_queue(out: &Path, rules: &[crate::config::AutoRule], bridge: &Path) -> Result<usize> {
    let today = crate::audit::days_since_epoch(&today_utc());
    let voc_rules: Vec<_> = rules.iter().filter(|r| r.action == "voc" && r.enabled).collect();
    write_queue(out, bridge, "voc-queue.json", |e| voc_rules.iter().any(|r| voc_eligible(r, e, today)), Some(VOC_MAX_PER_RUN))
}

pub const VOC_MAX_PER_RUN: usize = 5;

/// Rows an enabled VoC rule has flagged (first `VOC_MAX_PER_RUN`); the Transcripts page offers them in one click.
pub fn voc_ready(out: &Path, rules: &[crate::config::AutoRule]) -> Vec<String> {
    let today = crate::audit::days_since_epoch(&today_utc());
    let voc_rules: Vec<_> = rules.iter().filter(|r| r.action == "voc" && r.enabled).collect();
    load_state(out)
        .entries
        .iter()
        .filter(|(_, e)| e.duplicate_of.is_none() && e.md_path.as_deref().is_some_and(|p| Path::new(p).is_file()) && voc_rules.iter().any(|r| voc_eligible(r, e, today)))
        .map(|(n, _)| n.clone())
        .take(VOC_MAX_PER_RUN)
        .collect()
}

fn write_queue(out: &Path, bridge: &Path, file: &str, wanted: impl Fn(&Entry) -> bool, max_per_run: Option<usize>) -> Result<usize> {
    let state = load_state(out);
    let items: Vec<QueueItem> = state
        .entries
        .iter()
        .filter(|(_, e)| e.duplicate_of.is_none() && e.md_path.as_deref().is_some_and(|p| Path::new(p).is_file()) && wanted(e))
        .map(|(name, e)| QueueItem { vtt: name, path: e.md_path.as_deref().unwrap_or(""), title: &e.title, date: &e.date, category: &e.category })
        .collect();
    std::fs::create_dir_all(bridge).context("creating bridge dir")?;
    let tmp = bridge.join(format!("{file}.tmp"));
    let mut doc = serde_json::json!({ "generated": today_utc(), "items": items });
    if let Some(m) = max_per_run {
        doc["max_per_run"] = m.into();
    }
    std::fs::write(&tmp, serde_json::to_string_pretty(&doc)?)?;
    std::fs::rename(&tmp, bridge.join(file)).with_context(|| format!("writing {file}"))?;
    Ok(items.len())
}

/// What the Transcripts page shows per row.
#[derive(Debug, Serialize)]
pub struct Row {
    pub vtt: String,
    pub title: String,
    pub date: Option<String>,
    pub category: String,
    pub category_source: String,
    pub confidence: String,
    pub speakers: usize,
    pub duration_min: u32,
    pub words: u32,
    pub duplicate_of: Option<String>,
    pub near_duplicate: bool,
    /// The `.md` a double-click opens: own file, or the canonical copy for a duplicate.
    pub open_path: Option<String>,
    /// False when the `.md` was deleted or moved outside the Hub; the UI says so instead of erroring.
    pub md_exists: bool,
    pub hidden: bool,
    pub hidden_reason: Option<String>,
    pub voc_ingested: bool,
    pub voc_skip: bool,
    pub header: Option<Header>,
}

pub fn rows(out: &Path) -> Vec<Row> {
    let state = load_state(out);
    let mut rows: Vec<Row> = state
        .entries
        .iter()
        .map(|(name, e)| {
            let open = match &e.duplicate_of {
                Some(orig) => state.entries.get(orig).and_then(|o| o.md_path.clone()),
                None => e.md_path.clone(),
            };
            Row {
                md_exists: open.as_deref().is_some_and(|p| Path::new(p).is_file()),
                vtt: name.clone(),
                title: e.title.clone(),
                date: e.date.clone(),
                category: e.category.clone(),
                category_source: e.category_source.clone(),
                confidence: e.confidence.clone(),
                speakers: e.speakers.len(),
                duration_min: e.duration_min,
                words: e.words,
                duplicate_of: e.duplicate_of.clone(),
                near_duplicate: e.near,
                open_path: open,
                hidden: e.hidden,
                hidden_reason: e.hidden_reason.clone(),
                voc_ingested: e.voc_ingested,
                voc_skip: e.voc_skip,
                header: e.header.clone(),
            }
        })
        .collect();
    rows.sort_by(|a, b| b.date.cmp(&a.date).then(a.vtt.cmp(&b.vtt)));
    rows
}

/// Scans OneDrive roots (macOS CloudStorage, Windows profile dir) for a folder holding
/// `*_Transcript.vtt` files. Matches on contents, not folder name (the real one is misspelled).
pub fn autodetect_folder() -> Option<PathBuf> {
    let home = crate::config::dirs_home()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for root in [home.join("Library/CloudStorage"), home] {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for od in entries.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase().contains("onedrive")) {
            for dir in std::fs::read_dir(od.path()).into_iter().flatten().flatten().filter(|d| d.path().is_dir()) {
                let n = std::fs::read_dir(dir.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|f| f.file_name().to_string_lossy().to_lowercase().ends_with("_transcript.vtt"))
                    .count();
                if n > 0 && best.as_ref().is_none_or(|(b, _)| n > *b) {
                    best = Some((n, dir.path()));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests-data").join(name)).unwrap()
    }

    // Golden tests shared with the Python vtt-normalizer (same three fixtures, same assertions).
    #[test]
    fn simple() {
        let cues = merge_cues(parse_cues(&fixture("simple.vtt")));
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].ts, "00:00:04");
        assert_eq!(cues[0].speaker.as_deref(), Some("Brust, Allison"));
        assert!(cues[0].text.starts_with("Before we jump into going over VOC"));
    }

    #[test]
    fn different_speakers_do_not_merge() {
        let cues = merge_cues(parse_cues(&fixture("speakers.vtt")));
        let who: Vec<_> = cues.iter().map(|c| c.speaker.clone().unwrap()).collect();
        assert_eq!(who, ["Stumph, Catie", "Brust, Allison", "Stumph, Catie"]);
    }

    #[test]
    fn same_speaker_continuation() {
        let cues = merge_cues(parse_cues(&fixture("same-speaker-continuation.vtt")));
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].speaker.as_deref(), Some("Baldwin, Megan"));
        assert_eq!(cues[0].ts, "00:00:01");
        assert_eq!(cues[0].text, "This is part one of a thought that continues right into part two.");
        assert_eq!(cues[1].speaker.as_deref(), Some("Dahl, Leif"));
        assert!(cues[2].text.starts_with("And this part is interrupted"));
    }

    #[test]
    fn bom_crlf_short_timestamps_and_zoom_speakers() {
        let vtt = "\u{feff}WEBVTT\r\n\r\n01:05.500 --> 01:09.000\r\n<v Ann>Hi there</v>\r\n\r\n";
        let c = parse_cues(vtt);
        assert_eq!((c[0].ts.as_str(), c[0].speaker.as_deref(), c[0].text.as_str()), ("00:01:05", Some("Ann"), "Hi there"));
        let zoom = "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.000\nAnn Lee: hello all\n\n2\n00:00:03.000 --> 00:00:04.000\nBo Kim: hi\n";
        let c = parse_cues(zoom);
        assert_eq!((c[1].speaker.as_deref(), c[1].text.as_str()), (Some("Bo Kim"), "hi"));
        // Prose that merely contains a colon is not split when it isn't the dominant shape.
        let prose = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nnote: this is fine\n\n00:00:03.000 --> 00:00:04.000\nno speaker here\n\n00:00:05.000 --> 00:00:06.000\nor here\n";
        assert!(parse_cues(prose).iter().all(|c| c.speaker.is_none()));
    }

    #[test]
    fn filename_with_time_keeps_date_and_clean_title() {
        assert_eq!(title_and_date("Subscriber Usage Data for Second Brain_2026-10-13_16-39_Transcript.vtt"), ("Subscriber Usage Data for Second Brain".into(), Some("2026-10-13".into())));
        assert_eq!(title_and_date("Following- Claude Office Hours_2026-08-04_15-32_Transcript.vtt"), ("Claude Office Hours".into(), Some("2026-08-04".into())));
        assert_eq!(title_and_date("Innovation Weekly Sync_2026-10-02_Transcript.vtt"), ("Innovation Weekly Sync".into(), Some("2026-10-02".into())));
    }

    #[test]
    fn ai_header_parses_literal_newlines_and_crlf() {
        let v = fixture("ai-header.vtt");
        let h = parse_header(&v).unwrap();
        assert_eq!(h.summary.as_deref(), Some("The team reviewed subscriber usage and agreed on a dashboard approach. A follow-up is planned."));
        assert_eq!(h.actions, ["Zac Wolf to share the usage export by Friday.", "Lisa Pei to draft the rollout note and send it to the team."]);
        assert_eq!(h.owner.as_deref(), Some("Wolf, Zac"));
        assert_eq!(h.mentions.len(), 2);
        assert_eq!((h.mentions[0].ts.as_str(), h.mentions[0].text.as_str()), ("00:00:04", "Wolf, Zac opens the meeting."));
        let cues = parse_cues(&v);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].speaker.as_deref(), Some("Wolf, Zac"));
    }

    #[test]
    fn ai_header_placeholder_owner_no_mentions_and_cue_id() {
        let v = fixture("ai-header-no-mentions.vtt");
        let h = parse_header(&v).unwrap();
        assert_eq!(h.owner.as_deref(), Some("Erik Eastham"));
        assert!(h.mentions.is_empty());
        assert_eq!(h.actions, ["Erik to update the plan."]);
        assert_eq!(parse_cues(&v).len(), 1);
    }

    #[test]
    fn no_header_means_none_and_unchanged_render() {
        let v = fixture("simple.vtt");
        assert!(parse_header(&v).is_none());
        let md = render(&merge_cues(parse_cues(&v)), "s.vtt", "T", None, "2026-01-01", None);
        assert!(md.contains("type: transcript\n") && !md.contains("AI Summary"));
    }

    #[test]
    fn header_renders_sections_and_never_changes_hash() {
        let v = fixture("ai-header.vtt");
        let cues = merge_cues(parse_cues(&v));
        let h = parse_header(&v);
        let md = render(&cues, "s.vtt", "T", None, "2026-01-01", h.as_ref());
        assert!(md.contains("type: transcript-ai\n") && md.contains("owner: \"Wolf, Zac\"\n"));
        let at = |s: &str| md.find(s).unwrap();
        assert!(at("## AI Summary") < at("## Key Actions") && at("## Owner Mentions") < at("## [00:00:04]"));
        assert!(md.contains("1. Zac Wolf to share") && md.contains("- [00:00:04] Wolf, Zac opens"));
        let bare = "WEBVTT\n\n00:00:04.182 --> 00:00:23.142\n<v Wolf, Zac>Welcome everyone to the usage review.</v>\n\n00:00:30.000 --> 00:00:40.000\n<v Pei, Lisa>Zac, can you share the export?</v>\n";
        assert_eq!(content_hash(&cues), content_hash(&merge_cues(parse_cues(bare))));
    }

    #[test]
    fn header_scan_picks_up_header_and_migrates_old_state() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("src"), tmp.path().join("out"));
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("Sync_2026-10-13_16-39_Transcript.vtt"), fixture("ai-header.vtt")).unwrap();
        std::fs::write(src.join("Old_2026-10-01_10-00_Transcript.vtt"), fixture("simple.vtt")).unwrap();
        let rules = Rules::embedded();
        scan(&src, &out, &rules).unwrap();
        let mut st = load_state(&out);
        let e = &st.entries["Sync_2026-10-13_16-39_Transcript.vtt"];
        assert!(e.header_checked && e.header.as_ref().unwrap().actions.len() == 2);
        assert!(st.entries["Old_2026-10-01_10-00_Transcript.vtt"].header.is_none());
        // Pre-header state: same mtime, no header recorded -> one re-scan fills it in.
        for e in st.entries.values_mut() {
            e.header = None;
            e.header_checked = false;
        }
        save_state(&out, &st).unwrap();
        let r = scan(&src, &out, &rules).unwrap();
        let st = load_state(&out);
        assert!(st.entries["Sync_2026-10-13_16-39_Transcript.vtt"].header.is_some());
        assert_eq!(r.converted, 1, "only the headered file is rewritten");
        assert_eq!(scan(&src, &out, &rules).unwrap().processed, 0);
    }

    #[test]
    fn hash_ignores_timestamps_whitespace_bom_crlf() {
        let a = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v A>hello   world</v>\n";
        let b = "\u{feff}WEBVTT\r\n\r\n00:09:01.000 --> 00:09:02.000\r\n<v A>hello world</v>\r\n";
        let c = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v A>hello there</v>\n";
        let h = |s| content_hash(&merge_cues(parse_cues(s)));
        assert_eq!(h(a), h(b));
        assert_ne!(h(a), h(c));
    }

    #[test]
    fn filename_parsing() {
        assert_eq!(
            title_and_date("Following- Walmart internal sync _2026-09-24_Transcript.vtt"),
            ("Walmart internal sync".to_string(), Some("2026-09-24".to_string()))
        );
        assert_eq!(title_and_date("talk_2026-01-02_Transcript.vtt"), ("talk".to_string(), Some("2026-01-02".to_string())));
        assert_eq!(title_and_date("odd name.vtt"), ("odd name".to_string(), None));
        assert_eq!(title_norm("Kaizen Report\u{2011}Out"), "kaizen report-out");
    }

    #[test]
    fn renamed_category_migrates_on_load() {
        let d = std::env::temp_dir().join(format!("tr-rename-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            state_file(&d),
            r#"{"entries":{"a.vtt":{"title":"t","vtt_mtime":1,"category":"PD / Policy Deployment Meetings","category_source":"title","confidence":"high"}},"overrides":{"x":"PD / Policy Deployment Meetings"}}"#,
        )
        .unwrap();
        let st = load_state(&d);
        assert_eq!(st.entries["a.vtt"].category, "PD / Strat Meetings");
        assert_eq!(st.overrides["x"], "PD / Strat Meetings");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn hide_and_logged_survive_rescan_and_unhide_clears_logged() {
        let d = std::env::temp_dir().join(format!("tr-hide-{}", std::process::id()));
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("A Meeting-20260101_100000-Meeting Recording.vtt"), "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Ann>hello there</v>\n").unwrap();
        let rules = Rules::embedded();
        scan(&src, &out, &rules).unwrap();
        let v = vec![load_state(&out).entries.keys().next().unwrap().clone()];
        let raw = d.join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        assert_eq!(add_to_raw(&out, &raw, &v).unwrap(), (1, 0));
        let e = &load_state(&out).entries[&v[0]];
        assert!(e.logged && e.hidden && e.hidden_reason.as_deref() == Some("logged"));
        // source changes: flags must survive reprocessing
        std::fs::write(src.join(&v[0]), "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Ann>hello again</v>\n").unwrap();
        let bumped = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(src.join(&v[0])).unwrap().set_modified(bumped).unwrap();
        scan(&src, &out, &rules).unwrap();
        assert!(load_state(&out).entries[&v[0]].logged);
        set_hidden(&out, &v, false).unwrap();
        let e = &load_state(&out).entries[&v[0]];
        assert!(!e.hidden && !e.logged);
        set_hidden(&out, &v, true).unwrap();
        assert_eq!(load_state(&out).entries[&v[0]].hidden_reason.as_deref(), Some("manual"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn move_and_auto_move_rules() {
        let d = std::env::temp_dir().join(format!("tr-move-{}", std::process::id()));
        let (src, out, dest) = (d.join("src"), d.join("out"), d.join("dest"));
        std::fs::create_dir_all(&src).unwrap();
        let vtt = |t: &str, body: &str| format!("WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Ann>{body} {t}</v>\n");
        let (a, b) = ("Weekly Release Management-20260101_100000-Meeting Recording.vtt", "Kaizen Review-20260102_100000-Meeting Recording.vtt");
        std::fs::write(src.join(a), vtt("a", "release")).unwrap();
        std::fs::write(src.join(b), vtt("b", "kaizen")).unwrap();
        scan(&src, &out, &Rules::embedded()).unwrap();
        let rule = |id: &str, dest: &Path, mode: &str, cats: &[&str]| crate::config::AutoRule {
            id: id.into(),
            action: "move".into(),
            enabled: true,
            dest: Some(dest.to_path_buf()),
            mode: mode.into(),
            categories: cats.iter().map(|c| c.to_string()).collect(),
            after_days: 0,
            since: None,
        };
        let cfg = vec![rule("r", &dest, "include", &["Release Meetings"])];
        let r = auto_move(&out, &cfg).unwrap();
        assert_eq!((r.moved, r.errors.len()), (0, 1), "missing destination reports and never deletes");
        assert!(load_state(&out).entries[a].md_path.as_deref().is_some_and(|p| Path::new(p).is_file()));
        std::fs::create_dir_all(&dest).unwrap();
        assert_eq!(auto_move_preview(&out, &cfg, 0), 1);
        assert_eq!(auto_move(&out, &cfg).unwrap().moved, 1);
        let st = load_state(&out);
        assert!(st.entries[a].md_path.as_deref().unwrap().starts_with(dest.to_str().unwrap()));
        assert_eq!(st.entries[b].moved_by, None, "other category stays");
        assert_eq!(auto_move(&out, &cfg).unwrap().moved, 0, "rerun is a no-op");
        // exclude mode picks up everything not listed; a manual move is never re-moved by the rule
        let manual = d.join("manual");
        std::fs::create_dir_all(&manual).unwrap();
        assert_eq!(move_selected(&out, &[b.to_string()], &manual).unwrap().moved, 1);
        let ex = vec![rule("x", &dest, "exclude", &[])];
        assert_eq!(auto_move_preview(&out, &ex, 0), 0);
        // two rules, two folders; the first match wins an overlap
        let (d1, d2) = (d.join("d1"), d.join("d2"));
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();
        let c = std::fs::read_to_string(src.join(b)).unwrap();
        std::fs::write(src.join("Kaizen Two-20260103_100000-Meeting Recording.vtt"), c.replace("kaizen b", "kaizen z")).unwrap();
        scan(&src, &out, &Rules::embedded()).unwrap();
        let two = vec![rule("k", &d1, "include", &["Kaizen Meetings"]), rule("all", &d2, "exclude", &[])];
        assert_eq!(auto_move_preview(&out, &two, 1), 0, "kaizen already claimed by rule 1");
        assert_eq!(auto_move(&out, &two).unwrap().moved, 1);
        assert!(d1.read_dir().unwrap().count() == 1 && d2.read_dir().unwrap().count() == 0);
        // name collision gets a suffix, never an overwrite
        std::fs::write(dest.join("Kaizen Review-20260102_100000-Meeting Recording.md"), "other").unwrap();
        move_selected(&out, &[b.to_string()], &dest).unwrap();
        assert!(dest.join("Kaizen Review-20260102_100000-Meeting Recording-2.md").is_file());
        assert_eq!(std::fs::read_to_string(dest.join("Kaizen Review-20260102_100000-Meeting Recording.md")).unwrap(), "other");
        // changed source rewrites the moved file in place, not a stray copy in out/
        std::fs::write(src.join(a), vtt("a", "release changed")).unwrap();
        let bumped = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(src.join(a)).unwrap().set_modified(bumped).unwrap();
        scan(&src, &out, &Rules::embedded()).unwrap();
        let p = load_state(&out).entries[a].md_path.clone().unwrap();
        assert!(p.starts_with(dest.to_str().unwrap()) && std::fs::read_to_string(&p).unwrap().contains("changed"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn raw_stubs_for_manual_auto_and_moved_transcripts() {
        let d = std::env::temp_dir().join(format!("tr-stub-{}", std::process::id()));
        let (src, out, raw, dest) = (d.join("src"), d.join("out"), d.join("raw"), d.join("dest"));
        for dir in [&src, &raw, &dest] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let vtt = |b: &str| format!("WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Ann>{b}</v>\n");
        let (a, b) = ("Weekly Release Management-20260101_100000-Meeting Recording.vtt", "Kaizen Review-20260102_100000-Meeting Recording.vtt");
        std::fs::write(src.join(a), vtt("release")).unwrap();
        std::fs::write(src.join(b), vtt("kaizen")).unwrap();
        scan(&src, &out, &Rules::embedded()).unwrap();
        let rule = |cats: &[&str], enabled, since: Option<&str>| crate::config::AutoRule {
            id: "r".into(),
            action: "log-it".into(),
            enabled,
            dest: None,
            mode: "include".into(),
            categories: cats.iter().map(|c| c.to_string()).collect(),
            after_days: 0,
            since: since.map(Into::into),
        };
        let both = ["Release Meetings", "Kaizen Meetings"];
        assert_eq!(auto_add_to_raw(&out, &raw, &[rule(&both, false, None)]).unwrap(), 0, "disabled rule writes nothing");
        assert_eq!(auto_add_to_raw(&out, &raw, &[rule(&both, true, Some("2999-01-01"))]).unwrap(), 0, "`since` in the future excludes existing files");
        // a taken file name is never overwritten
        let ea = load_state(&out).entries[a].clone();
        let taken = raw.join(format!("{} - {} (transcript).md", ea.date.clone().or(ea.converted_at.clone()).unwrap(), ea.title));
        std::fs::write(&taken, "other").unwrap();
        set_hidden(&out, &[b.to_string()], true).unwrap();
        let rules = [rule(&both, true, Some(&today_utc()))];
        assert_eq!(auto_add_to_raw(&out, &raw, &rules).unwrap(), 1, "hidden stays out");
        let e = load_state(&out).entries[a].clone();
        assert!(e.logged && e.hidden && e.hidden_reason.as_deref() == Some("logged"));
        let stub = e.raw_note.clone().unwrap();
        assert_ne!(Path::new(&stub), taken.as_path());
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "other");
        let text = std::fs::read_to_string(&stub).unwrap();
        assert!(text.contains("type: transcript-ref") && text.contains(&serde_json::to_string(e.md_path.as_deref().unwrap()).unwrap()));
        assert_eq!(auto_add_to_raw(&out, &raw, &rules).unwrap(), 0, "rerun is a no-op");
        // manual add; duplicates and repeats are skipped; a missing raw folder errors without touching state
        assert_eq!(add_to_raw(&out, &raw, &[b.to_string(), a.to_string()]).unwrap(), (1, 1));
        assert!(add_to_raw(&out, &d.join("nope"), &[a.to_string()]).is_err());
        // moving the transcript rewrites the stub's pointer
        assert_eq!(move_selected(&out, &[a.to_string()], &dest).unwrap().moved, 1);
        let moved = load_state(&out).entries[a].md_path.clone().unwrap();
        assert!(std::fs::read_to_string(&stub).unwrap().contains(&serde_json::to_string(&moved).unwrap()));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn auto_hide_respects_age_and_unhide_sticks() {
        let d = std::env::temp_dir().join(format!("tr-autohide-{}", std::process::id()));
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        let n = "Weekly Release Management_2026-01-01_10-00_Transcript.vtt";
        std::fs::write(src.join(n), "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Ann>hide me body words</v>\n").unwrap();
        scan(&src, &out, &Rules::embedded()).unwrap_or_else(|e| panic!("{e:#}"));
        let rule = |days| crate::config::AutoRule { id: "h".into(), action: "hide".into(), enabled: true, dest: None, mode: "exclude".into(), categories: vec![], after_days: days, since: None };
        assert_eq!(auto_hide(&out, &[rule(100_000)]).unwrap(), 0, "meeting is not that old");
        assert_eq!(auto_hide(&out, &[rule(30)]).unwrap(), 1, "converted today but the meeting date is old");
        let e = load_state(&out).entries[n].clone();
        assert!(e.hidden && e.auto_hid && e.hidden_reason.as_deref() == Some("auto"));
        set_hidden(&out, &[n.to_string()], false).unwrap();
        assert_eq!(auto_hide(&out, &[rule(0)]).unwrap(), 0, "unhide sticks");
        scan(&src, &out, &Rules::embedded()).unwrap();
        assert!(load_state(&out).entries[n].auto_hid, "auto_hid survives a rescan");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn voc_queue_skips_other_skipped_and_ingested() {
        let d = std::env::temp_dir().join(format!("tr-vocq-{}", std::process::id()));
        let (src, out, bridge) = (d.join("src"), d.join("out"), d.join("bridge"));
        std::fs::create_dir_all(&src).unwrap();
        let vtt = |b: &str| format!("WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Ann>{b}</v>\n");
        let names = ["Weekly Release Management-20260101_100000-Meeting Recording.vtt", "Kaizen Review-20260102_100000-Meeting Recording.vtt", "Kaizen Review-20260103_100000-Meeting Recording.vtt"];
        for (i, n) in names.iter().enumerate() {
            std::fs::write(src.join(n), vtt(&format!("body {i} unique words here"))).unwrap();
        }
        scan(&src, &out, &Rules::embedded()).unwrap();
        let rule = crate::config::AutoRule { id: "v".into(), action: "voc".into(), enabled: true, dest: None, mode: "exclude".into(), categories: vec![], after_days: 0, since: None };
        let rules = [rule];
        let n0 = write_voc_queue(&out, &rules, &bridge).unwrap();
        let q: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(bridge.join("voc-queue.json")).unwrap()).unwrap();
        assert_eq!(q["max_per_run"], VOC_MAX_PER_RUN);
        assert!(q["items"].as_array().unwrap().iter().all(|i| i["category"] != "Other"));
        set_voc_skip(&out, &[names[0].to_string()], true).unwrap();
        mark_voc(&out, &[names[1].to_string()]).unwrap();
        assert!(write_voc_queue(&out, &rules, &bridge).unwrap() < n0, "skipped and ingested rows drop out");
        let ready = voc_ready(&out, &rules);
        assert!(!ready.contains(&names[0].to_string()) && !ready.contains(&names[1].to_string()), "skipped and ingested rows are not offered");
        assert!(ready.len() <= VOC_MAX_PER_RUN);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn today_is_iso_date() {
        let t = today_utc();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }

    fn vtt(speaker: &str, text: &str) -> String {
        format!("WEBVTT\n\n00:00:01.000 --> 00:01:00.000\n<v {speaker}>{text}</v>\n")
    }

    #[test]
    fn scan_converts_dedupes_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("src"), tmp.path().join("out"));
        std::fs::create_dir(&src).unwrap();
        let same = vtt("A", "identical content across dates");
        std::fs::write(src.join("Weekly Thing_2026-09-10_Transcript.vtt"), &same).unwrap();
        std::fs::write(src.join("Weekly Thing_2026-09-03_Transcript.vtt"), &same).unwrap();
        std::fs::write(src.join("Weekly Thing_2026-09-17_Transcript.vtt"), vtt("A", "different content")).unwrap();
        std::fs::write(src.join("notes.txt"), "ignored").unwrap();
        let rules = Rules::embedded();

        let r = scan(&src, &out, &rules).unwrap();
        assert_eq!((r.processed, r.converted), (3, 2), "duplicate is not converted");
        let rows = rows(&out);
        let dup = rows.iter().find(|r| r.vtt.contains("09-10")).unwrap();
        assert_eq!(dup.duplicate_of.as_deref(), Some("Weekly Thing_2026-09-03_Transcript.vtt"), "earliest date is canonical");
        let orig = rows.iter().find(|r| r.vtt.contains("09-03")).unwrap();
        assert_eq!(dup.open_path, orig.open_path, "duplicate opens the original");
        let md = std::fs::read_to_string(orig.open_path.as_ref().unwrap()).unwrap();
        assert!(md.contains("# Weekly Thing") && md.contains("A:\n\nidentical content across dates"));
        assert!(out.join("index.md").exists());
        let different = rows.iter().find(|r| r.vtt.contains("09-17")).unwrap();
        assert!(different.duplicate_of.is_none(), "same title, different content is not a duplicate");

        let again = scan(&src, &out, &rules).unwrap();
        assert_eq!((again.processed, again.converted), (0, 0), "rerun is a no-op");

        std::fs::write(src.join("New_2026-10-01_Transcript.vtt"), vtt("B", "fresh")).unwrap();
        let third = scan(&src, &out, &rules).unwrap();
        assert_eq!((third.processed, third.converted), (1, 1));
    }

    #[test]
    fn voc_flags_survive_rescan() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("src"), tmp.path().join("out"));
        std::fs::create_dir(&src).unwrap();
        let name = "Interview_2026-09-03_Transcript.vtt";
        std::fs::write(src.join(name), vtt("A", "first")).unwrap();
        scan(&src, &out, &Rules::embedded()).unwrap();
        let v = vec![name.to_string()];
        mark_voc(&out, &v).unwrap();
        set_voc_skip(&out, &v, true).unwrap();
        std::fs::write(src.join(name), vtt("A", "second")).unwrap();
        let bumped = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(src.join(name)).unwrap().set_modified(bumped).unwrap();
        scan(&src, &out, &Rules::embedded()).unwrap();
        let r = rows(&out).into_iter().next().unwrap();
        assert!(r.voc_ingested && r.voc_skip && !r.hidden, "VoC flags persist and never hide");
        set_voc_skip(&out, &v, false).unwrap();
        assert!(!rows(&out)[0].voc_skip);
    }

    #[test]
    fn near_duplicate_guard_flags_trivial_reexports_only() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("src"), tmp.path().join("out"));
        std::fs::create_dir(&src).unwrap();
        let body: Vec<String> = (0..1000).map(|i| format!("word{i}")).collect();
        let base = body.join(" ");
        // re-export with one word changed: 5 of ~1000 shingles differ
        let tweaked = base.replacen("word500", "changed", 1);
        let mut real = body.clone();
        real.iter_mut().skip(100).step_by(3).for_each(|w| w.push('x')); // ~1/3 of the words differ
        std::fs::write(src.join("Sync_2026-09-03_Transcript.vtt"), vtt("A", &base)).unwrap();
        std::fs::write(src.join("Sync_2026-09-10_Transcript.vtt"), vtt("A", &tweaked)).unwrap();
        std::fs::write(src.join("Sync_2026-09-17_Transcript.vtt"), vtt("A", &real.join(" "))).unwrap();
        scan(&src, &out, &Rules::embedded()).unwrap();
        let rows = rows(&out);
        let by = |d: &str| rows.iter().find(|r| r.vtt.contains(d)).unwrap();
        assert_eq!(by("09-10").duplicate_of.as_deref(), Some("Sync_2026-09-03_Transcript.vtt"));
        assert!(by("09-10").near_duplicate && !by("09-03").near_duplicate);
        assert!(by("09-17").duplicate_of.is_none(), "genuinely different content stays");
    }

    #[test]
    fn suspect_download_heals_when_the_real_transcript_arrives() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("src"), tmp.path().join("out"));
        std::fs::create_dir(&src).unwrap();
        let (first, second) = ("Weekly_2026-09-03_Transcript.vtt", "Weekly_2026-09-10_Transcript.vtt");
        std::fs::write(src.join(first), vtt("A", "week one")).unwrap();
        std::fs::write(src.join(second), vtt("A", "week one")).unwrap(); // bad download: same content
        let rules = Rules::embedded();
        scan(&src, &out, &rules).unwrap();
        assert!(rows(&out).iter().find(|r| r.vtt == second).unwrap().duplicate_of.is_some());

        // IT fixes the route; the real transcript replaces the file (newer mtime).
        std::fs::write(src.join(second), vtt("A", "week two, real content")).unwrap();
        let bumped = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(src.join(second)).unwrap().set_modified(bumped).unwrap();

        let r = scan(&src, &out, &rules).unwrap();
        assert_eq!((r.processed, r.converted), (1, 1));
        let healed = rows(&out).into_iter().find(|r| r.vtt == second).unwrap();
        assert!(healed.duplicate_of.is_none() && healed.md_exists, "now a normal, converted transcript");
        assert!(std::fs::read_to_string(healed.open_path.unwrap()).unwrap().contains("week two, real content"));
    }

    #[test]
    fn manual_category_sticks_to_the_series() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("src"), tmp.path().join("out"));
        std::fs::create_dir(&src).unwrap();
        let first = "Mystery Series_2026-09-03_Transcript.vtt";
        std::fs::write(src.join(first), vtt("A", "one")).unwrap();
        let rules = Rules::embedded();
        let r = scan(&src, &out, &rules).unwrap();
        assert_eq!(r.auto_other.len(), 1, "fallback to Other is reported for the review loop");

        let ch = set_category(&out, &[first.to_string()], "Kaizen Meetings").unwrap();
        assert_eq!((ch[0].from.as_str(), ch[0].auto_source.as_str()), ("Other", "fallback"));
        std::fs::write(src.join("Mystery Series_2026-09-10_Transcript.vtt"), vtt("A", "two")).unwrap();
        let r = scan(&src, &out, &rules).unwrap();
        assert!(r.auto_other.is_empty(), "next occurrence is right without another click");
        let next = rows(&out).into_iter().find(|r| r.vtt.contains("09-10")).unwrap();
        assert_eq!((next.category.as_str(), next.category_source.as_str()), ("Kaizen Meetings", "manual"));
    }

    /// Manual check against a real folder: `TRANSCRIPTS_SRC=... cargo test real_folder -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_folder() {
        let src = PathBuf::from(std::env::var("TRANSCRIPTS_SRC").expect("TRANSCRIPTS_SRC"));
        let out = tempfile::tempdir().unwrap();
        let rules = Rules::embedded();
        let t = std::time::Instant::now();
        let r = scan(&src, out.path(), &rules).unwrap();
        println!("{r:?} in {:?}", t.elapsed());
        let rows = rows(out.path());
        let groups: HashSet<_> = rows.iter().filter_map(|r| r.duplicate_of.clone()).collect();
        println!("{} rows, {} duplicate groups, {} duplicates", rows.len(), groups.len(), rows.iter().filter(|r| r.duplicate_of.is_some()).count());
        let mut by_cat: BTreeMap<String, usize> = BTreeMap::new();
        for r in rows.iter().filter(|r| r.duplicate_of.is_none()) {
            *by_cat.entry(format!("{} [{}]", r.category, r.confidence)).or_default() += 1;
        }
        println!("{by_cat:#?}");
        for r in rows.iter().filter(|r| r.duplicate_of.is_none() && r.category == "Other") {
            println!("OTHER: {}", r.title);
        }
        assert_eq!(scan(&src, out.path(), &rules).unwrap().processed, 0);
    }
}
