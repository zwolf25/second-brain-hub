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
}

fn config_file(config_dir: &Path) -> PathBuf {
    config_dir.join("config.json")
}

pub fn load(config_dir: &Path) -> AppConfig {
    let path = config_file(config_dir);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
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
    let home = dirs_home()?;

    let mut candidate_roots = vec![home.join("Library/CloudStorage")]; // macOS
    candidate_roots.push(home.clone()); // Windows: OneDrive folders sit directly under the user profile

    for root in candidate_roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.to_lowercase().contains("onedrive") {
                continue;
            }
            let candidate = entry.path().join("SVC-Product_Management - Second Brain");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
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
    let candidate = dirs_home()?
        .join("Documents/Claude/Projects/Second Brain/Second Brain Obsidian/raw");
    candidate.is_dir().then_some(candidate)
}

/// Needs the local per-machine inbox slug (written by the shared Second Brain's
/// own onboarding) to know which subfolder is "ours" — returns None if that
/// hasn't been set up yet, same as the `inbox-review` skill's own fallback.
pub fn autodetect_inbox() -> Option<PathBuf> {
    let slug_path = dirs_home()?.join(".claude/second-brain-inbox/inbox-slug");
    let slug = std::fs::read_to_string(slug_path).ok()?;
    let slug = slug.trim();
    if slug.is_empty() {
        return None;
    }
    let candidate = autodetect_second_brain_root()?.join("inbox").join(slug);
    candidate.is_dir().then_some(candidate)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
