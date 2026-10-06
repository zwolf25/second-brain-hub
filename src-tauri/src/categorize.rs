//! Zero-token transcript categorization: title rules, series inheritance,
//! a clear-signal VoC gate, body keyword scoring, then structure. Rules live in
//! `resources/categories.json` (embedded); the shared vault's
//! `transcript-audit/categories.json` overrides it when present so the owner can
//! tune accuracy without a Hub release.

use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

const EMBEDDED: &str = include_str!("../resources/categories.json");
const OTHER: &str = "Other";

#[derive(Debug, Clone, Deserialize)]
pub struct TitleRule {
    pub id: String,
    pub category: String,
    #[serde(default)]
    pub any: Vec<String>,
    #[serde(default)]
    pub all: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rules {
    pub version: u32,
    pub voc_category: String,
    pub categories: Vec<String>,
    pub title_rules: Vec<TitleRule>,
    pub voc_title_keywords: Vec<String>,
    pub voc_body_cues: Vec<String>,
    pub body_terms: BTreeMap<String, Vec<String>>,
}

impl Rules {
    pub fn embedded() -> Rules {
        serde_json::from_str(EMBEDDED).expect("embedded categories.json is valid")
    }

    /// Shared-vault copy wins when it parses; any problem falls back to embedded.
    pub fn load(override_path: Option<&Path>) -> Rules {
        override_path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(Rules::embedded)
    }
}

/// An already-categorized transcript, used for series inheritance.
pub struct Peer<'a> {
    pub title_norm: &'a str,
    pub speakers: &'a [String],
    pub category: &'a str,
    pub source: &'a str,
}

pub struct Input<'a> {
    pub title_norm: &'a str,
    pub speakers: &'a [String],
    /// (speaker, words spoken), for talk-time shape.
    pub speaker_words: &'a [(String, u32)],
    pub body_lower: &'a str,
    pub words: u32,
    /// Per-series manual override (title_norm -> category); beats every rule.
    pub override_category: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub category: String,
    pub source: String,
    /// "high" or "low" (low = shown as "suggested" in the UI).
    pub confidence: &'static str,
}

fn verdict(category: &str, source: impl Into<String>, confidence: &'static str) -> Verdict {
    Verdict { category: category.to_string(), source: source.into(), confidence }
}

pub fn title_rule(rules: &Rules, title_norm: &str) -> Option<Verdict> {
    rules.title_rules.iter().find_map(|r| {
        let all_ok = r.all.iter().all(|k| title_norm.contains(k.as_str()));
        let any_ok = r.any.is_empty() || r.any.iter().any(|k| title_norm.contains(k.as_str()));
        (all_ok && any_ok).then(|| verdict(&r.category, format!("title-rule:{}", r.id), "high"))
    })
}

fn jaccard(a: &[String], b: &[String]) -> f32 {
    let a: HashSet<&String> = a.iter().collect();
    let b: HashSet<&String> = b.iter().collect();
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f32 / union as f32
}

/// Peers whose own category came from a confident source; body/structure guesses never propagate.
fn inheritable(source: &str) -> bool {
    source.starts_with("title-rule") || source == "series" || source == "manual" || source == "voc-gate"
}

fn voc_signals(rules: &Rules, i: &Input, internal: &HashSet<String>) -> usize {
    let mut n = 0;
    if rules.voc_title_keywords.iter().any(|k| i.title_norm.contains(k.as_str())) {
        n += 1;
    }
    // External-participant signals need a learned internal-name set; with none
    // yet, everyone would look external.
    if !internal.is_empty() && !i.speakers.is_empty() {
        let external = i.speakers.iter().filter(|s| !internal.contains(*s)).count();
        if external * 2 > i.speakers.len() {
            n += 1;
        }
        let total: u32 = i.speaker_words.iter().map(|(_, w)| w).sum();
        let ext_words: u32 = i.speaker_words.iter().filter(|(s, _)| !internal.contains(s)).map(|(_, w)| w).sum();
        if total > 0 && i.speakers.len() <= 5 && ext_words * 2 >= total {
            n += 1;
        }
    }
    let cues: usize = rules.voc_body_cues.iter().map(|c| i.body_lower.matches(c.as_str()).count()).sum();
    if cues >= 3 {
        n += 1;
    }
    n
}

