//! Limited-data audit trail for category corrections, on the shared drive.
//!
//! One append-only file per user (`transcript-audit/events/<slug>.jsonl`) so OneDrive never
//! makes conflict copies. Events are queued locally first and flushed, so an unreachable
//! shared drive never loses one. Never logged: transcript text, attendee names, file paths, email.

use crate::transcripts::{title_norm, Entry};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts: String,
    pub user: String,
    pub hub_version: String,
    pub rules_version: u32,
    /// "manual_change" or "auto_other"
    pub event: String,
    pub day: String,
    pub title_norm: String,
    /// One-way hash of the normalized title; groups a series without naming attendees.
    pub series_hash: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auto_source: Option<String>,
    pub speaker_bucket: &'static str,
    pub duration_bucket: &'static str,
    pub reviewed: bool,
}

fn speaker_bucket(n: usize) -> &'static str {
    match n {
        0..=1 => "1",
        2 => "2",
        3..=5 => "3-5",
        6..=10 => "6-10",
        _ => "11+",
    }
}

fn duration_bucket(min: u32) -> &'static str {
    match min {
        0..=14 => "<15",
        15..=29 => "15-30",
        30..=59 => "30-60",
        _ => "60+",
    }
}

pub fn series_hash(title_norm: &str) -> String {
    Sha256::digest(title_norm.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

pub struct Audit {
    root: PathBuf,
    slug: String,
    pending: PathBuf,
    hub_version: String,
    rules_version: u32,
}

#[derive(Debug, Serialize, Clone)]
pub struct Status {
    pub enabled: bool,
    pub user: Option<String>,
    pub pending: usize,
    pub stuck_days: u32,
    pub events_file: Option<String>,
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{}T{:02}:{:02}:{:02}Z", crate::transcripts::today_utc(), secs % 86400 / 3600, secs % 3600 / 60, secs % 60)
}

impl Audit {
    /// None unless the shared vault and this user's inbox slug (written by Second Brain onboarding) both exist.
    pub fn new(sb_root: Option<&Path>, slug: Option<String>, pending_dir: &Path, hub_version: &str, rules_version: u32) -> Option<Audit> {
        let root = sb_root?.join("transcript-audit");
        let slug = slug?.trim().to_string();
        if slug.is_empty() || !sb_root?.is_dir() {
            return None;
        }
        Some(Audit { root, slug, pending: pending_dir.join("audit-pending.jsonl"), hub_version: hub_version.into(), rules_version })
    }

    pub fn event(&self, kind: &str, e: &Entry, from: Option<&str>, to: Option<&str>, auto_source: Option<&str>) -> Event {
        let tn = title_norm(&e.title);
        Event {
            ts: now_iso(),
            user: self.slug.clone(),
            hub_version: self.hub_version.clone(),
            rules_version: self.rules_version,
            event: kind.into(),
            day: crate::transcripts::today_utc(),
            series_hash: series_hash(&tn),
            title_norm: tn,
            from: from.map(Into::into),
            to: to.map(Into::into),
            auto_source: auto_source.map(Into::into),
            speaker_bucket: speaker_bucket(e.speakers.len()),
            duration_bucket: duration_bucket(e.duration_min),
            reviewed: false,
        }
    }

    pub fn events_file(&self) -> PathBuf {
        self.root.join("events").join(format!("{}.jsonl", self.slug))
    }

    fn pending_lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.pending).map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect()).unwrap_or_default()
    }

    /// Queue locally first, so nothing is lost if the shared drive is unreachable.
    pub fn queue(&self, events: &[Event]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        if let Some(dir) = self.pending.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.pending)?;
        for e in events {
            writeln!(f, "{}", serde_json::to_string(e)?)?;
        }
        Ok(())
    }

    /// Appends queued events to this user's shared file. On any failure the queue stays intact.
    pub fn flush(&self) -> Result<usize> {
        let lines = self.pending_lines();
        if lines.is_empty() {
            return Ok(0);
        }
        let file = self.events_file();
        std::fs::create_dir_all(file.parent().unwrap()).context("creating events dir")?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&file).context("opening events file")?;
        for l in &lines {
            writeln!(f, "{l}")?;
        }
        f.flush()?;
        std::fs::write(&self.pending, "")?;
        Ok(lines.len())
    }

    /// Dashboard heartbeat: `last_log_write` survives scans that wrote nothing.
    pub fn heartbeat(&self, wrote: bool) -> Result<()> {
        let dir = self.root.join("heartbeat");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", self.slug));
        let prev: serde_json::Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let now = now_iso();
        let last_write = if wrote { serde_json::Value::from(now.clone()) } else { prev.get("last_log_write").cloned().unwrap_or(serde_json::Value::Null) };
        let pending = self.pending_lines();
        let oldest = pending.first().and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok()).and_then(|v| v.get("ts").cloned());
        let hb = serde_json::json!({
            "user": self.slug, "last_scan": now, "last_log_write": last_write,
            "pending_flush": pending.len(), "oldest_pending_ts": oldest,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&hb)?).context("writing heartbeat")
    }

    pub fn status(&self) -> Status {
        let pending = self.pending_lines();
        let today_days = days_since_epoch(&crate::transcripts::today_utc());
        let stuck_days = pending
            .first()
            .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .and_then(|v| v.get("day").and_then(|d| d.as_str().map(String::from)))
            .map_or(0, |d| today_days.saturating_sub(days_since_epoch(&d)) as u32);
        Status { enabled: true, user: Some(self.slug.clone()), pending: pending.len(), stuck_days, events_file: self.events_file().exists().then(|| self.events_file().to_string_lossy().to_string()) }
    }
}

