//! Commit → task / session provenance resolution (EPIC cas-6212 / cas-519f,
//! spec §5).
//!
//! # What this module is, and what it deliberately is not
//!
//! Spec §5.2 calls for a `history_commit_provenance` **view** resolved at query
//! time over the edges that are actually populated. This is that view, expressed
//! as types plus a resolver rather than as SQL, for two reasons that are
//! properties of the data rather than of taste:
//!
//! 1. **Ambiguity is a reverse query.** A 7-char prefix edge is only usable once
//!    you know how many *indexed commits* it matches. That is a `GROUP BY` over
//!    the opposite direction of the join, which a flat `UNION ALL` view cannot
//!    express without either dropping the ambiguous rows or silently keeping
//!    one of them — both of which §5.2 forbids.
//! 2. **"Unmeasurable" must not collapse into "zero".** The edges live in other
//!    subsystems' tables (`tasks`, `events`, `commit_links`). A store opened
//!    without them — a fresh project, or the history tables under test — is a
//!    legitimate state, and a view over a missing table is an error or an empty
//!    set, neither of which is the truth ("this edge cannot be read here").
//!
//! # The corpus this resolves over, measured
//!
//! Re-measured read-only against the live database on 2026-08-08 (2,489 commits
//! reachable from HEAD):
//!
//! | Edge | State |
//! |---|---|
//! | `tasks.deliverables.factory_branch_anchor` | 245 distinct full SHAs, 223 reachable from HEAD → **8.96%** coverage |
//! | `events.worker_git_commit` | 10,359 rows, of which **9,295 carry no metadata at all**, 46 carry the `'?'` stub, and 1,018 are usable at **7 / 8 / 40** chars |
//! | `tasks.notes` close-decision text | 22 rows |
//! | `commit_links` | **0 rows** — the spine this milestone repairs |
//!
//! The 40-char class is new since the spec was written: it is cas-ea51's
//! full-width fix already reaching live data, exactly as §12 Q7 predicted. It is
//! why [`classify_candidate`] has an `Exact` arm rather than treating every
//! event SHA as an abbreviation.

use serde::{Deserialize, Serialize};

/// `tasks.deliverables.factory_branch_anchor` — a full 40-char SHA written at
/// commit time. The only exact, unambiguous commit→task edge that has ever been
/// populated (spec §5.2).
pub const LINK_METHOD_FACTORY_ANCHOR: &str = "factory_branch_anchor";

/// `events.worker_git_commit` whose `metadata.head_sha` is a full 40-char SHA
/// (cas-ea51 and later). No prefix arithmetic, no collision guard.
pub const LINK_METHOD_WORKER_EVENT_EXACT: &str = "worker_git_commit_exact";

/// `events.worker_git_commit` whose `metadata.head_sha` is an abbreviation of
/// git's dynamic width. Matched with `sha LIKE prefix || '%'` at the event's own
/// length — never `sha[0..8]`, which would miss the 594 seven-char rows.
pub const LINK_METHOD_WORKER_EVENT_PREFIX: &str = "worker_git_commit_prefix";

/// `tasks.notes` mentioning the commit as text (the free-text close receipt,
/// `close_ops.rs`'s `append_close_decision_note`). Substring evidence, so never
/// better than medium.
pub const LINK_METHOD_TASK_NOTE: &str = "task_note_text";

/// A `commit_links` row written by the PostToolUse hook — a *direct
/// observation* of the session that ran `git commit`.
///
/// Defined in `cas-types` beside the `CommitLink` that carries it (that crate
/// cannot depend on this one) and re-exported here so the `LINK_METHOD_*`
/// vocabulary reads as one list.
pub use cas_types::LINK_METHOD_HOOK_OBSERVED;

/// A `commit_links` row *reconstructed* by the history indexer from the
/// `worker_git_commit` edge (spec §5.3's repair).
pub const LINK_METHOD_INDEXER_WORKER_EVENT: &str = "indexer_worker_git_commit";

/// The minimum abbreviation width this resolver will match on.
///
/// Seven is not arbitrary: it is the shortest width the corpus actually
/// contains (594 rows), and it is where §5.2's collision arithmetic already
/// sits — 28 bits over 2,489 commits is a ~1.1% any-collision probability.
/// Anything shorter is not evidence, and rather than match it weakly it is
/// counted into [`EdgeHealth::excluded_too_short`] so its exclusion is a
/// visible number instead of a silent filter.
pub const MIN_PREFIX_LEN: usize = 7;

/// A full git object name.
pub const FULL_SHA_LEN: usize = 40;

/// The documented "git status unavailable" sentinel written by
/// `collect_worker_git_status` (`factory_ops.rs`). It is a degradation signal,
/// not a SHA; treating it as one would match nothing and read as a coverage gap
/// rather than as the boundary it is (spec §5.2 consequence 2).
pub const HEAD_SHA_STUB: &str = "?";

