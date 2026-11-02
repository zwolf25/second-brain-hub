use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

#[derive(Serialize, Clone)]
pub struct GraphNode {
    id: String,
    words: usize,
    path: String,
    /// Lists child wikis under `## Child wikis`.
    hub: bool,
}

#[derive(Serialize, Clone)]
pub struct GraphEdge {
    source: String,
    target: String,
    /// "child" when the hub lists the other wiki under `## Child wikis`, else "link".
    kind: &'static str,
}

#[derive(Serialize, Clone, Default)]
pub struct GraphData {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

/// [[target]] / [[target|alias]] / [[target#Heading]] / [[target#Heading|alias]]
/// -> target. Same-file anchors ([[#Heading]]) reduce to "" and are dropped.
/// Hand-rolled instead of a regex crate dependency — the vault's own wikilink
/// syntax is simple enough that scanning for `[[`/`]]` pairs covers it, and
/// this mirrors the dashboard's `vault_scan.py` parser exactly.
fn wikilink_targets(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let target = inner.split('|').next().unwrap_or("").split('#').next().unwrap_or("").trim();
        if !target.is_empty() {
            out.insert(target.to_string());
        }
        rest = &after[end + 2..];
    }
    out
}

/// Wikilinks inside the `## Child wikis` section (up to the next `## ` heading). Only a hub has one;
/// children point back through `topics:`, which stays an ordinary link.
fn child_targets(text: &str) -> BTreeSet<String> {
    let mut section = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            inside = line.trim_end().eq_ignore_ascii_case("## Child wikis");
        } else if inside {
            section.push_str(line);
            section.push('\n');
        }
    }
    wikilink_targets(&section)
}

/// Order-independent edge key: a hub-child pair is one edge whichever side links first.
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a < b { (a.into(), b.into()) } else { (b.into(), a.into()) }
}

fn word_count(body: &str) -> usize {
    body.split_whitespace().count()
}

/// id (file stem) -> (path, text) for the vault's top-level wikis (flat layout, like the indexer).
fn read_texts(vault_path: &Path) -> HashMap<String, (String, String)> {
    let mut texts = HashMap::new();
    let Ok(entries) = std::fs::read_dir(vault_path) else {
        return texts;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if stem == "_manifest" || stem.starts_with("~$") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        texts.insert(stem.to_string(), (path.to_string_lossy().into_owned(), text));
    }
    texts
}

/// Hub id -> its existing child ids (from `## Child wikis`). A "family" is the hub plus these.
pub fn families(vault_path: &Path) -> BTreeMap<String, Vec<String>> {
    let texts = read_texts(vault_path);
    let mut out = BTreeMap::new();
    for (id, (_, text)) in &texts {
        let kids: Vec<String> = child_targets(text).into_iter().filter(|c| c != id && texts.contains_key(c)).collect();
        if !kids.is_empty() {
            out.insert(id.clone(), kids);
        }
    }
    out
}

#[derive(Serialize)]
pub struct Backlink {
    pub title: String,
    pub path: String,
}

/// Wikis whose text links to `stem` (case-insensitive, like Obsidian), by title.
pub fn backlinks(vault_path: &Path, stem: &str) -> Vec<Backlink> {
    let mut out: Vec<Backlink> = read_texts(vault_path)
        .into_iter()
        .filter(|(id, (_, text))| !id.eq_ignore_ascii_case(stem) && wikilink_targets(text).iter().any(|t| t.eq_ignore_ascii_case(stem)))
        .map(|(id, (path, text))| Backlink { title: crate::frontmatter::parse(&text, &id).title, path })
        .collect();
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    out
}

/// Path of the wiki a `[[target]]` points at (exact stem first, then case-insensitive).
pub fn resolve(vault_path: &Path, target: &str) -> Option<String> {
    let mut fallback = None;
    for entry in std::fs::read_dir(vault_path).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem.starts_with("~$") {
            continue;
        }
        if stem == target {
            return Some(path.to_string_lossy().into_owned());
        }
        if stem.eq_ignore_ascii_case(target) {
            fallback = Some(path.to_string_lossy().into_owned());
        }
    }
    fallback
}

