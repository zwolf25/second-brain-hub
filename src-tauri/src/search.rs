use crate::index::Fields;
use anyhow::Result;
use serde::Serialize;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, PhraseQuery, Query, TermQuery};
use tantivy::schema::document::TantivyDocument;
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::snippet::SnippetGenerator;
use tantivy::{Index, Term};

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub path: String,
    pub title: String,
    pub snippet: String,
    pub updated: String,
}

const MAX_RESULTS: usize = 30;

// Query-side only (the tantivy tokenizer that indexes title/topics/body keeps
// stopwords, so exact/phrase matching against them still works) — this list
// just keeps fuzzy noise words like "how"/"do"/"i" from diluting ranking on
// natural-language queries, which is the query style people lean on now that
// there's no AI Search box to ask a question in instead.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "do", "does", "for", "from", "how",
    "i", "if", "in", "into", "is", "it", "of", "on", "or", "that", "the", "this", "to", "was",
    "what", "when", "where", "which", "who", "why", "will", "with", "you",
];

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

pub fn search(index: &Index, fields: &Fields, query_text: &str) -> Result<Vec<SearchResult>> {
    let query_text = query_text.trim();
    if query_text.is_empty() {
        return Ok(vec![]);
    }

    let reader = index.reader()?;
    let searcher = reader.searcher();

    let all_tokens: Vec<String> = query_text.split_whitespace().map(|t| t.to_lowercase()).collect();
    // A query that's entirely stopwords (rare, but possible) still needs
    // something to search on — fall back to the unfiltered list rather than
    // producing zero clauses.
    let significant: Vec<&String> = all_tokens.iter().filter(|t| !is_stopword(t)).collect();
    let tokens_for_clauses: Vec<&String> =
        if significant.is_empty() { all_tokens.iter().collect() } else { significant };

    let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![];
    for token in tokens_for_clauses {
        // Edit distance 2 for longer tokens, 1 for short ones, to avoid noisy matches.
        let distance = if token.chars().count() > 5 { 2 } else { 1 };

        // Exact (non-fuzzy) term clauses are boosted above their fuzzy
        // siblings so a precise match always outranks a same-field typo
        // match instead of tying with it.
        let title_term = Term::from_field_text(fields.title, token);
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(
                Box::new(TermQuery::new(title_term.clone(), IndexRecordOption::Basic)),
                3.0,
            )),
        ));
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(Box::new(FuzzyTermQuery::new(title_term, distance, true)), 2.0)),
        ));

        let topics_term = Term::from_field_text(fields.topics, token);
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(
                Box::new(TermQuery::new(topics_term.clone(), IndexRecordOption::Basic)),
                2.0,
            )),
        ));
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(Box::new(FuzzyTermQuery::new(topics_term, distance, true)), 1.5)),
        ));

        let body_term = Term::from_field_text(fields.body, token);
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(
                Box::new(TermQuery::new(body_term.clone(), IndexRecordOption::Basic)),
                1.5,
            )),
        ));
        clauses.push((Occur::Should, Box::new(FuzzyTermQuery::new(body_term, distance, true))));
    }

    // Literal-phrase boost: a page containing the exact multi-word phrase
    // (e.g. a distinctive feature name) jumps above pages that only match
    // scattered individual terms. Built from every original token, stopwords
    // included, since that's what's actually indexed in the body field.
    if all_tokens.len() >= 2 {
        let phrase_terms: Vec<Term> =
            all_tokens.iter().map(|t| Term::from_field_text(fields.body, t)).collect();
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(Box::new(PhraseQuery::new(phrase_terms)), 5.0)),
        ));
    }

    let query = BooleanQuery::new(clauses);
    let top_docs = searcher.search(&query, &TopDocs::with_limit(MAX_RESULTS))?;

    let snippet_generator = SnippetGenerator::create(&searcher, &query, fields.body).ok();

    let mut results = Vec::with_capacity(top_docs.len());
    for (_score, doc_address) in top_docs {
        let doc: TantivyDocument = searcher.doc(doc_address)?;

        let get = |f| doc.get_first(f).and_then(|v| v.as_str()).unwrap_or("").to_string();

        let snippet = snippet_generator
            .as_ref()
            .map(|g| g.snippet_from_doc(&doc).to_html())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let body = get(fields.body);
                body.chars().take(220).collect()
            });

        results.push(SearchResult {
            path: get(fields.path),
            title: get(fields.title),
            snippet,
            updated: get(fields.updated),
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use crate::index;

    #[test]
    fn exact_match_outranks_fuzzy_typo_match() {
        let vault = tempfile::tempdir().unwrap();
        let index_dir = tempfile::tempdir().unwrap();

        std::fs::write(
            vault.path().join("exact.md"),
            "---\ntype: wiki\n---\n# Exact\n\nAll about refrigerant tracking compliance.",
        )
        .unwrap();
        std::fs::write(
            vault.path().join("typo.md"),
            "---\ntype: wiki\n---\n# Typo\n\nAll about refrigirant systems (misspelled on purpose).",
        )
        .unwrap();

        index::rebuild(index_dir.path(), vault.path()).unwrap();
        let (idx, fields) = index::open(index_dir.path()).unwrap();

        let results = super::search(&idx, &fields, "refrigerant").unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "Exact", "exact term match should outrank a fuzzy typo match");
    }

    #[test]
    fn stopword_heavy_natural_language_query_still_surfaces_the_right_doc() {
        let vault = tempfile::tempdir().unwrap();
        let index_dir = tempfile::tempdir().unwrap();

        std::fs::write(
            vault.path().join("rate-validation.md"),
            "---\ntype: wiki\n---\n# Rate Validation\n\nSteps to enable rate validation for a provider.",
        )
        .unwrap();
        std::fs::write(
            vault.path().join("unrelated.md"),
            "---\ntype: wiki\n---\n# Unrelated\n\nNothing about that topic here.",
        )
        .unwrap();

        index::rebuild(index_dir.path(), vault.path()).unwrap();
        let (idx, fields) = index::open(index_dir.path()).unwrap();

        let results = super::search(&idx, &fields, "how do I enable rate validation").unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "Rate Validation");
    }
}