/// How much a resolved edge is worth.
///
/// Three values, not a float: every consumer of this either shows it to a human
/// or decides whether to trust it, and a 0.72 would invite a threshold nobody
/// can justify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkConfidence {
    /// Substring or otherwise-ambiguous evidence: real enough to show, never
    /// enough to act on.
    Low,
    /// A populated edge with a plausible but unverified join.
    Medium,
    /// An exact, unambiguous SHA match, or a directly observed link.
    High,
}

impl LinkConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkConfidence::Low => "low",
            LinkConfidence::Medium => "medium",
            LinkConfidence::High => "high",
        }
    }
}

/// How an abbreviation in the wild relates to a full SHA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateClass {
    /// A full 40-char object name: match by equality.
    Exact,
    /// A usable abbreviation of `len` chars: match by `LIKE prefix || '%'`.
    Prefix,
    /// The `'?'` sentinel — a declared degradation, not a SHA.
    Stub,
    /// Below [`MIN_PREFIX_LEN`].
    TooShort,
    /// Not hexadecimal, over 40 chars, or empty.
    Invalid,
}

/// Classify a `metadata.head_sha` value from the wild.
///
/// Pure, so §5.2's variable-width rule is testable with no database, no
/// repository and no fixtures — which is the point: the rule is about the shape
/// of strings, and every past bug here (`sha[0..8]`, the `'?'` stub read as a
/// SHA) was a string-shape bug.
pub fn classify_candidate(raw: &str) -> CandidateClass {
    let value = raw.trim();
    if value == HEAD_SHA_STUB {
        return CandidateClass::Stub;
    }
    if value.is_empty()
        || value.len() > FULL_SHA_LEN
        || !value.chars().all(|c| c.is_ascii_hexdigit())
    {
        return CandidateClass::Invalid;
    }
    if value.len() == FULL_SHA_LEN {
        return CandidateClass::Exact;
    }
    if value.len() < MIN_PREFIX_LEN {
        return CandidateClass::TooShort;
    }
    CandidateClass::Prefix
}

/// Does `candidate` identify `sha`?
///
/// This is spec §5.2 consequence 1 in one function: the comparison uses the
/// candidate's **own length**, so a 7-char row and a 40-char row are both
/// matched correctly by the same code. Comparison is case-insensitive because
/// git will happily accept and print either case.
///
/// Returns `false` for stubs, too-short and non-hex candidates rather than
/// erroring — the caller counts those separately (see [`EdgeHealth`]); a
/// matcher that panicked on live data would be worse than one that declines it.
pub fn candidate_matches(candidate: &str, sha: &str) -> bool {
    let candidate = candidate.trim();
    match classify_candidate(candidate) {
        CandidateClass::Exact | CandidateClass::Prefix => {}
        _ => return false,
    }
    if sha.len() != FULL_SHA_LEN || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    sha[..candidate.len()].eq_ignore_ascii_case(candidate)
}

/// One resolved provenance edge for one commit.
///
/// Every field that can be unknown is `Option`, and none is defaulted: an edge
/// that knows a task but not a session says so, rather than inventing an empty
/// session id that reads as "no session".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceLink {
    /// One of the `LINK_METHOD_*` constants. Named, never inferred from which
    /// fields happen to be populated.
    pub link_method: String,
    pub confidence: LinkConfidence,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    /// When the evidence was recorded (the event's `created_at`, the task's
    /// `updated_at`).
    pub observed_at: Option<String>,
    /// The abbreviation this edge carried, when it carried one. Reported so a
    /// reader can judge the join rather than trust it.
    pub matched_prefix: Option<String>,
    /// True when [`Self::matched_prefix`] matches more than one indexed commit.
    /// The edge is still returned — §5.2 forbids silently picking a winner —
    /// but at [`LinkConfidence::Low`].
    pub ambiguous: bool,
    /// Every indexed commit the prefix matched, when it matched more than one.
    /// Bounded by the resolver; empty when unambiguous.
    pub ambiguous_candidates: Vec<String>,
}

impl ProvenanceLink {
    /// A link that names a session is the only kind the `commit_links` spine can
    /// be repaired from — that table's `session_id` is `NOT NULL`.
    pub fn names_a_session(&self) -> bool {
        self.session_id.as_deref().is_some_and(|s| !s.is_empty())
    }
}

/// Everything known about one commit's origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitProvenance {
    pub sha: String,
    /// Ordered strongest-first. Empty is a legitimate answer, and is why
    /// [`Self::reason`] exists.
    pub links: Vec<ProvenanceLink>,
    /// Why `links` is empty, when it is. Spec §5.2: never a silent empty.
    pub reason: Option<String>,
}

impl CommitProvenance {
    /// An unlinked commit, carrying the stated reason. Q3 requires these to be
    /// *returned*, not dropped.
    pub fn unlinked(sha: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            sha: sha.into(),
            links: Vec::new(),
            reason: Some(reason.into()),
        }
    }

    /// The best available edge, or `None` when there is none.
    pub fn best(&self) -> Option<&ProvenanceLink> {
        self.links.first()
    }
}

