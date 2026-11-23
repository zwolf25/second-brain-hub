#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
//! OneDrive health probe for collaborators (port of the ZacAI Dashboard's `scan_onedrive`).
//!
//! macOS: process + sign-in + local sync DB (pending count) + an upload canary whose server eTag
//! revision proves a real round trip. Windows: process + sign-in + folder only; its sync state is
//! in proprietary files, so the upload test is reported as unavailable. Every probe fails open to
//! "can't verify" (yellow), never to a false green or red.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

const CANARY_NAME: &str = ".hub-onedrive-canary"; // distinct from the ZacAI Dashboard's own canary
const CANARY_REWRITE_AFTER: u64 = 900; // s: rewrite once the last write is confirmed
const CANARY_RED_AFTER: u64 = 600; // s: write never reached the server

#[cfg(target_os = "windows")]
const ICON_PLACE: &str = "system tray (bottom right, near the clock)";
#[cfg(not(target_os = "windows"))]
const ICON_PLACE: &str = "menu bar (top right)";

#[derive(Debug, Clone, PartialEq)]
pub enum Canary {
    /// No inbox folder or no DB on this platform: the upload test can't run.
    Unavailable,
    Ok,
    /// Written recently, server revision not caught up yet.
    Verifying,
    Failed(u64),
    /// Canary is old but its server revision can't be read.
    Blind,
}

#[derive(Debug, Clone)]
pub struct Inputs {
    pub running: bool,
    pub signed_in: bool,
    pub folder_ok: bool,
    /// None = unreadable (blind), only meaningful where a sync DB exists.
    pub pending: Option<u32>,
    pub canary: Canary,
    pub limited: bool,
}

#[derive(Serialize, Clone)]
pub struct Issue {
    pub code: &'static str,
    /// "red" | "yellow" | "info"
    pub severity: &'static str,
    pub title: String,
    pub detail: String,
    pub steps: Vec<String>,
    pub can_open: bool,
}

#[derive(Serialize, Clone)]
pub struct Check {
    pub label: String,
    /// "g" ok | "y" warn | "r" bad | "x" not checked
    pub state: &'static str,
}

#[derive(Serialize, Clone)]
pub struct Status {
    /// "ok" | "yellow" | "red"
    pub worst: &'static str,
    pub issues: Vec<Issue>,
    pub checks: Vec<Check>,
    pub limited: bool,
    pub checked_at: u64,
    pub platform: &'static str,
}

