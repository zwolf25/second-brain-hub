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

pub fn render(cues: &[Cue], source: &str, title: &str, date: Option<&str>, converted: &str) -> String {
    let mut out = format!("---\nsource: {source}\ntype: transcript\ntitle: {}\n", serde_json::to_string(title).unwrap());
    if let Some(d) = date {
        out.push_str(&format!("date: {d}\n"));
    }
    out.push_str(&format!("converted: {converted}\n---\n\n# {title}\n"));
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
    let (stem, date) = match stem.rsplit_once('_') {
        Some((t, d)) if is_date(d) => (t, Some(d.to_string())),
        _ => (stem, None),
    };
    let t = stem.trim();
    let t = if t.to_lowercase().starts_with("following-") { &t[10..] } else { t };
    (t.split_whitespace().collect::<Vec<_>>().join(" "), date)
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
    /// Off the default list (files untouched, still indexed). `hidden_reason` is "manual" or "logged".
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub hidden_reason: Option<String>,
    /// Sent to log-it. Set optimistically at launch (a deep link can't confirm completion).
    #[serde(default)]
    pub logged: bool,
    /// "manual" or "auto"; a moved file is never auto-moved again.
    #[serde(default)]
    pub moved_by: Option<String>,
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
            state.entries.get_mut(n).unwrap().duplicate_of = (i > 0).then(|| canonical.clone());
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
        if state.entries.get(&name).is_some_and(|e| e.vtt_mtime >= mtime) {
            continue;
        }
        // OneDrive cloud-only placeholders can fail to read; skip and retry next scan.
        match process_one(&path, &name, mtime, rules, &state, &internal) {
            Ok(mut entry) => {
                // Changed source: keep md_path so the same file is rewritten, but force reconversion below.
                if let Some(old) = state.entries.get(&name) {
                    entry.md_path = old.md_path.clone();
                    (entry.hidden, entry.hidden_reason, entry.logged, entry.moved_by) = (old.hidden, old.hidden_reason.clone(), old.logged, old.moved_by.clone());
                    reconvert.insert(name.clone());
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
        std::fs::write(&md, render(&cues, &name, &e.title, e.date.as_deref(), &converted))?;
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

/// Marks transcripts as sent to log-it and auto-hides them ("Logged" under Show hidden).
pub fn mark_logged(out: &Path, vtts: &[String]) -> Result<()> {
    let mut state = load_state(out);
    for e in state.entries.iter_mut().filter(|(k, _)| vtts.contains(k)).map(|(_, e)| e) {
        (e.logged, e.hidden, e.hidden_reason) = (true, true, Some("logged".into()));
    }
    save_state(out, &state)
}

#[derive(Debug, Default, Serialize)]
pub struct MoveReport {
    pub moved: usize,
    pub skipped: usize,
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

fn move_entries(out: &Path, dest: &Path, by: &str, pick: impl Fn(&str, &Entry) -> bool) -> Result<MoveReport> {
    anyhow::ensure!(dest.is_dir(), "Destination folder is missing: {}", dest.display());
    let mut state = load_state(out);
    let mut report = MoveReport::default();
    for (name, e) in state.entries.iter_mut() {
        let Some(md) = e.md_path.clone().filter(|p| Path::new(p).is_file()) else { continue };
        if e.duplicate_of.is_some() || !pick(name, e) {
            continue;
        }
        if Path::new(&md).parent() == Some(dest) {
            continue;
        }
        match move_file(Path::new(&md), dest) {
            Ok(to) => {
                e.md_path = Some(to.to_string_lossy().to_string());
                e.moved_by = Some(by.into());
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

/// Manual "Move selected": any converted, non-duplicate transcript, even one moved before.
pub fn move_selected(out: &Path, vtts: &[String], dest: &Path) -> Result<MoveReport> {
    move_entries(out, dest, "manual", |n, _| vtts.iter().any(|v| v == n))
}

fn auto_eligible(cfg: &crate::config::AutoMove, e: &Entry, today: u64) -> bool {
    let listed = cfg.categories.contains(&e.category);
    let in_scope = if cfg.mode == "exclude" { !listed } else { listed };
    let old_enough = e.converted_at.as_deref().is_some_and(|d| today >= crate::audit::days_since_epoch(d) + cfg.after_days);
    in_scope && old_enough && e.moved_by.is_none()
}

/// Files the auto-move rule would move now (the dry-run count shown before enabling).
pub fn auto_move_preview(out: &Path, cfg: &crate::config::AutoMove) -> usize {
    let today = crate::audit::days_since_epoch(&today_utc());
    load_state(out)
        .entries
        .values()
        .filter(|e| e.duplicate_of.is_none() && e.md_path.as_deref().is_some_and(|p| Path::new(p).is_file()) && auto_eligible(cfg, e, today))
        .count()
}

/// Runs at the end of each scan. A manually moved file (`moved_by` set) is never touched again,
/// and a later category edit never moves a file back.
pub fn auto_move(out: &Path, cfg: &crate::config::AutoMove) -> Result<MoveReport> {
    let dest = cfg.dest.as_deref().context("no destination folder set")?;
    let today = crate::audit::days_since_epoch(&today_utc());
    move_entries(out, dest, "auto", |_, e| auto_eligible(cfg, e, today))
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
    /// The `.md` a double-click opens: own file, or the canonical copy for a duplicate.
    pub open_path: Option<String>,
    /// False when the `.md` was deleted or moved outside the Hub; the UI says so instead of erroring.
    pub md_exists: bool,
    pub hidden: bool,
    pub hidden_reason: Option<String>,
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
                open_path: open,
                hidden: e.hidden,
                hidden_reason: e.hidden_reason.clone(),
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
    fn hide_and_logged_survive_rescan_and_unhide_requeues() {
        let d = std::env::temp_dir().join(format!("tr-hide-{}", std::process::id()));
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("A Meeting-20260101_100000-Meeting Recording.vtt"), "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Ann>hello there</v>\n").unwrap();
        let rules = Rules::embedded();
        scan(&src, &out, &rules).unwrap();
        let v = vec![load_state(&out).entries.keys().next().unwrap().clone()];
        mark_logged(&out, &v).unwrap();
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
        let cfg = crate::config::AutoMove { enabled: true, dest: Some(dest.clone()), mode: "include".into(), categories: vec!["Release Meetings".into()], after_days: 0 };
        assert!(auto_move(&out, &cfg).is_err(), "missing destination must fail, never delete");
        assert!(load_state(&out).entries[a].md_path.as_deref().is_some_and(|p| Path::new(p).is_file()));
        std::fs::create_dir_all(&dest).unwrap();
        assert_eq!(auto_move_preview(&out, &cfg), 1);
        assert_eq!(auto_move(&out, &cfg).unwrap().moved, 1);
        let st = load_state(&out);
        assert!(st.entries[a].md_path.as_deref().unwrap().starts_with(dest.to_str().unwrap()));
        assert_eq!(st.entries[b].moved_by, None, "other category stays");
        assert_eq!(auto_move(&out, &cfg).unwrap().moved, 0, "rerun is a no-op");
        // exclude mode picks up everything not listed; a manual move is never re-moved by the rule
        let manual = d.join("manual");
        std::fs::create_dir_all(&manual).unwrap();
        assert_eq!(move_selected(&out, &[b.to_string()], &manual).unwrap().moved, 1);
        let ex = crate::config::AutoMove { mode: "exclude".into(), categories: vec![], ..cfg.clone() };
        assert_eq!(auto_move_preview(&out, &ex), 0);
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
