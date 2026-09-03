use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

#[derive(Serialize, Clone)]
pub struct GraphNode {
    id: String,
    words: usize,
    path: String,
}

#[derive(Serialize, Clone)]
pub struct GraphEdge {
    source: String,
    target: String,
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

    let nodes: Vec<GraphNode> = texts
        .iter()
        .map(|(id, (path, text))| GraphNode { id: id.clone(), words: word_count(text), path: path.clone() })
        .collect();

    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut edges = Vec::new();
    for (name, (_, text)) in &texts {
        for target in wikilink_targets(text) {
            if &target == name || !texts.contains_key(&target) {
                continue;
            }
            let key = if name < &target {
                (name.clone(), target.clone())
            } else {
                (target.clone(), name.clone())
            };
            if seen.insert(key) {
                edges.push(GraphEdge { source: name.clone(), target });
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
    }
}
