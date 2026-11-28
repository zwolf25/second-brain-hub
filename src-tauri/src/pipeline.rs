//! Pipeline page data: the staleness JSON (passed through as text) and the VoC backlog.
//! The backlog is the Salesforce opportunity sidecar `win-loss/voc-candidates.json` (the wishlist only supplies a status); Claim and Dismiss state lives in one small
//! marker file per candidate under `win-loss/_voc/` (owner decision 2026-11-23: the Hub writes these markers).
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Contact {
    pub name: Option<String>,
    pub title: Option<String>,
    pub email: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct VocRow {
    pub key: String,
    pub kind: String, // loss | win | churn
    #[serde(rename = "type")]
    pub vtype: String, // new_logo | renewal | expansion | services | churn; empty for rows only in the wishlist
    pub account: String,
    pub date: String,
    pub amount: String,
    pub why: String,
    pub status: String,
    pub claimed_by: String,
    pub requested_by: String,
    pub declined: String,
    // detail from win-loss/voc-candidates.json (owner sync); empty for wishlist-only rows
    pub opp_name: String,
    pub acv: Option<f64>,
    pub acv_source: String,
    pub amount_flag: bool,
    pub raw_amount: Option<f64>,
    pub sfdc_url: String,
    pub locations: Option<f64>,
    pub reason_text: String,
    pub competitor: String,
    pub sales_rep: String,
    pub csm: String,
    pub contact: Option<Contact>,
    pub current_customer: bool,
}

/// One row of `win-loss/voc-candidates.json` (written by the owner's SFDC sync).
#[derive(Deserialize, Default)]
#[serde(default)]
struct Cand {
    key: String,
    kind: String,
    #[serde(rename = "type")]
    vtype: String,
    account: String,
    closed: String,
    opp_name: Option<String>,
    acv: Option<f64>,
    acv_source: Option<String>,
    amount_flag: bool,
    amount: Option<f64>,
    sfdc_url: Option<String>,
    locations: Option<f64>,
    reason_code: Option<String>,
    reason_text: Option<String>,
    competitor: Option<String>,
    sales_rep: Option<String>,
    csm: Option<String>,
    contact: Option<Contact>,
    current_customer: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Sidecar {
    rows: Vec<Cand>,
}

fn row_from(c: Cand) -> VocRow {
    VocRow {
        key: c.key,
        kind: c.kind,
        vtype: c.vtype,
        account: c.account,
        date: c.closed,
        amount: c.acv.map(|a| format!("${a:.0}")).unwrap_or_default(),
        why: c.reason_code.unwrap_or_default(),
        opp_name: c.opp_name.unwrap_or_default(),
        acv: c.acv,
        acv_source: c.acv_source.unwrap_or_default(),
        amount_flag: c.amount_flag,
        raw_amount: c.amount,
        sfdc_url: c.sfdc_url.unwrap_or_default(),
        locations: c.locations,
        reason_text: c.reason_text.unwrap_or_default(),
        competitor: c.competitor.unwrap_or_default(),
        sales_rep: c.sales_rep.unwrap_or_default(),
        csm: c.csm.unwrap_or_default(),
        contact: c.contact,
        current_customer: c.current_customer,
        ..Default::default()
    }
}

#[derive(Serialize)]
pub struct PipelineData {
    pub status_json: Option<String>,
    pub voc: Vec<VocRow>,
    /// Output name from the status JSON -> its main markdown file (only files that exist).
    pub paths: std::collections::HashMap<String, String>,
    /// PMD folder name -> PM / PMM, read from `pmds/_assignments.json` (owner-maintained from Confluence).
    pub assignments: std::collections::HashMap<String, Assignment>,
    /// Confluence page's modified date when `_assignments.json` was built.
    pub assignments_as_of: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Assignment {
    #[serde(default)]
    pub pm: Vec<String>,
    #[serde(default)]
    pub pmm: Vec<String>,
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub check: bool,
}

/// Where an output named in the status JSON lives in the shared vault.
fn output_file(root: &Path, name: &str) -> Option<std::path::PathBuf> {
    if name.contains("..") || name.contains('\\') {
        return None; // names come from a shared file; never let one climb out of the vault
    }
    let rel = match name {
        "Win/Loss/Churn report" => "win-loss/report/win-loss-report.md".to_string(),
        "Landscape map" => "competitors/landscape-map/landscape-map.md".to_string(),
        _ => match (name.strip_prefix("PMD: "), name.strip_prefix("Battlecard: ")) {
            (Some(slug), _) if !slug.contains('/') => format!("pmds/{slug}/PMD.md"),
            (_, Some(c)) if !c.contains('/') => format!("competitors/{c}/battlecard.md"),
            _ => return None,
        },
    };
    let p = root.join(rel);
    p.is_file().then_some(p)
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
            why: match col(&["Gate"]) {
                g if !g.is_empty() => format!("Current customer, active-customer gate: {g}"),
                _ => col(&["What the seller view says", "Why", "CSM reason"]),
            },
            status: col(&["Status"]),
            ..Default::default()
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

/// Loss reasons with nothing to learn from an interview (owner decision 2026-11-23).
const AUTO_DECLINE: [&str; 2] = ["Prospect Went Dark", "Timing"];

pub fn load(root: &Path) -> PipelineData {
    let status_json = std::fs::read_to_string(root.join("_status").join("intel-staleness.json")).ok();
    let paths = status_json
        .as_deref()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
        .and_then(|v| v["outputs"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|o| o["name"].as_str())
        .filter_map(|n| output_file(root, n).map(|p| (n.to_string(), p.to_string_lossy().into_owned())))
        .collect();
    let asg_json = std::fs::read_to_string(root.join("pmds").join("_assignments.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let assignments = asg_json
        .as_ref()
        .and_then(|v| serde_json::from_value(v["pmds"].clone()).ok())
        .unwrap_or_default();
    let assignments_as_of = asg_json
        .as_ref()
        .and_then(|v| v["source_modified"].as_str())
        .unwrap_or_default()
        .to_string();
    let wishlist = std::fs::read_to_string(root.join("win-loss").join("interview-wishlist.md"))
        .map(|t| parse_wishlist(&t))
        .unwrap_or_default();
    let side: Sidecar = std::fs::read_to_string(root.join("win-loss").join("voc-candidates.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    // The backlog is Salesforce opportunities only (the sidecar and its criteria); the wishlist just supplies a status.
    let mut voc: Vec<VocRow> = side.rows.into_iter().map(row_from).collect();
    for r in &mut voc {
        r.status = wishlist.iter().find(|w| w.key == r.key).map(|w| w.status.clone()).unwrap_or_default();
    }
    let dir = root.join("win-loss").join("_voc");
    for r in &mut voc {
        r.claimed_by = marker_field(&dir.join(format!("{}.claim.json", r.key)), "by");
        r.requested_by = marker_field(&dir.join(format!("{}.requested.json", r.key)), "by");
        r.declined = marker_field(&dir.join(format!("{}.declined.json", r.key)), "reason");
        if r.declined.is_empty() && AUTO_DECLINE.contains(&r.why.as_str()) {
            r.declined = format!("Auto: {}", r.why); // derived, no marker file; Hub hides Undo for these
        }
    }
    PipelineData { status_json, voc, paths, assignments, assignments_as_of }
}

/// Reruns the staleness detector and replaces `_status/intel-staleness.json`, on the owner's machine only
/// (`c4`, the crawl4ai folder, exists). `Ok(false)`: not the owner, nothing ran. On `Err` the old JSON is left alone.
/// Runs the detector alone, not `pipeline_status.py`: that shares temp names with its scheduled run, can double a
/// ledger line, and exits 0 when the detector fails.
pub fn recheck(root: &Path, c4: &Path, timeout: Duration) -> Result<bool, String> {
    if !c4.is_dir() {
        return Ok(false);
    }
    let dir = root.join("_status");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Own temp name, so a scheduled run (`.intel-staleness.json.tmp`) can never collide with this one.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!(".intel-staleness.hub-{}-{nanos}.tmp", std::process::id()));
    let result = run_detector(root, c4, &tmp, timeout)
        .and_then(|()| check_output(&tmp))
        .and_then(|()| std::fs::rename(&tmp, dir.join("intel-staleness.json")).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| true)
}

/// stdout goes straight to the temp file, so a large result can never fill a pipe and stall the wait loop.
fn run_detector(root: &Path, c4: &Path, out: &Path, timeout: Duration) -> Result<(), String> {
    let file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    // Same interpreter as the com.sb.pipeline.status LaunchAgent; a GUI app's PATH is not the shell's.
    let mut child = Command::new(c4.join(".venv/bin/python"))
        .arg("-I")
        .arg(root.join("skills-repo/sc-win-loss/shared/intel-staleness.py"))
        .arg("--json")
        .env("SECOND_BRAIN_ROOT", root)
        .env("CRAWL4AI_DIR", c4)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start Python: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) if s.success() => return Ok(()),
            Some(s) => return Err(format!("the staleness script failed ({s})")),
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the staleness script took longer than {} s", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Only a complete owner-side result may replace the saved one.
fn check_output(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("the staleness script returned bad JSON: {e}"))?;
    let complete = v["generated"].is_string() && v["outputs"].is_array() && v["queues"].is_array() && v["mtime_checks_skipped"] == false;
    complete.then_some(()).ok_or_else(|| "the staleness script returned an incomplete result".to_string())
}

/// kind: "claim" | "requested" | "declined"; `value` empty removes the marker (Release / Undo).
pub fn set_marker(root: &Path, key: &str, kind: &str, value: &str) -> Result<(), String> {
    if !matches!(kind, "claim" | "requested" | "declined") || key.is_empty() || key.contains(['/', '\\', '.']) {
        return Err("bad marker".into());
    }
    let dir = root.join("win-loss").join("_voc");
    let path = dir.join(format!("{key}.{kind}.json"));
    if value.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let field = if kind == "declined" { "reason" } else { "by" };
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
    fn output_paths_and_assignments_load_only_what_exists() {
        let d = tempfile::tempdir().unwrap();
        let w = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        w("_status/intel-staleness.json", r#"{"outputs":[{"name":"PMD: easypay"},{"name":"PMD: ghost"},{"name":"Battlecard: Corrigo"},{"name":"Win/Loss/Churn report"},{"name":"Landscape map"}]}"#);
        w("pmds/easypay/PMD.md", "x");
        w("competitors/Corrigo/battlecard.md", "x");
        w("win-loss/report/win-loss-report.md", "x");
        w("pmds/_assignments.json", r#"{"source_modified":"2026-11-20","pmds":{"easypay":{"pm":["Lisa Pei"],"pmm":["Kaycie Kaeding"],"check":true}}}"#);
        let p = load(d.path());
        let mut names: Vec<_> = p.paths.keys().cloned().collect();
        names.sort();
        assert_eq!(names, ["Battlecard: Corrigo", "PMD: easypay", "Win/Loss/Churn report"]);
        assert_eq!(p.assignments["easypay"].pm, ["Lisa Pei"]);
        assert!(p.assignments["easypay"].check);
        assert_eq!(p.assignments_as_of, "2026-11-20");
        assert!(output_file(d.path(), "PMD: ../../etc").is_none());
        w("competitors/a/b/battlecard.md", "x");
        assert!(output_file(d.path(), "Battlecard: a/b").is_none());
    }

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
        let side = r#"{"rows":[{"key":"urban-barn-2026-09-22","kind":"loss","type":"new_logo","account":"Urban Barn","closed":"2026-09-22"},{"key":"acme-co-2026-08-01","kind":"loss","type":"new_logo","account":"Acme Co.","closed":"2026-08-01"}]}"#;
        std::fs::write(d.join("win-loss/voc-candidates.json"), side).unwrap();
        set_marker(&d, "urban-barn-2026-09-22", "claim", "Zac").unwrap();
        set_marker(&d, "acme-co-2026-08-01", "declined", "Deal too small").unwrap();
        let p = load(&d);
        assert_eq!(p.voc[0].claimed_by, "Zac");
        assert_eq!(p.voc[1].declined, "Deal too small");
        set_marker(&d, "urban-barn-2026-09-22", "claim", "").unwrap();
        assert_eq!(load(&d).voc[0].claimed_by, "");
        assert!(set_marker(&d, "../x", "claim", "a").is_err());
        set_marker(&d, "urban-barn-2026-09-22", "requested", "Amy").unwrap();
        assert_eq!(load(&d).voc[0].requested_by, "Amy");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sidecar_rows_win_over_wishlist_and_keep_status() {
        let d = std::env::temp_dir().join(format!("hub-voc-side-{}", std::process::id()));
        std::fs::create_dir_all(d.join("win-loss")).unwrap();
        std::fs::write(d.join("win-loss/interview-wishlist.md"), SAMPLE).unwrap();
        let side = r#"{"rows":[{"key":"urban-barn-2026-09-22","kind":"loss","type":"new_logo","account":"Urban Barn","closed":"2026-09-22",
            "acv":42000,"acv_source":"opportunity amount","reason_code":"Sales","csm":"Pat","contact":{"name":"Lee","title":"VP","email":"l@x.com"}}]}"#;
        std::fs::write(d.join("win-loss/voc-candidates.json"), side).unwrap();
        let p = load(&d);
        assert_eq!(p.voc.len(), 1); // Salesforce rows only; the wishlist-only Acme row is not shown
        assert_eq!((p.voc[0].vtype.as_str(), p.voc[0].csm.as_str(), p.voc[0].status.as_str()), ("new_logo", "Pat", "Open"));
        assert_eq!(p.voc[0].contact.as_ref().unwrap().email.as_deref(), Some("l@x.com"));
        let side2 = r#"{"rows":[{"key":"k-2026-10-01","kind":"loss","type":"new_logo","account":"K","closed":"2026-10-01","reason_code":"Timing","amount":9000000,"amount_flag":true,"sfdc_url":"https://x/o"}]}"#;
        std::fs::write(d.join("win-loss/voc-candidates.json"), side2).unwrap();
        let p2 = load(&d);
        assert_eq!(p2.voc[0].declined, "Auto: Timing");
        assert_eq!((p2.voc[0].raw_amount, p2.voc[0].sfdc_url.as_str()), (Some(9000000.0), "https://x/o"));
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Fixture vault and fake crawl4ai folder only; never the live shared vault. `.venv/bin/python` is a link to the system Python.
#[cfg(all(test, unix))]
mod recheck_tests {
    use super::*;

    const OLD: &str = r#"{"generated":"2020-01-01T00:00:00Z","outputs":[]}"#;
    const GOOD: &str = r#"print('{"generated":"2026-11-27T00:00:00Z","outputs":[],"queues":[],"mtime_checks_skipped":false}')"#;

    fn system_python() -> std::path::PathBuf {
        std::env::split_paths(&std::env::var_os("PATH").unwrap()).map(|d| d.join("python3")).find(|p| p.is_file()).expect("python3 on PATH")
    }

    fn fixture(script: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let (root, c4) = (d.path().join("vault"), d.path().join("crawl4ai"));
        std::fs::create_dir_all(root.join("_status")).unwrap();
        std::fs::create_dir_all(root.join("skills-repo/sc-win-loss/shared")).unwrap();
        std::fs::create_dir_all(c4.join(".venv/bin")).unwrap();
        std::os::unix::fs::symlink(system_python(), c4.join(".venv/bin/python")).unwrap();
        std::fs::write(root.join("skills-repo/sc-win-loss/shared/intel-staleness.py"), script).unwrap();
        std::fs::write(root.join("_status/intel-staleness.json"), OLD).unwrap();
        (d, root, c4)
    }

    fn saved(root: &Path) -> String {
        std::fs::read_to_string(root.join("_status/intel-staleness.json")).unwrap()
    }

    fn status_files(root: &Path) -> usize {
        std::fs::read_dir(root.join("_status")).unwrap().count()
    }

    #[test]
    fn success_replaces_the_json_and_leaves_no_temp_file() {
        let (_d, root, c4) = fixture(GOOD);
        assert_eq!(recheck(&root, &c4, Duration::from_secs(30)), Ok(true));
        assert!(saved(&root).contains("2026-11-27"));
        assert_eq!(status_files(&root), 1);
    }

    #[test]
    fn every_failure_keeps_the_old_json_and_leaves_no_temp_file() {
        for (script, secs) in [
            ("print('{'); raise SystemExit(1)", 30),
            ("print('not json')", 30),
            ("print('{}')", 30),
            (r#"print('{"generated":"x","outputs":[],"queues":null,"mtime_checks_skipped":true}')"#, 30),
            ("import time; time.sleep(30)", 1),
        ] {
            let (_d, root, c4) = fixture(script);
            assert!(recheck(&root, &c4, Duration::from_secs(secs)).is_err(), "{script}");
            assert_eq!(saved(&root), OLD, "{script}");
            assert_eq!(status_files(&root), 1, "{script}");
        }
        let (_d, root, c4) = fixture(GOOD);
        std::fs::remove_file(c4.join(".venv/bin/python")).unwrap();
        assert!(recheck(&root, &c4, Duration::from_secs(30)).is_err());
        assert_eq!(saved(&root), OLD);
    }

    #[test]
    fn without_crawl4ai_nothing_runs() {
        let (d, root, _c4) = fixture("open(__file__ + '.ran', 'w').close()");
        assert_eq!(recheck(&root, &d.path().join("missing"), Duration::from_secs(30)), Ok(false));
        assert!(!root.join("skills-repo/sc-win-loss/shared/intel-staleness.py.ran").exists());
        assert_eq!(saved(&root), OLD);
    }

    /// Acceptance run against the real detector (it lives in the vault, so CI does not have it):
    /// HUB_DETECTOR=<path to intel-staleness.py> cargo test real_detector -- --ignored
    #[test]
    #[ignore]
    fn real_detector_clears_a_stale_battlecard_after_review() {
        let script = std::fs::read_to_string(std::env::var("HUB_DETECTOR").expect("set HUB_DETECTOR")).unwrap();
        let (_d, root, c4) = fixture(&script);
        for sub in ["win-loss/records", "churn/records", "competitors/Acme"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        for i in 0..11 {
            std::fs::write(root.join(format!("win-loss/records/r{i}.md")), "---\nprimary_competitor: \"Acme\"\n---\n").unwrap();
        }
        let card = |date: &str| std::fs::write(root.join("competitors/Acme/battlecard.md"), format!("# B\n\nLast reviewed: {date}\n")).unwrap();
        let stale = || {
            assert_eq!(recheck(&root, &c4, Duration::from_secs(60)), Ok(true));
            let v: serde_json::Value = serde_json::from_str(&saved(&root)).unwrap();
            v["outputs"].as_array().unwrap().iter().find(|o| o["name"] == "Battlecard: Acme").unwrap()["stale"].as_bool().unwrap()
        };
        card("2020-01-01");
        assert!(stale());
        card("2999-01-01");
        assert!(!stale());
    }
}
