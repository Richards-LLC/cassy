//! GH #1235: distinguish a cited external proof from an unsupported close claim.
//!
//! This is a bounded prose guard, not validation that a remote run passed.
//! Existing proof/verification gates retain that responsibility. Only the
//! proposed close reason is inspected; historical notes are not new assertions.

use regex::Regex;
use std::sync::OnceLock;

struct Grammar {
    clauses: Regex,
    proof: Regex,
    passed: Regex,
    external: Regex,
    future: Regex,
    execution: Regex,
    receipt: Regex,
    receipt_heading: Regex,
    negated: Regex,
}

fn grammar() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR.get_or_init(|| {
        let re = |pattern| Regex::new(pattern).expect("constant close proof grammar");
        Grammar {
            // Do not split inside URLs or artifact names.
            clauses: re(r"[;\n]|[.!?](?:\s+|$)"),
            proof: re(r"\b(tests?|suites?|journeys?|checks?|validation|verification|proof|ci|playwright|builds?)\b"),
            passed: re(r"\b(pass(?:ed|es|ing)?|green|success(?:ful|fully)?|succeed(?:ed|s)?|verified|validated|proved|proven|complete[ds]?)\b"),
            external: re(r"\b(supervisor|reviewer|qa|operator|merge[- ]queue|assembly|ci)\b"),
            future: re(r"\b(will|shall|going to|expected to|scheduled to)\b"),
            execution: re(r"\b(run|execute[ds]?|prove[ds]?|validate[ds]?|verify|check|test)\b"),
            // Plain run/CI numbers need enough digits to distinguish them
            // from test counts. Published artifact IDs are opaque hex IDs.
            receipt: re(r"\b(?:run|ci)\s*[:=#]?\s+[1-9][0-9]{5,}\b|\brun[ _-]?id\s*[:=#]?\s*[1-9][0-9]*\b|\brun\s*[:=#]\s*[1-9][0-9]*\b|/actions/runs/[1-9][0-9]*\b|\bart-[a-f0-9]{8,}\b"),
            receipt_heading: re(r"^[\s(`*]*(?:receipt|evidence|proof:|run\b|run[_-]?id\b|ci\b|artifact\b|art-[a-f0-9])"),
            negated: re(r"\b(?:(?:will|shall)\s+not|not|never|hasn't|haven't|didn't|isn't)\s+(?:yet\s+)?(?:pass(?:ed|ing)?|green|verified|validated|run|complete[ds]?)\b"),
        }
    })
}

/// Require a citation next to an external completion assertion or a promise
/// of future proof. Explicit deferral is accepted without claiming it passed.
/// A citation-only following clause may supply the receipt; another claim's
/// receipt cannot silently stand in for this one.
pub(super) fn has_unreceipted_claim(reason: &str) -> bool {
    let g = grammar();
    let lower = reason.to_ascii_lowercase();
    let clauses: Vec<_> = g
        .clauses
        .split(&lower)
        .filter(|s| !s.trim().is_empty())
        .collect();
    clauses.iter().enumerate().any(|(index, clause)| {
        // Remove only the negated assertion, not other positive assertions
        // in the same clause ("tests passed, build not verified").
        let assertion = g.negated.replace_all(clause, "");
        if !g.proof.is_match(&assertion) {
            return false;
        }
        let completed = g.passed.is_match(&assertion);
        let future =
            g.future.is_match(&assertion) && (completed || g.execution.is_match(&assertion));
        let claim = (completed && g.external.is_match(&assertion)) || future;
        if !claim || (!completed && clause.contains("deferred to ")) {
            return false;
        }
        let adjacent_receipt = clauses.get(index + 1).is_some_and(|next| {
            g.receipt_heading.is_match(next)
                && g.receipt.is_match(next)
                && !g.passed.is_match(next)
                && !g.future.is_match(next)
        });
        !g.receipt.is_match(clause) && !adjacent_receipt
    })
}
