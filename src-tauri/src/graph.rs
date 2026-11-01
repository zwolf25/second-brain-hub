use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
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

/// Cross-linked wiki graph for the Vault Neural Map view. Reads `vault_path`'s
/// top-level .md files directly (same flat layout the file watcher and
/// `index::rebuild` already assume) rather than reusing the tantivy pipeline,
/// since `frontmatter::parse` strips `[[`/`]]` brackets for search snippets —
/// this needs the link targets those brackets carry.
pub fn build_graph(vault_path: &Path) -> GraphData {
    let mut texts: HashMap<String, (String, String)> = HashMap::new(); // id -> (path, text)
    let Ok(entries) = std::fs::read_dir(vault_path) else {
        return GraphData::default();
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
}
