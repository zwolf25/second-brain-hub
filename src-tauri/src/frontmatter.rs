use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    updated: String,
}

pub struct ParsedDoc {
    pub title: String,
    pub body: String,
    pub topics: String,
    pub updated: String,
}

/// Splits `---\nyaml\n---\nbody` frontmatter, extracts title from the first
/// `# ` heading (fallback: filename), and strips `[[wikilink]]` brackets so
/// search snippets read as plain text.
pub fn parse(raw: &str, filename_fallback: &str) -> ParsedDoc {
    let (frontmatter_yaml, body) = split_frontmatter(raw);

    let fm: Frontmatter = frontmatter_yaml
        .and_then(|y| serde_yaml::from_str(y).ok())
        .unwrap_or_default();

    let title = body
        .lines()
        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
        .unwrap_or_else(|| filename_fallback.to_string());

    let strip_brackets = |s: &str| s.replace("[[", "").replace("]]", "");

    ParsedDoc {
        title: strip_brackets(&title),
        body: strip_brackets(body),
        topics: strip_brackets(&fm.topics.join(" ")),
        updated: fm.updated,
    }
}

fn split_frontmatter(raw: &str) -> (Option<&str>, &str) {
    let raw = raw.trim_start_matches('\u{feff}'); // strip BOM if present
    let Some(rest) = raw.strip_prefix("---") else {
        return (None, raw);
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(end) = rest.find("\n---") else {
        return (None, raw);
    };
    let yaml = &rest[..end];
    let after = &rest[end + 4..];
    let body = after.strip_prefix('\n').unwrap_or(after);
    (Some(yaml), body)
}
