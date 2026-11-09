//! GH #1235: distinguish a cited external proof from an unsupported close claim.
//!
//! This is a bounded prose guard, not validation that a remote run passed.
//! Existing proof/verification gates retain that responsibility. Only the
//! proposed close reason is inspected; historical notes are not new assertions.

use regex::Regex;
use std::path::{Component, Path};
use std::sync::OnceLock;

struct Grammar {
    clauses: Regex,
    tokens: Regex,
    proof: Regex,
    passed: Regex,
    external: Regex,
    future: Regex,
    receipt: Regex,
    receipt_heading: Regex,
    negated: Regex,
    duration_percentage_ratio: Regex,
}

fn grammar() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR.get_or_init(|| {
        let re = |pattern| Regex::new(pattern).expect("constant close proof grammar");
        let proof = r"(?:tests?|suites?|journeys?|checks?|validation|verification|proof|ci|playwright|builds?)";
        let external = r"(?:supervisor|reviewer|qa|operator|merge[- ]queue|assembly|ci)";
        let status = r"(?:pass(?:ed|es|ing)?|green|success(?:ful|fully)?|succeed(?:ed|s)?|verified|validated|proved|proven|complete[ds]?)";
        let subject = format!(r"{proof}(?:\s+runs?)?(?:\s+(?:in|on|by|at)\s+(?:the\s+)?{external})?");
        Grammar {
            // Do not split inside URLs or artifact names.
            clauses: re(r"[;\n]|[.!?](?:\s+|$)"),
            tokens: re(r"`[^`]*`|\S+"),
            proof: re(&format!(r"\b{proof}\b")),
            // Require a proof subject/status or a completion verb/object,
            // rather than an adjective elsewhere in the clause.
            passed: re(&format!(r"\b{subject}(?:\s+(?:has|have|had|is|are|was|were|been|now|all|already|successfully))*\s+{status}\b|\b{external}(?:\s+(?:has|have|had|already|successfully))*\s+(?:passed|verified|validated|proved|completed)\s+(?:(?:the|all|a|an)\s+)?{proof}\b")),
            external: re(&format!(r"\b{external}\b")),
            future: re(&format!(r"\b(?:will|shall|going to|expected to|scheduled to)\s+(?:(?:be|have|been|successfully)\s+)*(?:{status}|run|execute[ds]?|prove[ds]?|validate[ds]?|verify|check|test)\b")),
            // Plain run/CI numbers need enough digits to distinguish them
            // from test counts. Published artifact IDs are opaque hex IDs.
            receipt: re(r"(?i)\b(?:run|ci)\s*[:=#]?\s+[1-9][0-9]{5,}\b|\brun[ _-]?id\s*[:=#]?\s*[1-9][0-9]*\b|\brun\s*[:=#]\s*[1-9][0-9]*\b|/actions/runs/[1-9][0-9]*\b|\bart-[a-f0-9]{8,}\b"),
            receipt_heading: re(r"^[\s(`*]*(?:receipt|evidence|proof:|run\b|run[_-]?id\b|ci\b|artifact\b|art-[a-f0-9])"),
            negated: re(r"\b(?:(?:will|shall)\s+not|not|never|hasn't|haven't|didn't|isn't)\s+(?:yet\s+)?(?:pass(?:ed|ing)?|green|verified|validated|run|complete[ds]?)\b"),
            duration_percentage_ratio: re(r"(?i)^[+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:ns|us|µs|ms|s|m|h|d)/[+-]?(?:\d+(?:\.\d+)?|\.\d+)%$"),
        }
    })
}

