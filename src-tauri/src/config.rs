use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub vault_path: Option<PathBuf>,
    #[serde(default)]
    pub raw_path: Option<PathBuf>,
    #[serde(default)]
    pub inbox_path: Option<PathBuf>,
    #[serde(default)]
    pub shared_raw_path: Option<PathBuf>,
    /// Folder of Teams `.vtt` transcripts. `None` = feature off: nothing converts, watches or writes.
    #[serde(default)]
    pub transcripts_path: Option<PathBuf>,
    /// "Not now" on the Transcripts empty state; no nudges while set.
    #[serde(default)]
    pub transcripts_dismissed: bool,
    #[serde(default)]
    pub hide_transcripts_page: bool,
    /// Start at login is turned on once when transcripts are first set up; after that it is the user's choice.
    #[serde(default)]
    pub autostart_default_applied: bool,
    /// The user confirmed that automatic VoC copies transcripts (with attendee names) into the shared vault.
    #[serde(default)]
    pub voc_consent: bool,
    /// The user dismissed the one-time "add future transcripts automatically?" offer.
    #[serde(default)]
    pub raw_stub_offer_dismissed: bool,
    /// Old `log-it` rules were given a `since` date once, so upgrading never mass-writes raw notes.
    #[serde(default)]
    pub raw_stub_migrated: bool,
    /// Ordered rules run at the end of each transcripts scan; the first matching rule claims a file.
    #[serde(default)]
    pub auto_rules: Vec<AutoRule>,
}

/// One automatic action on converted transcripts, all run inside the Hub (no scheduled Claude task):
/// "move" relocates the `.md`, "log-it" (shown as "Add to Second Brain"; id kept for config compatibility)
/// writes a raw-note stub, "voc" flags the row for a user-triggered VoC run.
/// `mode` is "include" or "exclude" over `categories`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoRule {
    pub id: String,
    pub action: String,
    pub enabled: bool,
    pub dest: Option<PathBuf>,
    pub mode: String,
    pub categories: Vec<String>,
    /// Only files converted at least this many days ago (0 = right away).
    pub after_days: u64,
    /// Only files converted on or after this ISO date (`None` = no lower bound).
    #[serde(default)]
    pub since: Option<String>,
}

fn config_file(config_dir: &Path) -> PathBuf {
    config_dir.join("config.json")
}

pub fn load(config_dir: &Path) -> AppConfig {
    let path = config_file(config_dir);
    // Only a config that parsed is migrated and re-saved; an unreadable one is never overwritten.
    let Some(mut cfg) = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<AppConfig>(&s).ok()) else {
        return AppConfig::default();
    };
    if !cfg.raw_stub_migrated {
        let today = crate::transcripts::today_utc();
        cfg.auto_rules.iter_mut().filter(|r| r.action == "log-it" && r.since.is_none()).for_each(|r| r.since = Some(today.clone()));
        cfg.raw_stub_migrated = true;
        let _ = save(config_dir, &cfg);
    }
    cfg
}

pub fn save(config_dir: &Path, config: &AppConfig) -> Result<()> {
    std::fs::create_dir_all(config_dir).context("creating config dir")?;
    let path = config_file(config_dir);
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(path, json).context("writing config.json")
}

/// Vault folder contains at least one .md file.
pub fn is_valid_vault(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|e| {
        e.path()
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("md"))
            .unwrap_or(false)
    })
}

/// Probe likely OneDrive locations for the shared SC Second Brain root folder.
/// Paths differ per collaborator (username, OS) — this only covers the common case;
/// callers fall back to a manual folder picker in the UI when it finds nothing.
pub fn autodetect_second_brain_root() -> Option<PathBuf> {
    const NAME: &str = "SVC-Product_Management - Second Brain";
    onedrive_roots().into_iter().find_map(|od| {
        let direct = od.join(NAME);
        if direct.is_dir() {
            return Some(direct);
        }
        // Windows SharePoint shortcuts can nest under `Documents - <library>`.
        std::fs::read_dir(&od).ok()?.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("Documents")).find_map(|e| {
            let nested = e.path().join(NAME);
            nested.is_dir().then_some(nested)
        })
    })
}

/// Every OneDrive sync root (any tenant name): macOS `~/Library/CloudStorage/OneDrive-*`,
/// Windows `%USERPROFILE%\OneDrive*`. Sorted so the pick is stable.
pub(crate) fn onedrive_roots() -> Vec<PathBuf> {
    let Some(home) = dirs_home() else { return vec![] };
    let mut roots = vec![];
    for base in [home.join("Library/CloudStorage"), home] {
        let Ok(entries) = std::fs::read_dir(&base) else { continue };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().to_lowercase().contains("onedrive") && e.path().is_dir())
            .map(|e| e.path())
            .collect();
        found.sort();
        roots.extend(found);
    }
    roots
}

pub fn autodetect_vault() -> Option<PathBuf> {
    let candidate = autodetect_second_brain_root()?.join("wikis");
    (candidate.is_dir() && is_valid_vault(&candidate)).then_some(candidate)
}

