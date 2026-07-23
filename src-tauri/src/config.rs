use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub vault_path: Option<PathBuf>,
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

/// Probe likely OneDrive locations for the shared SC Second Brain wikis folder.
/// Paths differ per collaborator (username, OS) — this only covers the common case;
/// falls back to a manual folder picker in the UI when it finds nothing.
pub fn autodetect_vault() -> Option<PathBuf> {
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
            let candidate = entry
                .path()
                .join("SVC-Product_Management - Second Brain")
                .join("wikis");
            if candidate.is_dir() && is_valid_vault(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
