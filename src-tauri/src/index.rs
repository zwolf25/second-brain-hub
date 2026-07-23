use crate::frontmatter;
use anyhow::{Context, Result};
use std::path::Path;
use tantivy::schema::{Field, Schema, STORED, STRING, TEXT};
use tantivy::{doc, Index, IndexWriter};
use walkdir::WalkDir;

#[derive(Clone, Copy)]
pub struct Fields {
    pub path: Field,
    pub title: Field,
    pub body: Field,
    pub topics: Field,
    pub updated: Field,
}

pub fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let path = builder.add_text_field("path", STRING | STORED);
    let title = builder.add_text_field("title", TEXT | STORED);
    let body = builder.add_text_field("body", TEXT | STORED);
    let topics = builder.add_text_field("topics", TEXT | STORED);
    let updated = builder.add_text_field("updated", STRING | STORED);
    let schema = builder.build();
    (schema, Fields { path, title, body, topics, updated })
}

/// Full rebuild: wipes and recreates the on-disk index, then walks the vault
/// and indexes every .md file. Simple over incremental — fine at vault sizes
/// this small (under a second for ~100 files).
pub fn rebuild(index_dir: &Path, vault_path: &Path) -> Result<usize> {
    if index_dir.exists() {
        std::fs::remove_dir_all(index_dir).context("clearing old index dir")?;
    }
    std::fs::create_dir_all(index_dir).context("creating index dir")?;

    let (schema, fields) = build_schema();
    let index = Index::create_in_dir(index_dir, schema).context("creating tantivy index")?;
    let mut writer: IndexWriter = index.writer(50_000_000)?;

    let mut count = 0usize;
    for entry in WalkDir::new(vault_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let Some(ext) = path.extension() else { continue };
        if !ext.eq_ignore_ascii_case("md") {
            continue;
        }
        let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("untitled");
        // Skip OneDrive transient/lock artifacts.
        if filename.starts_with("~$") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(path) else { continue };

        let parsed = frontmatter::parse(&raw, filename);
        let rel_path = path.to_string_lossy().to_string();

        writer.add_document(doc!(
            fields.path => rel_path,
            fields.title => parsed.title,
            fields.body => parsed.body,
            fields.topics => parsed.topics,
            fields.updated => parsed.updated,
        ))?;
        count += 1;
    }

    writer.commit()?;
    Ok(count)
}

pub fn open(index_dir: &Path) -> Result<(Index, Fields)> {
    let (_, fields) = build_schema();
    let index = Index::open_in_dir(index_dir).context("opening tantivy index")?;
    Ok((index, fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search;

    #[test]
    fn rebuild_and_fuzzy_search_round_trip() {
        let vault = tempfile::tempdir().unwrap();
        let index_dir = tempfile::tempdir().unwrap();

        std::fs::write(
            vault.path().join("refrigerant-tracking.md"),
            "---\ntype: wiki\ntopics:\n  - \"[[work-order-management]]\"\nupdated: 2026-07-01T00:00:00Z\n---\n\
             # Refrigerant Tracking\n\nRelated Feature Flags: FF_REFRIGERANT_LOG, FF_EPA_COMPLIANCE.",
        )
        .unwrap();
        std::fs::write(
            vault.path().join("unrelated.md"),
            "---\ntype: wiki\nupdated: 2026-06-01T00:00:00Z\n---\n\
             # Unrelated Topic\n\nNothing about cooling equipment here.",
        )
        .unwrap();

        let count = rebuild(index_dir.path(), vault.path()).unwrap();
        assert_eq!(count, 2);

        let (idx, fields) = open(index_dir.path()).unwrap();

        let results = search::search(&idx, &fields, "refrigerant").unwrap();
        assert!(!results.is_empty(), "expected at least one match for 'refrigerant'");
        assert_eq!(results[0].title, "Refrigerant Tracking");
        assert!(!results[0].snippet.is_empty());

        // Fuzzy: a typo should still match via edit-distance term query.
        let typo_results = search::search(&idx, &fields, "refrigirant").unwrap();
        assert!(!typo_results.is_empty(), "expected fuzzy match despite typo");
        assert_eq!(typo_results[0].title, "Refrigerant Tracking");
    }
}
