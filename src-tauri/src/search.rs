use crate::index::Fields;
use anyhow::Result;
use serde::Serialize;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query};
use tantivy::schema::document::TantivyDocument;
use tantivy::schema::Value;
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

pub fn search(index: &Index, fields: &Fields, query_text: &str) -> Result<Vec<SearchResult>> {
    let query_text = query_text.trim();
    if query_text.is_empty() {
        return Ok(vec![]);
    }

    let reader = index.reader()?;
    let searcher = reader.searcher();

    let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![];
    for token in query_text.split_whitespace() {
        let token = token.to_lowercase();
        // Edit distance 2 for longer tokens, 1 for short ones, to avoid noisy matches.
        let distance = if token.chars().count() > 5 { 2 } else { 1 };

        let title_term = Term::from_field_text(fields.title, &token);
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(
                Box::new(FuzzyTermQuery::new(title_term, distance, true)),
                2.0,
            )),
        ));

        let topics_term = Term::from_field_text(fields.topics, &token);
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(
                Box::new(FuzzyTermQuery::new(topics_term, distance, true)),
                1.5,
            )),
        ));

        let body_term = Term::from_field_text(fields.body, &token);
        clauses.push((Occur::Should, Box::new(FuzzyTermQuery::new(body_term, distance, true))));
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
