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

#[cfg(test)]
mod tests {
    use super::*;

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
