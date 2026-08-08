//! Free text → FTS5 expression, shared by every FTS-backed store.
//!
//! Factored out of `knowledge_store.rs` when the history index (EPIC cas-6212)
//! became the second FTS consumer. It is deliberately *shared* rather than
//! copied: the function encodes a correctness lesson (cas-461a, below) that a
//! fork would silently lose, exactly as the three forked git-log parsers M1
//! collapsed had each lost the same rename fix.

/// Turn a free-text query into an FTS5 expression that cannot be a syntax
/// error: every token is quoted, and terms are **ORed**.
///
/// WHY OR AND NOT AND (cas-461a): this used to join the quoted tokens with a
/// space, which FTS5 reads as an implicit `AND` — every term had to occur in
/// the same document. The Tantivy BM25 surface it sits beside is disjunctive,
/// so the two differed in boolean semantics and the FTS side was the strict
/// one: recall fell as the query got longer, and past ~3 terms a user got
/// nothing. The cas-d075 measurement found 7 of 10 real-vocabulary queries
/// returning **zero** pages where legacy returned 4–10, with the same queries
/// matching 18–107 pages under `OR` — the content was present and indexed and
/// the defect was query construction alone. The failure was silent (a clean
/// "no matches", not an error), which is why it survived.
///
/// Ranking needs no extra machinery: callers order by `bm25()`, and BM25 over
/// a disjunctive match set inherently prefers documents containing more of the
/// query's terms. So an OR match with BM25 ordering *is* the "AND-preference"
/// behaviour, without the recall cliff.
///
/// Double-quoted runs are preserved as FTS5 phrases: `verifier "quality gates"`
/// becomes `"verifier" OR "quality gates"`. An unterminated quote is treated as
/// a phrase running to end of input rather than an error.
///
/// Injection safety is structural, not filtering: only alphanumeric tokens ever
/// reach the output, so no user input can close a quote or introduce an FTS5
/// operator. Punctuation-only input yields `None`, which callers turn into an
/// empty result set instead of a syntax error.
pub fn fts_or_query(query: &str) -> Option<String> {
    fn flush(raw: &str, is_phrase: bool, terms: &mut Vec<String>) {
        let tokens: Vec<String> = raw
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_lowercase())
            .collect();
        if tokens.is_empty() {
            return;
        }
        if is_phrase {
            // One FTS5 phrase: the tokens must appear adjacently, in order.
            terms.push(format!("\"{}\"", tokens.join(" ")));
        } else {
            terms.extend(tokens.into_iter().map(|t| format!("\"{t}\"")));
        }
    }

    let mut terms: Vec<String> = Vec::new();
    let mut segment = String::new();
    let mut in_phrase = false;
    for ch in query.chars() {
        if ch == '"' {
            flush(&segment, in_phrase, &mut terms);
            segment.clear();
            in_phrase = !in_phrase;
        } else {
            segment.push(ch);
        }
    }
    flush(&segment, in_phrase, &mut terms);

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

#[cfg(test)]
mod tests {
    use super::fts_or_query;

    #[test]
    fn terms_are_ored_and_phrases_preserved() {
        assert_eq!(
            fts_or_query("verifier \"quality gates\"").as_deref(),
            Some("\"verifier\" OR \"quality gates\"")
        );
    }

    /// The injection guard is structural: operators cannot survive tokenizing.
    #[test]
    fn fts5_operators_cannot_escape_the_quoting() {
        let expr = fts_or_query("foo* OR bar NEAR(baz) \"").unwrap();
        assert!(!expr.contains('*'), "glob operator leaked: {expr}");
        assert!(!expr.contains('('), "NEAR call leaked: {expr}");
        // Every quote in the output is one this function emitted, in pairs.
        assert_eq!(expr.matches('"').count() % 2, 0);
    }

    #[test]
    fn punctuation_only_input_is_not_a_query() {
        assert!(fts_or_query("--- ???").is_none());
        assert!(fts_or_query("").is_none());
    }
}