/// The raw-notes badge triggers `/wiki-builder` — the *personal* skill, which
/// scans a hardcoded path under the user's own Claude projects folder, not
/// the shared OneDrive vault. The two are easy to conflate (both are named
/// `raw/`, and the shared Second Brain vault is even symlinked *into* the
/// personal one for browsing) — `wiki-builder`'s own SKILL.md explicitly
/// warns about this exact mixup. Earlier versions of this function wrongly
/// built on `autodetect_second_brain_root()` (the shared/OneDrive probe) and
/// found the shared team `raw/` folder instead of the personal one.
///
/// This path is `wiki-builder`'s own hardcoded scan target, not a Zac-specific
/// guess — any collaborator who completed the standard Second Brain onboarding
/// has real files here (Zac's happens to be a symlink into his personal ZacAI
/// project, but the literal path below resolves the same for everyone who set
/// up wiki-builder normally).
pub fn autodetect_raw() -> Option<PathBuf> {
    raw_candidates().into_iter().find(|p| p.is_dir())
}

/// In order: the vault-root override (`SECOND_BRAIN_VAULT_ROOT`, from the environment or
/// `~/.claude/settings.json` `env` since a Finder-launched app doesn't see `.zshrc`), the
/// standard `~/Documents` path, then the same path inside each OneDrive root (Known Folder
/// Move redirects `Documents` there, see ONBOARDING.md Section 1).
fn raw_candidates() -> Vec<PathBuf> {
    const TAIL: &str = "Claude/Projects/Second Brain/Second Brain Obsidian/raw";
    let Some(home) = dirs_home() else { return vec![] };
    let mut out = vec![];
    let override_root = std::env::var_os("SECOND_BRAIN_VAULT_ROOT").map(PathBuf::from).or_else(|| {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).ok()?).ok()?;
        Some(PathBuf::from(v.get("env")?.get("SECOND_BRAIN_VAULT_ROOT")?.as_str()?))
    });
    if let Some(root) = override_root {
        let root = if root.starts_with("~") { home.join(root.strip_prefix("~").unwrap_or(&root).strip_prefix("/").unwrap_or(Path::new(""))) } else { root };
        out.push(root.join("Second Brain Obsidian/raw"));
    }
    out.push(home.join("Documents").join(TAIL));
    out.extend(onedrive_roots().into_iter().map(|od| od.join("Documents").join(TAIL)));
    out
}

/// Needs the local per-machine inbox slug (written by the shared Second Brain's
/// own onboarding) to know which subfolder is "ours" — returns None if that
/// hasn't been set up yet, same as the `inbox-review` skill's own fallback.
pub fn autodetect_inbox() -> Option<PathBuf> {
    let home = dirs_home()?;
    let inbox = autodetect_second_brain_root()?.join("inbox");
    let marked = std::fs::read_to_string(home.join(".claude/second-brain-inbox/inbox-slug")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    marked.map(|slug| inbox.join(slug)).filter(|p| p.is_dir()).or_else(|| inbox_by_email(&home, &inbox))
}

/// No slug marker (or a stale one): match the Claude account email against the
/// `email:` line of each `inbox/_roster/<slug>.md` profile.
fn inbox_by_email(home: &Path, inbox: &Path) -> Option<PathBuf> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).ok()?).ok()?;
    let email = v.get("oauthAccount")?.get("emailAddress")?.as_str()?.to_lowercase();
    std::fs::read_dir(inbox.join("_roster")).ok()?.flatten().find_map(|e| {
        let text = std::fs::read_to_string(e.path()).ok()?;
        let has = text.lines().any(|l| l.trim().to_lowercase().strip_prefix("email:").is_some_and(|v| v.trim().trim_matches('"') == email));
        let dir = inbox.join(e.path().file_stem()?);
        (has && dir.is_dir()).then_some(dir)
    })
}

/// Unlike `autodetect_raw` (the *personal* wiki-builder raw folder — a
/// different thing, see that function's own comment), this points at the
/// shared vault's own `raw/` folder — every teammate's unprocessed
/// contributions, not just this user's. Purely informational (see the
/// shared-raw badge in `lib.rs`/`main.js`): there's nothing a viewer can do
/// about someone else's pending files, it's just a freshness signal for the
/// wikis they're reading.
pub fn autodetect_shared_raw() -> Option<PathBuf> {
    let candidate = autodetect_second_brain_root()?.join("raw");
    candidate.is_dir().then_some(candidate)
}

pub(crate) fn dirs_home() -> Option<PathBuf> {
    // Windows: USERPROFILE is the real profile; HOME is often set by Git Bash/MSYS to something else.
    let (first, second) = if cfg!(windows) { ("USERPROFILE", "HOME") } else { ("HOME", "USERPROFILE") };
    std::env::var_os(first).or_else(|| std::env::var_os(second)).map(PathBuf::from)
}

#[cfg(test)]
mod autodetect_tests {
    use super::*;

    #[test]
    fn inbox_matches_roster_email_when_slug_marker_missing() {
        let t = tempfile::tempdir().unwrap();
        let inbox = t.path().join("inbox");
        std::fs::create_dir_all(inbox.join("_roster")).unwrap();
        std::fs::create_dir_all(inbox.join("jane-doe")).unwrap();
        std::fs::write(inbox.join("_roster/jane-doe.md"), "---\nname: Jane Doe\nslug: jane-doe\nemail: Jane@x.com\n---\n").unwrap();
        std::fs::write(t.path().join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"jane@x.com"}}"#).unwrap();
        assert_eq!(inbox_by_email(t.path(), &inbox), Some(inbox.join("jane-doe")));
    }
}