/// Require a citation next to an external completion assertion or a promise
/// of future proof. Explicit deferral is accepted without claiming it passed.
/// A citation-only following clause may supply the receipt; another claim's
/// receipt cannot silently stand in for this one.
pub(super) fn validate_claims(
    reason: &str,
    cas_root: &Path,
    receipt_root: &Path,
) -> Result<(), String> {
    let g = grammar();
    // Keep the original spelling for case-sensitive filesystem citations.
    let clauses: Vec<_> = g
        .clauses
        .split(reason)
        .filter(|s| !s.trim().is_empty())
        .collect();
    for (index, clause) in clauses.iter().enumerate() {
        let lower = assertion_words(clause);
        // Remove only the negated assertion, not other positive assertions
        // in the same clause ("tests passed, build not verified").
        let assertion = g.negated.replace_all(&lower, "");
        if !g.proof.is_match(&assertion) {
            continue;
        }
        let completed = g.passed.is_match(&assertion);
        let future = g.future.is_match(&assertion);
        let claim = (completed && g.external.is_match(&assertion)) || future;
        if !claim || (!completed && lower.contains("deferred to ")) {
            continue;
        }
        let classification = if future {
            "FUTURE_PROOF"
        } else {
            "EXTERNAL_COMPLETION"
        };
        let with_claim = |error: String| {
            format!(
                "{error}\nClassification: {classification}\nClause: {}",
                clause.trim()
            )
        };
        let adjacent_receipt = clauses.get(index + 1).filter(|next| {
            let lower = assertion_words(next);
            (g.receipt_heading.is_match(&next.to_ascii_lowercase())
                || !local_paths(next).is_empty())
                && (g.receipt.is_match(next) || !local_paths(next).is_empty())
                && !g.passed.is_match(&lower)
                && !g.future.is_match(&lower)
        });
        let mut receipted = false;
        for cited in std::iter::once(*clause).chain(adjacent_receipt.copied()) {
            let paths = local_paths(cited);
            receipted |= g.receipt.is_match(cited) || !paths.is_empty();
            // Run IDs/actions URLs retain their existing semantics: close
            // performs no network check. Every local citation must resolve,
            // even when another citation in the same claim is valid.
            for id in g
                .receipt
                .find_iter(cited)
                .map(|m| m.as_str())
                .filter(|id| id.to_ascii_lowercase().starts_with("art-"))
            {
                let found = cas_store::SqliteArtifactStore::open(cas_root)
                    .and_then(|store| store.get(&id.to_ascii_lowercase()))
                    .map_err(|error| with_claim(receipt_error(id, &error.to_string())))?;
                if found.is_none() {
                    return Err(with_claim(receipt_error(
                        id,
                        "no artifact record in this project",
                    )));
                }
            }
            for path in paths {
                let expanded = crate::qa_evidence::expand_home(path);
                if !receipt_root.join(expanded).is_file() {
                    return Err(with_claim(receipt_error(path, "no file at the cited path")));
                }
            }
        }
        if !receipted {
            return Err(with_claim("PROOF CLAIM RECEIPT REQUIRED\nCite a run id, artifact id or local receipt path beside each third-party or future proof claim.\nIf it has not run, write \"deferred to <stage>\" instead of claiming a pass.".to_string()));
        }
    }
    Ok(())
}

/// Identifiers, filenames, paths and inline code are data, not prose actors or
/// predicates. Keep a barrier so removing one cannot join unrelated words.
fn assertion_words(clause: &str) -> String {
    let lower = clause.to_ascii_lowercase();
    grammar()
        .tokens
        .find_iter(&lower)
        .map(|token| {
            let word = token
                .as_str()
                .trim_matches(|c| "()[]{},:;.!?\"'*".contains(c));
            if !word.is_empty()
                && (word == "merge-queue"
                    || word.chars().all(|c| c.is_ascii_alphabetic() || c == '\''))
            {
                word
            } else {
                "_"
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn receipt_error(citation: &str, detail: &str) -> String {
    format!(
        "PROOF CLAIM RECEIPT REQUIRED\nCannot resolve receipt `{citation}`: {detail}.\nCite an existing receipt, or write \"deferred to <stage>\" if it has not run."
    )
}

/// Qualified local paths, optionally backtick-quoted (including spaces).
/// Bare test counts and remote run URLs are not filesystem citations.
fn local_paths(clause: &str) -> Vec<&str> {
    let mut paths = Vec::new();
    for (index, segment) in clause.split('`').enumerate() {
        let tokens: Vec<_> = if index % 2 == 1 {
            vec![segment]
        } else {
            segment.split_whitespace().collect()
        };
        for token in tokens {
            let path = super::strip_path_punctuation(token);
            if grammar().duration_percentage_ratio.is_match(path) {
                continue;
            }
            let has_path_component = |candidate: &str| {
                Path::new(candidate)
                    .components()
                    .any(|component| matches!(component, Component::Normal(_)))
            };
            if !path.contains("://")
                && !path.to_ascii_lowercase().starts_with("/actions/runs/")
                && ((path.starts_with('/') && has_path_component(path))
                    || ["~/", "./", "../"]
                        .iter()
                        .any(|prefix| path.strip_prefix(*prefix).is_some_and(has_path_component))
                    || (path.contains('/') && Path::new(path).extension().is_some()))
            {
                paths.push(path);
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::validate_claims;

    #[test]
    fn close_reason_requires_receipts_only_for_proof_assertions() {
        let cas_root = tempfile::tempdir().expect("temporary Cassy root");
        let receipt_root = tempfile::tempdir().expect("temporary receipt root");
        std::fs::write(receipt_root.path().join("receipt.log"), "proof").expect("receipt file");

        validate_claims(
            "Only journeys.yaml manually resolved: supervisor-handoff-machine-lineage, both complete blocks verbatim.",
            cas_root.path(),
            receipt_root.path(),
        )
        .expect("local documentation integrity is not an external completion claim");

        let reason = "CI reported 10312 tests passed / 1 failed";
        let error = validate_claims(reason, cas_root.path(), receipt_root.path())
            .expect_err("an unsupported CI claim still requires a receipt");
        assert!(error.contains("EXTERNAL_COMPLETION"), "{error}");
        assert!(error.contains(reason), "{error}");

        validate_claims(
            "CI tests passed ./receipt.log",
            cas_root.path(),
            receipt_root.path(),
        )
        .expect("path-shaped local receipts remain valid");
    }
}