/// Cross-linked wiki graph for the Vault Neural Map view. Reads `vault_path`'s
/// top-level .md files directly (same flat layout the file watcher and
/// `index::rebuild` already assume) rather than reusing the tantivy pipeline,
/// since `frontmatter::parse` strips `[[`/`]]` brackets for search snippets —
/// this needs the link targets those brackets carry.
pub fn build_graph(vault_path: &Path) -> GraphData {
    let texts = read_texts(vault_path);
    if texts.is_empty() {
        return GraphData::default();
    }

    // (hub, child) pairs, restricted to wikis that exist.
    let mut children: Vec<(&String, String)> = Vec::new();
    for (id, (_, text)) in &texts {
        for c in child_targets(text) {
            if &c != id && texts.contains_key(&c) {
                children.push((id, c));
            }
        }
    }
    let hubs: BTreeSet<&String> = children.iter().map(|(h, _)| *h).collect();
    let child_pairs: BTreeSet<(String, String)> = children.iter().map(|(h, c)| pair_key(h, c)).collect();

    let nodes: Vec<GraphNode> = texts
        .iter()
        .map(|(id, (path, text))| GraphNode { id: id.clone(), words: word_count(text), path: path.clone(), hub: hubs.contains(id) })
        .collect();

    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut edges = Vec::new();
    for (name, (_, text)) in &texts {
        for target in wikilink_targets(text) {
            if &target == name || !texts.contains_key(&target) {
                continue;
            }
            let key = pair_key(name, &target);
            if seen.insert(key.clone()) {
                let kind = if child_pairs.contains(&key) { "child" } else { "link" };
                edges.push(GraphEdge { source: name.clone(), target, kind });
            }
        }
    }

    GraphData { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nodes_and_dedupes_undirected_edges() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::write(
            vault.path().join("a.md"),
            "---\ntopics:\n  - \"[[b]]\"\n---\n# A\n\nSee [[b|the B doc]] and [[#Self Heading]].",
        )
        .unwrap();
        std::fs::write(vault.path().join("b.md"), "# B\n\nLinks back to [[a]] and [[missing]].").unwrap();
        std::fs::write(vault.path().join("_manifest.md"), "not a real wiki").unwrap();

        let g = build_graph(vault.path());
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1, "a<->b should collapse to one undirected edge, missing/self dropped");
        assert_eq!(g.edges[0].kind, "link");
    }

    #[test]
    fn child_wikis_section_marks_hub_and_child_edges() {
        let vault = tempfile::tempdir().unwrap();
        let w = |n: &str, t: &str| std::fs::write(vault.path().join(n), t).unwrap();
        w("hub.md", "# Hub\n\nSee [[other]].\n\n## Child wikis\n- [[kid]]: a\n- [[gone]]: missing\n\n## Recap\nNot a child: [[notkid]]");
        w("kid.md", "---\ntopics:\n  - \"[[hub]]\"\n---\n# Kid");
        w("other.md", "# Other");
        w("notkid.md", "# Notkid");
        let g = build_graph(vault.path());
        let kind = |a: &str, b: &str| g.edges.iter().find(|e| (e.source == a && e.target == b) || (e.source == b && e.target == a)).map(|e| e.kind);
        assert_eq!(g.edges.len(), 3, "hub-kid once (kid's topics link collapses into it), hub-other, hub-notkid");
        assert_eq!(kind("hub", "kid"), Some("child"));
        assert_eq!(kind("hub", "other"), Some("link"));
        assert_eq!(kind("hub", "notkid"), Some("link"), "after the Child wikis section ends");
        assert!(g.nodes.iter().find(|n| n.id == "hub").unwrap().hub);
        assert!(!g.nodes.iter().find(|n| n.id == "kid").unwrap().hub);
    }

    #[test]
    fn families_backlinks_and_resolve() {
        let vault = tempfile::tempdir().unwrap();
        let w = |n: &str, t: &str| std::fs::write(vault.path().join(n), t).unwrap();
        w("hub.md", "# Hub\n\n## Child wikis\n- [[kid]]\n- [[gone]]");
        w("kid.md", "---\ntopics:\n  - \"[[hub]]\"\n---\n# Kid Title");
        w("solo.md", "# Solo\n\nSee [[Kid]].");
        let f = families(vault.path());
        assert_eq!(f.len(), 1);
        assert_eq!(f["hub"], vec!["kid".to_string()]);
        let b: Vec<String> = backlinks(vault.path(), "kid").into_iter().map(|b| b.title).collect();
        assert_eq!(b, vec!["Hub".to_string(), "Solo".to_string()]);
        assert!(resolve(vault.path(), "KID").unwrap().ends_with("kid.md"));
        assert!(resolve(vault.path(), "nope").is_none());
    }
}
