//! Reference extraction for `history_docs.refs_json` (EPIC cas-6212 / cas-9a38,
//! spec §4.1 + §8).
//!
//! Two kinds of reference end up in the same JSON object, and the distinction
//! matters more than the format does:
//!
//! - **Structured** references come from GitHub's GraphQL response — a merged
//!   PR's `mergeCommit.oid` and its `commits` list. These are facts, full
//!   40-char SHAs, and they are what gives spec §8's "which PR shipped this
//!   commit" edge without heuristics.
//! - **Textual** references are scraped out of prose bodies: `#123`, `cas-9a38`,
//!   and abbreviated commit SHAs. These are *candidates*. Nothing here resolves
//!   them; resolution against `history_commits` — with variable-width prefix
//!   matching and an ambiguity verdict — is M5's job (spec §11 M5). Storing the
//!   raw candidate rather than a resolved id is deliberate: the resolver must
//!   see the width the human actually wrote.
//!
//! # Why the hex scraper demands a digit
//!
//! `[0-9a-f]{7,}` matches ordinary English words — "accede", "decade",
//! "façade" minus its cedilla, "deface", "bedded", "cabbed". Measured against
//! this repo's own issue corpus, a digitless filter is the difference between a
//! usable candidate list and one dominated by prose. A real abbreviated SHA is
//! overwhelmingly likely to contain a digit (the probability that 7 random hex
//! nibbles are all in `a-f` is (6/16)^7 ≈ 0.1%), so the rule discards ~0.1% of
//! genuine SHAs to discard essentially all of the English. That trade is stated
//! rather than hidden because it is a recall loss, and M5 sees only what
//! survives it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Minimum abbreviated-SHA width accepted from prose. Seven is git's own
/// historical default abbreviation and the floor below which collisions in a
/// repo this size stop being rare.
const MIN_SHA_WIDTH: usize = 7;
const MAX_SHA_WIDTH: usize = 40;

/// Everything a doc points at. Serialized into `history_docs.refs_json`.
///
/// Every list is sorted and de-duplicated so the JSON is stable: an unchanged
/// body must not produce a changed `refs_json`, or every re-fetch would look
/// like an edit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRefs {
    /// Abbreviated or full commit SHAs mentioned in prose. Candidates, not
    /// resolved commits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    /// `#123` style issue/PR references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<i64>,
    /// CAS task ids (`cas-9a38`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<String>,
    /// A merged PR's merge commit, straight from GraphQL. Full 40 chars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_commit: Option<String>,
    /// The commits GraphQL reports on a PR. Full 40 chars.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pr_commits: Vec<String>,
    /// Git tag range a CHANGELOG section covers, e.g. `v2.48.3..v2.49.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_range: Option<String>,
    /// The tag a CHANGELOG section names, when it exists in the repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl DocRefs {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `None` when nothing was found, so an empty object is never written as
    /// `"{}"` — "no references" and "never looked" stay distinguishable.
    pub fn to_json(&self) -> Option<String> {
        (!self.is_empty()).then(|| serde_json::to_string(self).unwrap_or_default())
    }
}

/// Scrape prose for commit SHAs, `#issue` numbers and `cas-` task ids.
pub fn extract_from_text(text: &str) -> DocRefs {
    let mut commits: BTreeSet<String> = BTreeSet::new();
    let mut issues: BTreeSet<i64> = BTreeSet::new();
    let mut tasks: BTreeSet<String> = BTreeSet::new();

    // One pass over word-ish tokens. Splitting on "not [alnum-]" keeps
    // `cas-9a38` whole while breaking `abc123,` and `(#12)` apart.
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch == '#' {
            let rest = &text[idx + 1..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty()
                && digits.len() <= 9
                && let Ok(n) = digits.parse::<i64>()
                && n > 0
            {
                issues.insert(n);
            }
            continue;
        }
        if !is_token_char(ch) {
            continue;
        }
        // Only start a token at a real boundary.
        if idx > 0
            && text[..idx]
                .chars()
                .next_back()
                .is_some_and(|p| is_token_char(p) || p == '#')
        {
            continue;
        }
        let token: &str = {
            let end = text[idx..]
                .char_indices()
                .find(|(_, c)| !is_token_char(*c))
                .map(|(off, _)| idx + off)
                .unwrap_or(text.len());
            &text[idx..end]
        };
        while chars.peek().is_some_and(|(i, _)| *i < idx + token.len()) {
            chars.next();
        }

        if let Some(task) = task_id(token) {
            tasks.insert(task);
        } else if let Some(sha) = commit_sha(token) {
            commits.insert(sha);
        }
    }

    DocRefs {
        commits: commits.into_iter().collect(),
        issues: issues.into_iter().collect(),
        tasks: tasks.into_iter().collect(),
        ..DocRefs::default()
    }
}

fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-'
}

/// `cas-9a38`, case-insensitively; the stored form is lowercase so a body
/// shouting `CAS-9A38` joins the same task as one that does not.
fn task_id(token: &str) -> Option<String> {
    let lower = token.to_ascii_lowercase();
    let suffix = lower.strip_prefix("cas-")?;
    let id = suffix.split('-').next()?;
    (id.len() >= 4 && id.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| format!("cas-{id}"))
}

/// An abbreviated or full commit SHA, subject to the digit rule documented in
/// the module header.
fn commit_sha(token: &str) -> Option<String> {
    let lower = token.to_ascii_lowercase();
    if lower.len() < MIN_SHA_WIDTH || lower.len() > MAX_SHA_WIDTH {
        return None;
    }
    if !lower.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // A full 40-char SHA is unambiguous even without a digit; only the
    // abbreviated forms need the English filter.
    if lower.len() < MAX_SHA_WIDTH && !lower.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_shas_issue_numbers_and_task_ids() {
        let refs = extract_from_text(
            "Fixes #155 and #7. Landed as 58084e5a, full f509695b365c84dd868d12df7411470cbae5c227. \
             Tracked by cas-9a38 (see CAS-6212).",
        );
        assert_eq!(refs.issues, vec![7, 155]);
        assert_eq!(refs.tasks, vec!["cas-6212".to_string(), "cas-9a38".to_string()]);
        assert_eq!(
            refs.commits,
            vec![
                "58084e5a".to_string(),
                "f509695b365c84dd868d12df7411470cbae5c227".to_string(),
            ]
        );
    }

    /// The failure this rule exists to prevent: English prose read as commits.
    #[test]
    fn digitless_hex_words_are_not_commits() {
        let refs = extract_from_text("The facade decade acceded, deadbeef defaced, cabbaged.");
        assert!(
            refs.commits.is_empty(),
            "prose leaked into the commit candidates: {:?}",
            refs.commits
        );
    }

    #[test]
    fn short_hex_and_non_hex_tokens_are_rejected() {
        let refs = extract_from_text("abc123 is 6 chars; zzz1234 is not hex; ab12cd3 is 7 and hex");
        assert_eq!(refs.commits, vec!["ab12cd3".to_string()]);
    }

    /// A body must produce byte-identical `refs_json` on every pass, or the
    /// change detector treats a re-fetch as an edit.
    #[test]
    fn refs_json_is_stable_and_sorted() {
        let a = extract_from_text("cas-9a38 #3 ab12cd3 #1 cas-0001 99aabbcc");
        let b = extract_from_text("99aabbcc cas-0001 #1 ab12cd3 #3 cas-9a38");
        assert_eq!(a, b);
        assert_eq!(a.to_json(), b.to_json());
        assert_eq!(a.issues, vec![1, 3]);
    }

    #[test]
    fn empty_refs_serialize_to_none_not_an_empty_object() {
        assert_eq!(extract_from_text("nothing to see here").to_json(), None);
        assert!(extract_from_text("#1").to_json().is_some());
    }

    #[test]
    fn a_sha_glued_to_punctuation_is_still_found() {
        let refs = extract_from_text("(58084e5a), [ab12cd3]. trailing:99aabbcc;");
        assert_eq!(
            refs.commits,
            vec![
                "58084e5a".to_string(),
                "99aabbcc".to_string(),
                "ab12cd3".to_string(),
            ]
        );
    }

    /// A token that merely *contains* a hex run must not be mined for one —
    /// `v2.49.0-abc1234` is a version string, not a commit reference.
    #[test]
    fn hyphenated_tokens_are_not_split_into_candidate_shas() {
        let refs = extract_from_text("release-abc1234 shipped");
        assert!(refs.commits.is_empty(), "{:?}", refs.commits);
    }
}