const BODY_FLOOR_PER_1K: f32 = 3.0;

fn body_score(rules: &Rules, i: &Input) -> Option<Verdict> {
    if i.words == 0 {
        return None;
    }
    let mut scores: Vec<(&String, f32)> = rules
        .body_terms
        .iter()
        .map(|(cat, terms)| {
            let hits: usize = terms.iter().map(|t| i.body_lower.matches(t.as_str()).count()).sum();
            (cat, hits as f32 * 1000.0 / i.words as f32)
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (cat, top) = *scores.first()?;
    let runner_up = scores.get(1).map_or(0.0, |s| s.1);
    (top >= BODY_FLOOR_PER_1K && top >= runner_up * 2.0).then(|| verdict(cat, "body-score", "low"))
}

pub fn categorize(rules: &Rules, i: &Input, peers: &[Peer], internal: &HashSet<String>) -> Verdict {
    if let Some(c) = i.override_category {
        return verdict(c, "manual", "high");
    }
    if let Some(v) = title_rule(rules, i.title_norm) {
        return v;
    }
    let inherited = peers.iter().find(|p| {
        inheritable(p.source)
            && p.category != OTHER
            && (p.title_norm == i.title_norm || (i.speakers.len() >= 2 && jaccard(p.speakers, i.speakers) >= 0.8))
    });
    if let Some(p) = inherited {
        return verdict(p.category, "series", "high");
    }
    // Clear-signal gate: VoC is never a fallback, it needs >= 2 independent signals.
    if voc_signals(rules, i, internal) >= 2 {
        return verdict(&rules.voc_category, "voc-gate", "high");
    }
    if let Some(v) = body_score(rules, i) {
        return v;
    }
    if i.speakers.len() == 2 {
        return verdict("1-on-1s", "structure", "low");
    }
    verdict(OTHER, "fallback", "low")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real titles from the 156-file download folder, expected per the confirmed plan table.
    const TITLE_TABLE: &[(&str, &str)] = &[
        ("walmart internal sync", "Hyper Enterprise Meetings"),
        ("walmart hyper enterprise - touchbase", "Hyper Enterprise Meetings"),
        ("cvs accelerated development meeting", "Hyper Enterprise Meetings"),
        ("roadchef internal alignment", "Customer Related Internal Syncs"),
        ("wincco foods (internal) _ refrigerant tracking issues", "Customer Related Internal Syncs"),
        ("weekly kaizen report-out & sustainment check-in (engineering)", "Kaizen Meetings"),
        ("[r3 2026 toi] ss - slack integration", "Release Meetings"),
        ("r3 weekly release management", "Release Meetings"),
        ("servicechannel august cab meeting", "CAB Meetings"),
        ("cab - run of show", "CAB Meetings"),
        ("cab deck review session", "CAB Meetings"),
        ("servicechannel town hall", "Town Halls"),
        ("weekly scoping review session (tpm)", "Scoping Reviews"),
        ("c3 scope review (2 of 3)", "Scoping Reviews"),
        ("viewpost __ servicechannel sync", "Partner Meetings"),
        ("glacier grid demo- servicechannel", "Partner Meetings"),
        ("red program review - july lpm", "LPM Meetings"),
        ("lpm deck review - july", "LPM Meetings"),
        ("product commercialization — easypay bi-weekly sync", "Product Commercialization Meetings"),
        ("servicechannel ai & action hub packaging discussion & alignment", "Product Commercialization Meetings"),
        ("customer renewals obeya", "Obeya Meetings"),
        ("2027 roadmap l2 steerco gate reviews (session 1 of 2)", "Roadmapping Meetings"),
        ("roadmapping office hours", "Roadmapping Meetings"),
        ("innovation weekly sync", "Innovation Meetings"),
        ("2027 pd discussion", "PD / Policy Deployment Meetings"),
        ("ai business empowerment strat _ weekly 30m", "PD / Policy Deployment Meetings"),
        ("claude office hours", "Group / Team Meetings"),
        ("monthly fr risk review", "Group / Team Meetings"),
        ("supply manager migration weekly sync", "Project Meetings / Standups"),
        ("ai action hub sync", "Project Meetings / Standups"),
        ("zac-casey hotel integration sync", "1-on-1s"),
    ];

    const NO_TITLE_RULE: &[&str] = &[
        "following- walmart sync",
        "lowe's-servicechannel sync",
        "fbs champion journey update- new requirements",
        "state of it _ strategic review & prioritization with senior leadership",
        "review vision workshop for p&e offsite",
        "poc prioritization process",
        "t-shirt sizing sanity check - action hub",
        "talk",
        "ff deprecation task",
    ];

    #[test]
    fn title_table() {
        let rules = Rules::embedded();
        for (title, want) in TITLE_TABLE {
            let got = title_rule(&rules, &title.to_lowercase()).map(|v| v.category);
            assert_eq!(got.as_deref(), Some(*want), "title: {title}");
        }
        for title in NO_TITLE_RULE {
            assert!(title_rule(&rules, title).is_none(), "should not match a title rule: {title}");
        }
    }

    fn input<'a>(title: &'a str, speakers: &'a [String], words: &'a [(String, u32)], body: &'a str) -> Input<'a> {
        Input { title_norm: title, speakers, speaker_words: words, body_lower: body, words: body.split_whitespace().count() as u32, override_category: None }
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn voc_needs_two_signals() {
        let rules = Rules::embedded();
        let internal: HashSet<String> = names(&["Wolf, Zac"]).into_iter().collect();
        let sp = names(&["Wolf, Zac", "Acme, Pat"]);
        let w = vec![("Wolf, Zac".to_string(), 100), ("Acme, Pat".to_string(), 400)];
        // Customer name only in title, routine sync: stays out of VoC.
        let routine = input("acme sync", &sp, &w, "status update on tickets");
        assert_ne!(categorize(&rules, &routine, &[], &internal).category, rules.voc_category);
        // Interview keyword + external majority + talk share = VoC.
        let iv = input("acme customer interview", &sp, &w, "tell me about your day");
        assert_eq!(categorize(&rules, &iv, &[], &internal).category, rules.voc_category);
        // One weak signal (keyword only, no learned internal names) is not enough.
        let one = input("acme customer interview", &sp, &w, "hello");
        assert_ne!(categorize(&rules, &one, &[], &HashSet::new()).category, rules.voc_category);
    }

    #[test]
    fn series_inherits_only_confident_sources() {
        let rules = Rules::embedded();
        let sp = names(&["A", "B", "C"]);
        let peers = [Peer { title_norm: "some new series", speakers: &sp, category: "Kaizen Meetings", source: "title-rule:kaizen" }];
        let i = input("some new series", &sp, &[], "x");
        assert_eq!(categorize(&rules, &i, &peers, &HashSet::new()).source, "series");
        let weak = [Peer { title_norm: "some new series", speakers: &sp, category: "Kaizen Meetings", source: "body-score" }];
        assert_eq!(categorize(&rules, &i, &weak, &HashSet::new()).category, "Other");
    }

    #[test]
    fn manual_override_beats_rules() {
        let rules = Rules::embedded();
        let sp = names(&["A", "B", "C"]);
        let mut i = input("claude office hours", &sp, &[], "x");
        assert_eq!(categorize(&rules, &i, &[], &HashSet::new()).category, "Group / Team Meetings");
        i.override_category = Some("Innovation Meetings");
        let v = categorize(&rules, &i, &[], &HashSet::new());
        assert_eq!((v.category.as_str(), v.source.as_str()), ("Innovation Meetings", "manual"));
    }

    #[test]
    fn structure_and_fallback() {
        let rules = Rules::embedded();
        let two = names(&["A", "B"]);
        assert_eq!(categorize(&rules, &input("zzz", &two, &[], "x"), &[], &HashSet::new()).category, "1-on-1s");
        let three = names(&["A", "B", "C"]);
        assert_eq!(categorize(&rules, &input("zzz", &three, &[], "x"), &[], &HashSet::new()).category, "Other");
    }
}
