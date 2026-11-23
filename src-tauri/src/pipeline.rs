//! Pipeline page data: the staleness JSON (passed through as text) and the VoC backlog.
//! The backlog is parsed from `win-loss/interview-wishlist.md`; Claim and Dismiss state lives in one small
//! marker file per candidate under `win-loss/_voc/` (owner decision 2026-11-23: the Hub writes these markers).
use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct VocRow {
    pub key: String,
    pub kind: String, // loss | win | churn
    pub account: String,
    pub date: String,
    pub amount: String,
    pub why: String,
    pub status: String,
    pub claimed_by: String,
    pub declined: String,
}

#[derive(Serialize)]
pub struct PipelineData {
    pub status_json: Option<String>,
    pub voc: Vec<VocRow>,
}

fn kind_for(heading: &str) -> Option<&'static str> {
    let h = heading.to_lowercase();
    if h.starts_with("tier 1") || h.starts_with("tier 3") {
        Some("loss")
    } else if h.starts_with("tier 2") {
        Some("win")
    } else if h.starts_with("tier 5") {
        Some("churn")
    } else {
        None // tier 4 (win-back) and tier 6 (churn track, not an interview ask) stay out
    }
}

/// Marker key: lowercase alphanumerics of the account, dash, closed date.
pub fn marker_key(account: &str, date: &str) -> String {
    let slug: String = account
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!("{slug}-{date}")
}

pub fn parse_wishlist(text: &str) -> Vec<VocRow> {
    let mut rows = Vec::new();
    let mut kind: Option<&'static str> = None;
    let mut header: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            kind = kind_for(h);
            header.clear();
            continue;
        }
        let Some(k) = kind else { continue };
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<String> = line.trim().trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
        if cells.first().map(|c| c == "Account").unwrap_or(false) {
            header = cells;
            continue;
        }
        if header.is_empty() || cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')) {
            continue;
        }
        let col = |names: &[&str]| -> String {
            header
                .iter()
                .position(|h| names.iter().any(|n| h.eq_ignore_ascii_case(n)))
                .and_then(|i| cells.get(i))
                .cloned()
                .unwrap_or_default()
        };
        let account = col(&["Account"]);
        let date = col(&["Closed", "Churned"]);
        if account.is_empty() {
            continue;
        }
        rows.push(VocRow {
            key: marker_key(&account, &date),
            kind: k.to_string(),
            account,
            date,
            amount: col(&["Amount", "ARR"]),
            why: col(&["What the seller view says", "Why", "CSM reason", "Gate"]),
            status: col(&["Status"]),
            claimed_by: String::new(),
            declined: String::new(),
        });
    }
    rows
}

fn marker_field(path: &Path, field: &str) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get(field).and_then(|f| f.as_str().map(String::from)))
        .unwrap_or_default()
}

pub fn load(root: &Path) -> PipelineData {
    let status_json = std::fs::read_to_string(root.join("_status").join("intel-staleness.json")).ok();
    let mut voc = std::fs::read_to_string(root.join("win-loss").join("interview-wishlist.md"))
        .map(|t| parse_wishlist(&t))
        .unwrap_or_default();
    let dir = root.join("win-loss").join("_voc");
    for r in &mut voc {
        r.claimed_by = marker_field(&dir.join(format!("{}.claim.json", r.key)), "by");
        r.declined = marker_field(&dir.join(format!("{}.declined.json", r.key)), "reason");
    }
    PipelineData { status_json, voc }
}

/// kind: "claim" | "declined"; `value` empty removes the marker (Release / Undo).
pub fn set_marker(root: &Path, key: &str, kind: &str, value: &str) -> Result<(), String> {
    if !matches!(kind, "claim" | "declined") || key.is_empty() || key.contains(['/', '\\', '.']) {
        return Err("bad marker".into());
    }
    let dir = root.join("win-loss").join("_voc");
    let path = dir.join(format!("{key}.{kind}.json"));
    if value.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let field = if kind == "claim" { "by" } else { "reason" };
    let body = serde_json::json!({ field: value, "at": chrono_now() });
    std::fs::write(&path, body.to_string()).map_err(|e| e.to_string())
}

fn chrono_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    secs.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "## Tier 1: new-logo losses\n\n| Account | Closed | Amount | Segment | What the seller view says | Ask the buyer | Channel | Status |\n|---|---|---|---|---|---|---|---|\n| Urban Barn | 2026-09-22 | $40,000 | Retail | Lost after demo | Why? | Clozd | Open |\n\n## Tier 4: win-back\n| Account | Closed | Status |\n|---|---|---|\n| Skip Me | 2026-01-01 | Open |\n\n## Tier 5: churned\n\n| Account | Churned | ARR | CSM reason | Competitor named | Ask | Status |\n|---|---|---|---|---|---|---|\n| Acme Co. | 2026-08-01 | $75,000 | pricing | | q | Open |\n";

    #[test]
    fn parses_tiers_and_skips_win_back() {
        let r = parse_wishlist(SAMPLE);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].kind.as_str(), r[0].account.as_str(), r[0].amount.as_str()), ("loss", "Urban Barn", "$40,000"));
        assert_eq!(r[0].key, "urban-barn-2026-09-22");
        assert_eq!((r[1].kind.as_str(), r[1].why.as_str()), ("churn", "pricing"));
    }

    #[test]
    fn markers_round_trip() {
        let d = std::env::temp_dir().join(format!("hub-voc-{}", std::process::id()));
        std::fs::create_dir_all(d.join("win-loss")).unwrap();
        std::fs::write(d.join("win-loss/interview-wishlist.md"), SAMPLE).unwrap();
        set_marker(&d, "urban-barn-2026-09-22", "claim", "Zac").unwrap();
        set_marker(&d, "acme-co-2026-08-01", "declined", "Deal too small").unwrap();
        let p = load(&d);
        assert_eq!(p.voc[0].claimed_by, "Zac");
        assert_eq!(p.voc[1].declined, "Deal too small");
        set_marker(&d, "urban-barn-2026-09-22", "claim", "").unwrap();
        assert_eq!(load(&d).voc[0].claimed_by, "");
        assert!(set_marker(&d, "../x", "claim", "a").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
