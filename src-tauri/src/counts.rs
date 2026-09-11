use std::path::Path;

const WIKI_PROCESSED_MARKER: &str = "<!-- wiki-processed -->";

/// A raw file counts as unprocessed if it doesn't contain the literal marker
/// `sc-wiki-builder` stamps on every file it's folded into a wiki — that marker
/// is the sole authoritative signal per the skill (frontmatter shape varies,
/// since anyone on the team can contribute files with inconsistent structure).
pub fn count_unprocessed_raw(raw_path: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(raw_path) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
        })
        .filter(|e| {
            std::fs::read_to_string(e.path())
                .map(|content| !content.contains(WIKI_PROCESSED_MARKER))
                .unwrap_or(false)
        })
        .count()
}

/// An inbox item is its note (`.md`, either standalone or a `*.share.md`
/// companion) — not any payload file it's paired with, which avoids
/// double-counting paired items without assuming every item has a payload.
pub fn count_inbox_items(inbox_path: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(inbox_path) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_raw_and_inbox_correctly() {
        let raw = tempfile::tempdir().unwrap();
        std::fs::write(raw.path().join("unprocessed.md"), "# A raw note\nsome content").unwrap();
        std::fs::write(
            raw.path().join("processed.md"),
            "# Done\nfolded in\n<!-- wiki-processed -->",
        )
        .unwrap();
        std::fs::write(raw.path().join("notes.txt"), "not markdown, ignored").unwrap();
        assert_eq!(count_unprocessed_raw(raw.path()), 1);

        let inbox = tempfile::tempdir().unwrap();
        std::fs::write(inbox.path().join("report.pdf.share.md"), "note").unwrap();
        std::fs::write(inbox.path().join("report.pdf"), "payload").unwrap();
        std::fs::write(inbox.path().join("standalone.share.md"), "note-only item").unwrap();
        assert_eq!(count_inbox_items(inbox.path()), 2);
    }
}
