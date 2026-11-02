use anyhow::{bail, Context, Result};
use pulldown_cmark::{html, Event, Options, Parser};
use serde::Serialize;
use std::path::Path;

use crate::frontmatter;

#[derive(Debug, Serialize)]
pub struct RenderedDoc {
    pub html: String,
    pub title: String,
    pub source_path: String,
}

/// Reads a `.md`/`.markdown` file, strips frontmatter and renders the body to HTML.
/// `[[wikilinks]]` become `<a class="wikilink" data-target=…>` (the page resolves and
/// opens them); code blocks and inline code keep their brackets. No syntax highlighting.
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

    let (_, body) = frontmatter::split_frontmatter(&raw);
    let html_out = markdown_to_html(body);

    Ok(RenderedDoc { html: html_out, title: parsed.title, source_path: path.to_string_lossy().to_string() })
}

fn markdown_to_html(body: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let linked = link_wikilinks(body);
    let parser = Parser::new_ext(&linked, options).filter_map(sanitize_event);
    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);
    html_out
}

/// Raw HTML in a vault file is shown as text, not rendered (the CSP blocks scripts, but external
/// images and misleading links would still render). Only the anchors `link_wikilinks` writes pass
/// through; HTML comments (e.g. `<!-- wiki-processed -->`) are dropped.
fn sanitize_event(ev: Event) -> Option<Event> {
    match ev {
        Event::Html(s) | Event::InlineHtml(s) => {
            let t = s.trim();
            if t.starts_with("<!--") {
                None
            } else if is_wikilink_tag(t) || t == "</a>" {
                Some(Event::InlineHtml(s))
            } else {
                Some(Event::Text(s))
            }
        }
        other => Some(other),
    }
}

/// Exactly the open tag `link_line` emits (target is `esc`aped, so it holds no quote or angle bracket).
fn is_wikilink_tag(t: &str) -> bool {
    const HEAD: &str = "<a class=\"wikilink\" data-target=\"";
    const TAIL: &str = "\" href=\"#\">";
    t.len() >= HEAD.len() + TAIL.len()
        && t.starts_with(HEAD)
        && t.ends_with(TAIL)
        && !t[HEAD.len()..t.len() - TAIL.len()].contains(['"', '<', '>'])
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `[[target|alias]]` / `[[target#Heading]]` -> anchor; `[[#Heading]]` -> plain text.
fn link_line(seg: &str) -> String {
    let mut out = String::new();
    let mut rest = seg;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let (target_part, alias) = match inner.split_once('|') {
            Some((t, a)) => (t, Some(a)),
            None => (inner, None),
        };
        let target = target_part.split('#').next().unwrap_or("").trim();
        let label = esc(alias.unwrap_or(if target.is_empty() { inner.trim_start_matches('#') } else { inner }).trim());
        out.push_str(&rest[..start]);
        if target.is_empty() {
            out.push_str(&label);
        } else {
            out.push_str(&format!("<a class=\"wikilink\" data-target=\"{}\" href=\"#\">{label}</a>", esc(target)));
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Turns wikilinks into anchors, skipping fenced code blocks and inline code spans.
pub fn link_wikilinks(body: &str) -> String {
    let mut fenced = false;
    let mut lines = Vec::new();
    for line in body.split('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            lines.push(line.to_string());
        } else if fenced {
            lines.push(line.to_string());
        } else {
            let mut out = String::new();
            for (i, seg) in line.split('`').enumerate() {
                if i > 0 {
                    out.push('`');
                }
                out.push_str(&if i % 2 == 0 { link_line(seg) } else { seg.to_string() });
            }
            lines.push(out);
        }
    }
    lines.join("\n")
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
    fn raw_html_is_shown_as_text_but_wikilinks_still_link() {
        let h = markdown_to_html("See [[Other Page|other]].\n\n<img src=\"https://x.test/p.gif\"> and <script>alert(1)</script>\n\n<!-- wiki-processed -->\n\nuse <date> here\n");
        assert!(h.contains("<a class=\"wikilink\" data-target=\"Other Page\" href=\"#\">other</a>"), "{h}");
        assert!(!h.contains("<img") && !h.contains("<script"), "{h}");
        assert!(h.contains("&lt;img") && h.contains("&lt;script&gt;"), "{h}");
        assert!(!h.contains("wiki-processed"), "{h}");
        assert!(h.contains("&lt;date&gt;"), "{h}");
    }

    #[test]
    fn forged_wikilink_tag_with_extra_attributes_is_escaped() {
        let h = markdown_to_html("<a class=\"wikilink\" data-target=\"x\" href=\"#\" onclick=\"evil()\">go</a>");
        assert!(!h.contains("<a class"), "{h}"); // tag is text, so onclick is inert
        assert!(h.contains("&lt;a class"), "{h}");
    }

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
        assert!(doc.html.contains(r##"<a class="wikilink" data-target="Other Page" href="#">Other Page</a>"##));
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

    #[test]
    fn wikilinks_become_anchors_except_in_code() {
        let out = link_wikilinks("A [[x|Label <b>]] and [[y#Head]] and [[#Self]].\n`[[code]]`\n```\n[[fenced]]\n```");
        assert!(out.contains(r##"<a class="wikilink" data-target="x" href="#">Label &lt;b&gt;</a>"##));
        assert!(out.contains(r#"data-target="y""#));
        assert!(out.contains("and Self."), "same-page anchors become plain text");
        assert!(out.contains("`[[code]]`") && out.contains("[[fenced]]"));
    }
}
