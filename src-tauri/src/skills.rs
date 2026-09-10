use crate::frontmatter::split_frontmatter;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub plugin: String,
    pub description: String,
    pub added: String,
}

#[derive(Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
}

/// Walks `<shared_vault_root>/skills-repo/*/skills/*/SKILL.md`, one entry per skill.
/// `plugin` is the path segment right after `skills-repo/`. `added` is the file's
/// mtime, formatted `YYYY-MM-DD`. Unparseable files are skipped, not fatal — same
/// pattern as `plugins::check_drift`.
pub fn list_skills(shared_vault_root: &Path) -> Vec<SkillInfo> {
    // Sort by the raw mtime, not the formatted date string — two skills added
    // the same day would otherwise tie and fall back to filesystem read order.
    let mut skills: Vec<(std::time::SystemTime, SkillInfo)> = glob_skill_files(shared_vault_root)
        .filter_map(|(plugin, path)| {
            let raw = std::fs::read_to_string(&path).ok()?;
            let (yaml, _) = split_frontmatter(&raw);
            let fm: SkillFrontmatter = serde_yaml::from_str(yaml?).ok()?;
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((
                modified,
                SkillInfo {
                    name: fm.name,
                    plugin,
                    description: fm.description,
                    added: format_date(modified),
                },
            ))
        })
        .collect();

    skills.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    skills.into_iter().map(|(_, s)| s).collect()
}

fn glob_skill_files(shared_vault_root: &Path) -> impl Iterator<Item = (String, std::path::PathBuf)> {
    let skills_repo = shared_vault_root.join("skills-repo");
    std::fs::read_dir(&skills_repo)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .flat_map(|plugin_entry| {
            let plugin = plugin_entry.file_name().to_string_lossy().to_string();
            let skills_dir = plugin_entry.path().join("skills");
            std::fs::read_dir(&skills_dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(move |skill_entry| (plugin.clone(), skill_entry.path().join("SKILL.md")))
                .collect::<Vec<_>>()
        })
        .filter(|(_, path)| path.is_file())
}

/// Manual formatting since no date/time crate is a dependency yet — not worth
/// adding one for a single `YYYY-MM-DD` conversion.
fn format_date(modified: std::time::SystemTime) -> String {
    let days_since_epoch = modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86400)
        .unwrap_or(0);

    // Civil-from-days algorithm (Howard Hinnant's), proleptic Gregorian calendar.
    let z = days_since_epoch as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    fn write_skill(dir: &Path, plugin: &str, skill: &str, name: &str, description: &str) {
        let skill_dir = dir.join("skills-repo").join(plugin).join("skills").join(skill);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nbody"),
        )
        .unwrap();
    }

    #[test]
    fn sorts_descending_by_mtime_and_skips_malformed() {
        let vault = tempfile::tempdir().unwrap();
        write_skill(vault.path(), "plugin-a", "older-skill", "older-skill", "does older things");
        sleep(Duration::from_millis(1100)); // mtime granularity is 1s on some filesystems
        write_skill(vault.path(), "plugin-b", "newer-skill", "newer-skill", "does newer things");

        let malformed_dir = vault
            .path()
            .join("skills-repo/plugin-c/skills/broken-skill");
        std::fs::create_dir_all(&malformed_dir).unwrap();
        std::fs::write(malformed_dir.join("SKILL.md"), "no frontmatter here").unwrap();

        let skills = list_skills(vault.path());
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "newer-skill");
        assert_eq!(skills[0].plugin, "plugin-b");
        assert_eq!(skills[1].name, "older-skill");
        assert!(skills[0].added >= skills[1].added);
    }
}
