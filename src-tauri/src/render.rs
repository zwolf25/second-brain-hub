use anyhow::{bail, Context, Result};
use pulldown_cmark::{html, Options, Parser};
use serde::Serialize;
use std::path::Path;

use crate::frontmatter;

#[derive(Debug, Serialize)]
pub struct RenderedDoc {
    pub html: String,
    pub title: String,
    pub source_path: String,
}

/// Reads a `.md`/`.markdown` file, strips frontmatter and wikilink brackets
/// (reusing `frontmatter::parse`, same as the search indexer), and renders
/// the body to HTML. No wikilink navigation, no syntax highlighting — MVP
/// viewer scope, matches the search snippet's plain-text bracket stripping.
pub fn render_file(path: &Path) -> Result<RenderedDoc> {
    let is_markdown = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false);
    if !is_markdown {
        bail!("Not a markdown file: {}", path.display());
    }

    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let filename_fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled");
    let parsed = frontmatter::parse(&raw, filename_fallback);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(&parsed.body, options);
    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);

    Ok(RenderedDoc { html: html_out, title: parsed.title, source_path: path.to_string_lossy().to_string() })
}

#[derive(Debug, Serialize)]
pub struct EditDoc {
    /// Verbatim frontmatter block incl. `---` fences ("" if none) — hidden from
    /// the editor and written back untouched.
    pub header: String,
    /// Raw markdown body, wikilinks intact.
    pub body: String,
}

fn ensure_markdown(path: &Path) -> Result<()> {
    let ok = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false);
    if !ok {
        bail!("Not a markdown file: {}", path.display());
    }
    Ok(())
}

pub fn read_for_edit(path: &Path) -> Result<EditDoc> {
    ensure_markdown(path)?;
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let (_, body) = frontmatter::split_frontmatter(&raw);
    // `body` is a suffix of `raw` (BOM aside), so the header is whatever precedes it.
    let trimmed = raw.trim_start_matches('\u{feff}');
    let header = trimmed[..trimmed.len() - body.len()].to_string();
    Ok(EditDoc { header, body: body.to_string() })
}

/// Atomic write (tmp + rename) so OneDrive never syncs a half-written file.
/// Tmp name starts with `~$` so the indexer's transient-file filter skips it.
pub fn write_for_edit(path: &Path, header: &str, body: &str) -> Result<()> {
    ensure_markdown(path)?;
    let name = path.file_name().and_then(|n| n.to_str()).context("no filename")?;
    // rename() replaces a read-only file without complaint, so check explicitly.
    if std::fs::metadata(path).map(|m| m.permissions().readonly()).unwrap_or(false) {
        bail!("{} is read-only — you don't have permission to edit it.", path.display());
    }
    let tmp = path.with_file_name(format!("~$hub-{name}"));
    // Always end with exactly one newline (POSIX/git convention); the textarea doesn't add one.
    std::fs::write(&tmp, format!("{header}{}\n", body.trim_end())).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!("saving {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_roundtrip_preserves_frontmatter_and_wikilinks() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.md");
        let original = "---\ntype: wiki\n---\n# T\n\nSee [[Other Page]].\n";
        std::fs::write(&file, original).unwrap();

        let doc = read_for_edit(&file).unwrap();
        assert_eq!(doc.header, "---\ntype: wiki\n---\n");
        assert!(doc.body.contains("[[Other Page]]"));

        write_for_edit(&file, &doc.header, &doc.body).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);

        write_for_edit(&file, &doc.header, "# T\n\nchanged\n").unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("---\ntype: wiki\n---\n# T"));
        assert!(!dir.path().join("~$hub-doc.md").exists(), "tmp file must not linger");

        write_for_edit(&file, &doc.header, "# T\n\nno newline\n\n\n").unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().ends_with("no newline\n"));
        write_for_edit(&file, &doc.header, "# T\n\nbare").unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().ends_with("bare\n"));
    }

    #[test]
    fn write_refuses_readonly_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ro.md");
        std::fs::write(&file, "# keep\n").unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms).unwrap();

        assert!(write_for_edit(&file, "", "# overwritten\n").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# keep\n");
        assert!(!dir.path().join("~$hub-ro.md").exists());
    }

    #[test]
    fn edit_without_frontmatter_and_non_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("n.md");
        std::fs::write(&file, "# plain\n").unwrap();
        let doc = read_for_edit(&file).unwrap();
        assert_eq!(doc.header, "");
        assert_eq!(doc.body, "# plain\n");
        assert!(write_for_edit(&dir.path().join("x.txt"), "", "hi").is_err());
    }

    #[test]
    fn renders_gfm_surface_and_strips_wikilinks() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.md");
        std::fs::write(
            &file,
            "---\ntype: wiki\n---\n\
             # A Title\n\n\
             See [[Other Page]] for more.\n\n\
             | a | b |\n|---|---|\n| 1 | 2 |\n\n\
             - [ ] todo\n- [x] done\n\n\
             Deleted ~~text~~ here.[^1]\n\n\
             [^1]: a footnote.\n",
        )
        .unwrap();

        let doc = render_file(&file).unwrap();

        assert_eq!(doc.title, "A Title");
        assert!(!doc.html.contains("[["), "wikilink brackets should be stripped");
        assert!(!doc.html.contains("]]"));
        assert!(doc.html.contains("<table"), "tables should render");
        assert!(doc.html.contains("checkbox"), "task lists should render as checkboxes");
        assert!(doc.html.contains("<del>"), "strikethrough should render");
        assert!(doc.html.to_lowercase().contains("footnote"), "footnotes should render");
    }

    #[test]
    fn rejects_non_markdown_extension() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.txt");
        std::fs::write(&file, "hello").unwrap();
        assert!(render_file(&file).is_err());
    }
}