/// The measured state of one provenance edge — how much of it is usable and
/// what was excluded (spec §10.1's "publish both numbers, split by confidence").
///
/// This exists because a bare coverage percentage cannot distinguish "the edge
/// is thin" from "the edge is broken". §5.2's own correction happened because
/// nobody had re-counted a row class.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeHealth {
    /// Which edge, by `LINK_METHOD_*` family name.
    pub edge: String,
    /// Rows carrying a usable identifier.
    pub usable_rows: i64,
    /// Rows carrying the `'?'` degradation sentinel.
    pub excluded_stub: i64,
    /// Rows carrying no identifier at all.
    pub excluded_absent: i64,
    /// Rows carrying an abbreviation below [`MIN_PREFIX_LEN`], or a non-hex
    /// value.
    pub excluded_unusable: i64,
    /// Distinct identifiers among the usable rows — the real join fan-in, which
    /// `usable_rows` overstates because one worker emits many events.
    pub distinct_identifiers: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three widths the live corpus actually contains, plus the boundary
    /// cases either side of [`MIN_PREFIX_LEN`].
    #[test]
    fn classification_covers_every_width_the_corpus_has() {
        assert_eq!(classify_candidate("28ac14c"), CandidateClass::Prefix); // 7 — 594 rows
        assert_eq!(classify_candidate("28ac14cd"), CandidateClass::Prefix); // 8 — 420 rows
        assert_eq!(
            classify_candidate(&"a".repeat(FULL_SHA_LEN)),
            CandidateClass::Exact // 40 — cas-ea51's fix, 4 rows and growing
        );
        assert_eq!(classify_candidate("?"), CandidateClass::Stub); // 46 rows
        assert_eq!(classify_candidate("28ac1"), CandidateClass::TooShort);
        assert_eq!(classify_candidate(""), CandidateClass::Invalid);
        assert_eq!(classify_candidate("none"), CandidateClass::Invalid);
        assert_eq!(
            classify_candidate(&"a".repeat(FULL_SHA_LEN + 1)),
            CandidateClass::Invalid
        );
    }

    /// The regression §5.2 names outright: slicing `sha[0..8]` silently drops
    /// every 7-char row. 594 of the 1,018 usable rows are 7 chars, so a matcher
    /// that fails this test loses 58% of the edge and reports it as a coverage
    /// gap.
    #[test]
    fn a_seven_char_prefix_matches_where_a_fixed_eight_char_slice_would_not() {
        let sha = "28ac14cdeadbeef0123456789abcdef012345678";
        assert!(candidate_matches("28ac14c", sha), "7-char prefix must match");
        assert!(candidate_matches("28ac14cd", sha), "8-char prefix must match");
        assert!(candidate_matches(sha, sha), "40-char exact must match");
        // The failure mode being guarded: the 7-char row is NOT equal to the
        // 8-char slice, which is exactly why the fixed-slice join misses it.
        assert_ne!(&sha[..8], "28ac14c");
    }

    #[test]
    fn the_degradation_stub_is_never_a_sha() {
        let sha = "?abcdef0123456789abcdef0123456789abcdef0";
        // Even against a string that literally starts with '?', the stub must
        // not match: it is a sentinel, and `sha` is not a valid object name.
        assert!(!candidate_matches("?", sha));
        assert!(!candidate_matches("?", "28ac14cdeadbeef0123456789abcdef012345678"));
    }

    #[test]
    fn case_is_not_a_reason_to_miss_a_match() {
        let sha = "28ac14cdeadbeef0123456789abcdef012345678";
        assert!(candidate_matches("28AC14C", sha));
        assert!(candidate_matches(&sha.to_uppercase(), sha));
    }

    #[test]
    fn a_short_or_malformed_candidate_declines_rather_than_matching_loosely() {
        let sha = "28ac14cdeadbeef0123456789abcdef012345678";
        assert!(!candidate_matches("28ac1", sha), "below MIN_PREFIX_LEN");
        assert!(!candidate_matches("", sha));
        assert!(!candidate_matches("zzzzzzz", sha));
        // A non-SHA left-hand side is declined too, so a caller that passes a
        // short_sha by mistake gets false rather than a wrong match.
        assert!(!candidate_matches("28ac14c", "28ac14c"));
    }

    #[test]
    fn confidence_orders_low_below_high() {
        assert!(LinkConfidence::High > LinkConfidence::Medium);
        assert!(LinkConfidence::Medium > LinkConfidence::Low);
        assert_eq!(LinkConfidence::High.as_str(), "high");
    }

    #[test]
    fn an_unlinked_commit_carries_its_reason() {
        let p = CommitProvenance::unlinked("abc", "no populated edge");
        assert!(p.links.is_empty());
        assert_eq!(p.reason.as_deref(), Some("no populated edge"));
        assert!(p.best().is_none());
    }
}