pub fn disabled_status() -> Status {
    Status { enabled: false, user: None, pending: 0, stuck_days: 0, events_file: None }
}

fn days_since_epoch(date: &str) -> u64 {
    let p: Vec<i64> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    let [y, m, d] = p[..] else { return 0 };
    // days_from_civil
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            title: "Acme Weekly".into(),
            speakers: vec!["Secret Name".into(), "Other Person".into()],
            duration_min: 42,
            category: "Other".into(),
            category_source: "fallback".into(),
            confidence: "low".into(),
            ..Default::default()
        }
    }

    fn audit(tmp: &Path) -> Audit {
        Audit::new(Some(&tmp.join("sb")), Some("zac-wolf\n".into()), &tmp.join("data"), "0.7.0", 1).unwrap()
    }

    #[test]
    fn event_carries_only_limited_fields() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sb")).unwrap();
        let a = audit(tmp.path());
        let ev = a.event("manual_change", &entry(), Some("Other"), Some("Kaizen Meetings"), Some("fallback"));
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("Secret Name") && !json.contains("Other Person"), "no attendee names");
        assert_eq!((ev.speaker_bucket, ev.duration_bucket, ev.user.as_str()), ("2", "30-60", "zac-wolf"));
        assert_eq!(ev.title_norm, "acme weekly");
        let auto = serde_json::to_string(&a.event("auto_other", &entry(), None, None, None)).unwrap();
        assert!(!auto.contains("\"from\"") && !auto.contains("auto_source"));
    }

    #[test]
    fn unreachable_drive_queues_then_flushes_and_heartbeats() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = tmp.path().join("sb");
        std::fs::create_dir(&sb).unwrap();
        let a = audit(tmp.path());
        // Block the events dir with a plain file: flush must fail and keep the queue.
        std::fs::create_dir_all(sb.join("transcript-audit")).unwrap();
        std::fs::write(sb.join("transcript-audit/events"), "blocker").unwrap();
        a.queue(&[a.event("auto_other", &entry(), None, None, None)]).unwrap();
        assert!(a.flush().is_err());
        assert_eq!(a.status().pending, 1);

        std::fs::remove_file(sb.join("transcript-audit/events")).unwrap();
        assert_eq!(a.flush().unwrap(), 1);
        assert_eq!(a.status().pending, 0);
        let body = std::fs::read_to_string(a.events_file()).unwrap();
        assert_eq!(body.lines().count(), 1);
        assert_eq!(a.flush().unwrap(), 0, "second flush appends nothing");

        a.heartbeat(true).unwrap();
        a.heartbeat(false).unwrap();
        let hb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(sb.join("transcript-audit/heartbeat/zac-wolf.json")).unwrap()).unwrap();
        assert!(hb["last_log_write"].is_string(), "last write survives an idle scan");
        assert_eq!(hb["pending_flush"], 0);
    }

    #[test]
    fn disabled_without_slug_or_vault() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(Audit::new(None, Some("x".into()), tmp.path(), "v", 1).is_none());
        assert!(Audit::new(Some(tmp.path()), None, tmp.path(), "v", 1).is_none());
        assert!(Audit::new(Some(&tmp.path().join("missing")), Some("x".into()), tmp.path(), "v", 1).is_none());
    }

    #[test]
    fn day_math_matches_calendar() {
        assert_eq!(days_since_epoch("1970-01-01"), 0);
        assert_eq!(days_since_epoch("2026-10-06") - days_since_epoch("2026-10-03"), 3);
    }
}