fn issue(code: &'static str, severity: &'static str, title: &str, detail: &str, steps: Vec<String>) -> Issue {
    Issue { code, severity, title: title.into(), detail: detail.into(), steps, can_open: true }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Pure: turns raw probe results into issues + checklist. All user-facing wording lives here.
pub fn evaluate(i: &Inputs, now: u64) -> Status {
    let mut issues = vec![];
    let mut checks = vec![];
    let place = ICON_PLACE;

    if !i.running {
        issues.push(issue(
            "not_running",
            "red",
            "The OneDrive app is not running",
            "Nothing syncs until OneDrive is open.",
            vec![
                "Open OneDrive from Applications (or the Start menu on Windows).".into(),
                format!("Wait for the cloud icon to appear in the {place}."),
                "Press Refresh here.".into(),
            ],
        ));
    } else if !i.signed_in {
        issues.push(issue(
            "signed_out",
            "red",
            "OneDrive is signed out",
            "OneDrive is running but not signed in to your Fortive account.",
            vec![
                format!("Click the OneDrive cloud icon in the {place} and choose Sign in."),
                "Use your Fortive email and password.".into(),
                "Press Refresh here.".into(),
            ],
        ));
    }
    if !i.folder_ok {
        issues.push(issue(
            "folder_missing",
            "red",
            "Second Brain folder not found",
            "The Hub can't find the shared \"SVC-Product_Management - Second Brain\" folder in OneDrive.",
            s(&[
                "Open the shared library in your browser and choose \"Add shortcut to My files\".",
                "Wait for the folder to appear under OneDrive in your file browser.",
                "Press Refresh. If it never appears, use \"Copy details\" and send them to Zac.",
            ]),
        ));
    }
    if let Canary::Failed(mins) = i.canary {
        issues.push(issue(
            "uploads_failing",
            "red",
            "Uploads are not reaching OneDrive",
            &format!("The Hub's test file was written {mins} minutes ago and never confirmed. Changes you make may not reach teammates, and you may be missing theirs."),
            vec![
                format!("Click the OneDrive cloud icon in the {place}."),
                "If it says \"Sign in\" or shows a warning, sign in again. This is common after a Fortive password change.".into(),
                "If it says \"Syncing paused\", choose Resume.".into(),
                "Wait a minute, then press Refresh here.".into(),
            ],
        ));
    }
    if let Some(n) = i.pending.filter(|n| *n > 0) {
        issues.push(issue(
            "pending",
            "yellow",
            &format!("{n} item(s) waiting to sync"),
            "OneDrive has changes it has not finished syncing. This usually clears on its own.",
            vec![
                "Wait about 15 minutes, then press Refresh.".into(),
                format!("If it stays, click the OneDrive icon in the {place} and look for \"Syncing paused\" or \"Storage full\"."),
            ],
        ));
    }
    let blind = i.canary == Canary::Blind || (i.pending.is_none() && !i.limited && i.running && i.signed_in);
    if blind {
        issues.push(issue(
            "blind",
            "yellow",
            "Can't verify sync status",
            "The Hub could not read OneDrive's status (an update may have changed it). Your files may be fine; this just can't be confirmed.",
            vec![
                format!("Click the OneDrive cloud icon in the {place} and check it says \"Up to date\"."),
                "If it does, you can ignore this. Use \"Copy details\" if it keeps showing.".into(),
            ],
        ));
    }
    if i.canary == Canary::Verifying {
        issues.push(Issue {
            code: "verifying",
            severity: "info",
            title: "Verifying uploads…".into(),
            detail: "The Hub just wrote a test file and is waiting for OneDrive to confirm it. Check again in a few minutes.".into(),
            steps: vec![],
            can_open: false,
        });
    }
    if i.limited {
        issues.push(Issue {
            code: "limited",
            severity: "info",
            title: "Limited checks on Windows".into(),
            detail: format!("The Hub confirms OneDrive is running, signed in and your folder is present. It can't yet tell whether files are uploading, so a quiet failure would not show here. If teammates stop seeing your changes, open the OneDrive icon in the {place} and look for warnings."),
            steps: vec![],
            can_open: false,
        });
    }

    checks.push(Check { label: "OneDrive app running".into(), state: if i.running { "g" } else { "r" } });
    checks.push(Check {
        label: "Signed in to your Fortive account".into(),
        state: if !i.running { "x" } else if i.signed_in { "g" } else { "r" },
    });
    checks.push(Check { label: "Second Brain folder present".into(), state: if i.folder_ok { "g" } else { "r" } });
    checks.push(match &i.canary {
        _ if i.limited => Check { label: "Upload test: not available on Windows".into(), state: "x" },
        Canary::Unavailable => Check { label: "Upload test: not run".into(), state: "x" },
        Canary::Ok => Check { label: "Upload test passed".into(), state: "g" },
        Canary::Verifying => Check { label: "Upload test: verifying".into(), state: "x" },
        Canary::Failed(m) => Check { label: format!("Upload test failed ({m} min)"), state: "r" },
        Canary::Blind => Check { label: "Upload test: unreadable".into(), state: "y" },
    });

    let worst = if issues.iter().any(|x| x.severity == "red") {
        "red"
    } else if issues.iter().any(|x| x.severity == "yellow") {
        "yellow"
    } else {
        "ok"
    };
    Status { worst, issues, checks, limited: i.limited, checked_at: now, platform: std::env::consts::OS }
}

/// Pure canary state machine. `age` = seconds since the canary file was written, `expected` = the
/// server revision it asked for, `current` = the server revision the sync DB reports now.
/// Returns (state, rewrite_now).
pub fn canary_state(age: Option<u64>, expected: Option<i64>, current: Option<i64>) -> (Canary, bool) {
    let rewrite_due = age.is_none_or(|a| a > CANARY_REWRITE_AFTER);
    if age.is_none() {
        return (Canary::Verifying, true); // just written: nothing proven yet
    }
    if let (Some(a), Some(e), Some(c)) = (age, expected, current) {
        if c < e {
            return (if a > CANARY_RED_AFTER { Canary::Failed(a / 60) } else { Canary::Verifying }, false);
        }
    }
    if age.is_some_and(|a| a > CANARY_RED_AFTER) && current.is_none() {
        return (Canary::Blind, rewrite_due);
    }
    (Canary::Ok, rewrite_due)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Runs a probe command with no console window flash on Windows.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(cmd);
    c.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = c.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(target_os = "macos")]
fn is_running() -> bool {
    run("pgrep", &["-f", "OneDrive Sync Service"]).is_some()
}
#[cfg(target_os = "windows")]
fn is_running() -> bool {
    run("tasklist", &["/FI", "IMAGENAME eq OneDrive.exe", "/NH"]).is_some_and(|o| o.to_lowercase().contains("onedrive.exe"))
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn is_running() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn is_signed_in() -> bool {
    run("defaults", &["read", "com.microsoft.OneDrive", "IsBusinessProvisioned"]).is_some_and(|o| o.trim() == "1")
}
#[cfg(target_os = "windows")]
fn is_signed_in() -> bool {
    // A signed-in business account has a UserEmail value under Accounts\Business*.
    run("reg", &["query", r"HKCU\Software\Microsoft\OneDrive\Accounts", "/s", "/v", "UserEmail"]).is_some_and(|o| o.contains("UserEmail"))
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn is_signed_in() -> bool {
    false
}

/// The OneDrive account slot that is actually syncing: the settings dir whose DB was written most
/// recently. Collaborators' slot is not always `Business1`.
#[cfg(target_os = "macos")]
fn sync_db() -> Option<PathBuf> {
    let base = std::env::var_os("HOME").map(PathBuf::from)?.join("Library/Application Support/OneDrive/settings");
    pick_newest_db(&base)
}

fn pick_newest_db(base: &Path) -> Option<PathBuf> {
    std::fs::read_dir(base)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let db = e.path().join("SyncEngineDatabase.db");
            let m = [db.clone(), e.path().join("SyncEngineDatabase.db-wal")]
                .iter()
                .filter_map(|p| p.metadata().ok()?.modified().ok())
                .max()?;
            db.is_file().then_some((m, db))
        })
        .max_by_key(|(m, _)| *m)
        .map(|(_, db)| db)
}

#[cfg(target_os = "macos")]
fn sql(db: &Path, q: &str) -> Option<String> {
    run("sqlite3", &["-readonly", "-cmd", ".timeout 3000", &db.to_string_lossy(), q])
}

#[cfg(target_os = "macos")]
fn pending_count(db: &Path) -> Option<u32> {
    let q = "SELECT (SELECT COUNT(*) FROM od_ClientFilePostponedChange_Records)+(SELECT COUNT(*) FROM od_ClientFolderPostponedChange_Records);";
    sql(db, q)?.trim().parse().ok()
}

/// Every collaborator's inbox holds a file with this name, so match on the parent folder too.
#[cfg(target_os = "macos")]
fn canary_revision(db: &Path, owner: &str) -> Option<i64> {
    let owner = owner.replace('\'', "''");
    let q = format!(
        "SELECT f.eTag FROM od_ClientFile_Records f JOIN od_ClientFolder_Records p ON p.resourceID=f.parentResourceID \
         WHERE f.fileName='{CANARY_NAME}' AND p.folderName='{owner}';"
    );
    parse_revision(&sql(db, &q)?)
}

/// `"{GUID},190"` -> 190. Anything but exactly one row is ambiguous, so None (reads as "can't verify", never red).
fn parse_revision(out: &str) -> Option<i64> {
    let mut rows = out.lines().map(str::trim).filter(|l| !l.is_empty());
    let row = rows.next()?;
    if rows.next().is_some() {
        return None;
    }
    row.trim_matches('"').rsplit(',').next()?.parse().ok()
}

fn dir_nonempty(p: &Path) -> bool {
    std::fs::read_dir(p).is_ok_and(|d| d.flatten().any(|e| !e.file_name().to_string_lossy().starts_with('.')))
}

/// Writes/reads the canary in the user's own inbox folder. Returns the state; never raises.
#[cfg(target_os = "macos")]
fn probe_canary(inbox: Option<&Path>, db: Option<&Path>) -> Canary {
    let (Some(inbox), Some(db)) = (inbox, db) else { return Canary::Unavailable };
    let file = inbox.join(CANARY_NAME);
    let now = now_secs();
    let parsed = std::fs::read_to_string(&file).ok().and_then(|t| {
        let mut p = t.split_whitespace();
        Some((p.next()?.parse::<u64>().ok()?, p.next()?.parse::<i64>().ok()?))
    });
    let (age, expected) = match parsed {
        Some((ts, e)) => (Some(now.saturating_sub(ts)), Some(e)),
        None => (None, None),
    };
    let current = inbox.file_name().and_then(|n| n.to_str()).and_then(|n| canary_revision(db, n));
    let (state, rewrite) = canary_state(age, expected, current);
    if rewrite && inbox.is_dir() {
        let _ = std::fs::write(&file, format!("{now} {}\n", current.unwrap_or(0) + 1));
    }
    state
}

/// Gathers inputs from this machine. `vault` = configured wikis folder, `inbox` = the user's own inbox folder.
pub fn probe(vault: Option<&Path>, inbox: Option<&Path>) -> Status {
    let running = is_running();
    let signed_in = running && is_signed_in();
    let folder_ok = vault
        .and_then(|v| v.parent().map(Path::to_path_buf))
        .or_else(crate::config::autodetect_second_brain_root)
        .is_some_and(|p| dir_nonempty(&p));

    #[cfg(target_os = "macos")]
    let (pending, canary, limited) = if running && signed_in {
        match sync_db() {
            Some(db) => (pending_count(&db), probe_canary(inbox, Some(&db)), false),
            None => (None, Canary::Blind, false),
        }
    } else {
        (Some(0), Canary::Unavailable, false)
    };
    #[cfg(not(target_os = "macos"))]
    let (pending, canary, limited) = {
        let _ = inbox;
        (Some(0), Canary::Unavailable, true)
    };

    #[allow(unused_mut)]
    let mut inputs = Inputs { running, signed_in, folder_ok, pending, canary, limited };
    #[cfg(debug_assertions)]
    debug_force(&mut inputs);
    evaluate(&inputs, now_secs())
}

/// Debug builds only (compiled out of releases): the first word in `<tmp>/hub-onedrive-force` forces a
/// state so every alert can be seen without breaking OneDrive. Delete the file to go back to real probes.
#[cfg(debug_assertions)]
fn debug_force(i: &mut Inputs) {
    let Ok(t) = std::fs::read_to_string(std::env::temp_dir().join("hub-onedrive-force")) else { return };
    match t.split_whitespace().next().unwrap_or("") {
        "not_running" => (i.running, i.signed_in) = (false, false),
        "signed_out" => (i.running, i.signed_in) = (true, false),
        "folder_missing" => i.folder_ok = false,
        "uploads" => (i.running, i.signed_in, i.canary) = (true, true, Canary::Failed(14)),
        "pending" => (i.running, i.signed_in, i.pending) = (true, true, Some(14)),
        "blind" => (i.running, i.signed_in, i.canary) = (true, true, Canary::Blind),
        "verifying" => (i.running, i.signed_in, i.canary) = (true, true, Canary::Verifying),
        "windows" => (i.limited, i.canary) = (true, Canary::Unavailable),
        _ => {}
    }
}

/// Brings the OneDrive app forward so the user can see its own status icon/menu.
pub fn open_app() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let r = Command::new("open").args(["-a", "OneDrive"]).status();
    #[cfg(target_os = "windows")]
    let r = {
        use std::os::windows::process::CommandExt;
        Command::new("cmd").args(["/C", "start", "", "onedrive"]).creation_flags(0x0800_0000).status()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let r: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::other("unsupported platform"));
    match r {
        Ok(st) if st.success() => Ok(()),
        Ok(_) => Err("OneDrive could not be opened. Open it from your Applications folder or Start menu.".into()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Inputs {
        Inputs { running: true, signed_in: true, folder_ok: true, pending: Some(0), canary: Canary::Ok, limited: false }
    }

    #[test]
    fn healthy_is_ok() {
        let s = evaluate(&healthy(), 1);
        assert_eq!(s.worst, "ok");
        assert!(s.issues.is_empty());
    }

    #[test]
    fn not_running_is_red_and_hides_signed_out() {
        let s = evaluate(&Inputs { running: false, signed_in: false, ..healthy() }, 1);
        assert_eq!(s.worst, "red");
        assert_eq!(s.issues.len(), 1);
        assert_eq!(s.issues[0].code, "not_running");
    }

    #[test]
    fn signed_out_missing_folder_and_failed_upload_are_red() {
        assert_eq!(evaluate(&Inputs { signed_in: false, ..healthy() }, 1).issues[0].code, "signed_out");
        assert_eq!(evaluate(&Inputs { folder_ok: false, ..healthy() }, 1).issues[0].code, "folder_missing");
        assert_eq!(evaluate(&Inputs { canary: Canary::Failed(14), ..healthy() }, 1).issues[0].code, "uploads_failing");
    }

    #[test]
    fn pending_and_blind_are_yellow_never_red() {
        assert_eq!(evaluate(&Inputs { pending: Some(14), ..healthy() }, 1).worst, "yellow");
        assert_eq!(evaluate(&Inputs { pending: None, ..healthy() }, 1).worst, "yellow");
        assert_eq!(evaluate(&Inputs { canary: Canary::Blind, ..healthy() }, 1).worst, "yellow");
    }

    #[test]
    fn verifying_and_windows_limited_are_info_only() {
        assert_eq!(evaluate(&Inputs { canary: Canary::Verifying, ..healthy() }, 1).worst, "ok");
        let w = evaluate(&Inputs { canary: Canary::Unavailable, limited: true, pending: Some(0), ..healthy() }, 1);
        assert_eq!(w.worst, "ok");
        assert!(w.limited && w.issues.iter().any(|i| i.code == "limited"));
    }

    #[test]
    fn canary_thresholds() {
        assert_eq!(canary_state(None, None, Some(3)), (Canary::Verifying, true)); // first run: write one, nothing proven yet
        assert_eq!(canary_state(Some(60), Some(4), Some(3)), (Canary::Verifying, false));
        assert_eq!(canary_state(Some(660), Some(4), Some(3)), (Canary::Failed(11), false));
        assert_eq!(canary_state(Some(1000), Some(4), Some(4)), (Canary::Ok, true));
        assert_eq!(canary_state(Some(700), Some(4), None), (Canary::Blind, false));
        assert_eq!(canary_state(Some(30), Some(4), None), (Canary::Ok, false)); // too young to judge
    }

    #[test]
    fn parses_exactly_one_revision_row() {
        assert_eq!(parse_revision("\"{504A},190\"\n"), Some(190));
        assert_eq!(parse_revision(""), None);
        // several collaborators' canaries -> ambiguous, never picks one
        assert_eq!(parse_revision("\"{A},190\"\n\"{B},9\"\n\"{C},1\"\n"), None);
    }

    #[test]
    fn picks_the_slot_with_the_newest_db() {
        let d = tempfile::tempdir().unwrap();
        for slot in ["Business1", "Business2"] {
            std::fs::create_dir_all(d.path().join(slot)).unwrap();
        }
        std::fs::write(d.path().join("Business1/SyncEngineDatabase.db"), "x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(d.path().join("Business2/SyncEngineDatabase.db"), "x").unwrap();
        assert!(pick_newest_db(d.path()).unwrap().to_string_lossy().contains("Business2"));
        assert!(pick_newest_db(&d.path().join("nope")).is_none());
    }

    /// Manual: `cargo test live_probe -- --ignored --nocapture` prints this machine's real status (writes the canary into your own inbox folder).
    #[test]
    #[ignore]
    fn live_probe() {
        let inbox = crate::config::autodetect_inbox();
        println!("inbox={inbox:?}");
        let s = probe(None, inbox.as_deref());
        println!("worst={} limited={}", s.worst, s.limited);
        for c in &s.checks {
            println!("[{}] {}", c.state, c.label);
        }
        for i in &s.issues {
            println!("({}) {}: {}", i.severity, i.code, i.title);
        }
    }
}
