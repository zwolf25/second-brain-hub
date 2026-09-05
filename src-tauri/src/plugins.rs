use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct PluginDrift {
    pub key: String,
    pub installed: String,
    pub vault: String,
}

#[derive(Deserialize)]
struct InstalledPlugins {
    #[serde(default)]
    plugins: HashMap<String, Vec<InstalledEntry>>,
}

#[derive(Deserialize)]
struct InstalledEntry {
    version: Option<String>,
}

#[derive(Deserialize)]
struct PluginManifest {
    version: Option<String>,
}

/// Same comparison as `second-brain-inbox-check.js`'s `checkPluginDrift()`: for every
/// `<plugin>@sc-pm-skills` entry Claude Code has installed, compare its version against
/// the shared vault's own copy of that plugin's manifest at
/// `<shared_vault_root>/skills-repo/<plugin>/.claude-plugin/plugin.json`.
pub fn check_drift(shared_vault_root: &Path, installed_plugins_path: &Path) -> Vec<PluginDrift> {
    let Ok(raw) = std::fs::read_to_string(installed_plugins_path) else {
        return vec![];
    };
    let Ok(installed) = serde_json::from_str::<InstalledPlugins>(&raw) else {
        return vec![];
    };

    let mut drifted: Vec<PluginDrift> = installed
        .plugins
        .into_iter()
        .filter_map(|(key, entries)| {
            let (plugin_name, marketplace) = key.split_once('@')?;
            if marketplace != "sc-pm-skills" {
                return None;
            }
            let manifest_raw = std::fs::read_to_string(
                shared_vault_root
                    .join("skills-repo")
                    .join(plugin_name)
                    .join(".claude-plugin/plugin.json"),
            )
            .ok()?;
            let vault_version = serde_json::from_str::<PluginManifest>(&manifest_raw)
                .ok()?
                .version?;
            let installed_version = entries.first()?.version.clone()?;
            (installed_version != vault_version).then_some(PluginDrift {
                key,
                installed: installed_version,
                vault: vault_version,
            })
        })
        .collect();

    drifted.sort_by(|a, b| a.key.cmp(&b.key));
    drifted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_version_mismatch_and_skips_other_marketplaces() {
        let vault = tempfile::tempdir().unwrap();
        let manifest_dir = vault.path().join("skills-repo/behind-plugin/.claude-plugin");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(manifest_dir.join("plugin.json"), r#"{"version": "1.2.0"}"#).unwrap();

        let installed = vault.path().join("installed_plugins.json");
        std::fs::write(
            &installed,
            r#"{"plugins": {
                "behind-plugin@sc-pm-skills": [{"version": "1.1.0"}],
                "current-plugin@sc-pm-skills": [{"version": "1.0.0"}],
                "other@some-other-marketplace": [{"version": "0.1.0"}]
            }}"#,
        )
        .unwrap();
        // current-plugin has no manifest on disk here, so it's skipped, not flagged —
        // only behind-plugin (a real version mismatch) should surface.

        let drift = check_drift(vault.path(), &installed);
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].key, "behind-plugin@sc-pm-skills");
        assert_eq!(drift[0].installed, "1.1.0");
        assert_eq!(drift[0].vault, "1.2.0");
    }
}
