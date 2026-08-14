//! Pre-insert overlap detection for CAS memories.
//!
//! Implements the 4-step workflow from the salvaged skill reference
//! (`overlap-detection.md`) as a pure-Rust function with no MCP or store
//! dependency. Callers extract facets from the new memory, fetch candidate
//! memories through whatever search path is available to them, build
//! `CandidateFacets` for each, and call `check_overlap`. The return value
//! is an `OverlapDecision` the caller acts on.
//!
//! # Workflow
//!
//! 1. **Term extraction** — the caller's job, but `extract_facets_from_body`
//!    is provided as a convenience for the common case of parsing a memory
//!    body into facets. Tokenization prefers reference symbols (file paths,
//!    function/class names, commit SHAs), then symptom strings, then title
//!    tokens.
//! 2. **Candidate selection** — also the caller's job. In cas-cli this is
//!    `SearchIndexTrait::search_candidates_by_module` when the new memory has
//!    a `module`, falling back to `search_for_dedup` otherwise. Callers
//!    should pass at most ~5 candidates.
//! 3. **Dimension scoring** — [`check_overlap`] scores each candidate on 5
//!    dimensions (problem statement, root cause, solution approach,
//!    referenced files, tags), then subtracts 1 for module mismatch and 1
//!    for track mismatch (floor at 0).
//! 4. **Decision** — highest score drives the action:
//!    - **4–5** → [`OverlapDecision::HighOverlap`] — caller must block the
//!      insert and update the existing memory instead.
//!    - **2–3** → [`OverlapDecision::ModerateOverlap`] — caller proceeds with
//!      insert but adds bidirectional cross-references (capped at 3).
//!    - **0–1** → [`OverlapDecision::LowOverlap`] — caller proceeds normally.
//!
//! # Performance
//!
//! All scoring is pure token/set comparison. No embeddings, no I/O. A single
//! call over 5 candidates is sub-millisecond for realistic memory sizes.

use std::collections::{BTreeSet, HashSet};

/// Facets extracted from a new (not-yet-stored) memory. Callers populate
/// this from the memory body + request parameters.
#[derive(Debug, Clone, Default)]
pub struct NewMemoryFacets {
    /// Short title / `name` frontmatter field. Lower-cased by the extractor.
    pub title: String,
    /// One-line description / `description` frontmatter field.
    pub description: String,
    /// Structured `module` field, if present.
    pub module: Option<String>,
    /// Structured `track` field (`bug` or `knowledge`), if present.
    pub track: Option<String>,
    /// Structured `root_cause` field, if present.
    pub root_cause: Option<String>,
    /// Tag set, lowercased.
    pub tags: HashSet<String>,
    /// Reference symbols: file paths, function names, commit SHAs, etc.
    pub file_refs: HashSet<String>,
    /// General body tokens used for problem-statement and solution-shape
    /// comparison. Lowercased, stop-words removed.
    pub body_tokens: HashSet<String>,
}

/// Facets for an existing candidate memory. Mirror of [`NewMemoryFacets`]
/// plus the candidate's slug (entry id) so the decision can point back to
/// the matched memory.
#[derive(Debug, Clone, Default)]
pub struct CandidateFacets {
    /// The candidate entry's id (what overlap decisions reference).
    pub slug: String,
    pub title: String,
    pub description: String,
    pub module: Option<String>,
    pub track: Option<String>,
    pub root_cause: Option<String>,
    pub tags: HashSet<String>,
    pub file_refs: HashSet<String>,
    pub body_tokens: HashSet<String>,
    /// How many slugs are already listed in this candidate's
    /// `related_memories` / `related:*` tags. Used to enforce the
    /// cross-reference cap — once a candidate has ≥3 related entries,
    /// adding another is skipped and the decision carries a
    /// `refresh_recommended` flag instead.
    pub related_count: usize,
}

/// Per-dimension scoring breakdown for a single candidate. Each field is
/// either 0 or 1. `penalty` carries the combined module/track adjustment
/// (always ≤ 0). The net score is what [`OverlapDecision`] acts on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DimensionScores {
    pub problem_statement: u8,
    pub root_cause: u8,
    pub solution_approach: u8,
    pub referenced_files: u8,
    pub tags: u8,
    /// Combined module + track mismatch penalty. Always ≤ 0.
    pub penalty: i8,
}

impl DimensionScores {
    /// Net score after applying the penalty (floored at 0). Ranges 0..=5.
    pub fn net(&self) -> u8 {
        let raw = (self.problem_statement
            + self.root_cause
            + self.solution_approach
            + self.referenced_files
            + self.tags) as i8;
        let net = raw + self.penalty;
        if net < 0 { 0 } else { net as u8 }
    }
}

/// A single candidate match with its dimension breakdown.
#[derive(Debug, Clone)]
pub struct OverlapMatch {
    pub slug: String,
    pub scores: DimensionScores,
    /// A bounded, deterministic sample of the lexical content shared by the
    /// two memories. This makes a high-overlap refusal auditable instead of
    /// asking the caller to infer why a numeric score was assigned.
    pub matched_terms: Vec<String>,
    /// True if appending a cross-reference to this candidate would exceed
    /// the 3-link cap. Caller should skip the mutation and surface a
    /// refresh recommendation instead.
    pub cap_reached: bool,
}

/// Recommended follow-up action for a high-overlap match. The caller maps
/// this into its own contract (MCP response, CLI output, etc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapRecommendation {
    /// Update the existing memory in place with the new content. Default
    /// for headless callers.
    UpdateExisting,
    /// Let the user decide interactively. Default for interactive callers.
    SurfaceForUserDecision,
}

/// The decision returned by [`check_overlap`]. Callers act on this.
#[derive(Debug, Clone)]
pub enum OverlapDecision {
    /// No candidate scored above threshold. Caller should insert normally.
    LowOverlap,

    /// One or more candidates scored 2–3. Caller should insert and add
    /// bidirectional cross-references to each entry in `links`. If a
    /// candidate's cross-reference would exceed the cap, the link still
    /// appears in `links` (so the new memory records the relationship) but
    /// its `cap_reached` flag is set; callers should skip mutating that
    /// candidate and set `refresh_recommended` on their response.
    ModerateOverlap {
        links: Vec<OverlapMatch>,
        refresh_recommended: bool,
    },

    /// Highest candidate scored 4–5. Caller must BLOCK the insert and
    /// follow the recommendation (typically: update the existing memory in
    /// place).
    HighOverlap {
        best: OverlapMatch,
        all_high_scoring: Vec<OverlapMatch>,
        recommendation: OverlapRecommendation,
    },
}

/// Maximum number of cross-references a single memory is allowed to
/// accumulate. Beyond this, new cross-refs are skipped and a refresh is
/// recommended instead.
pub const CROSS_REF_CAP: usize = 3;

/// Stop-words dropped during token extraction. Tiny list — overlap scoring
/// favors high-signal tokens (symbols, error strings) anyway.
const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "to", "of", "in", "on", "for", "with", "is", "was",
    "were", "be", "been", "this", "that", "it", "as", "at", "by", "from", "not", "no", "so", "do",
    "does", "did", "has", "have", "had", "are", "am", "will", "would", "can", "could", "should",
    "may", "might", "we", "you", "our", "your", "their", "they", "them", "there", "here", "when",
    "where", "which", "who", "what", "how", "why", "about", "into", "than", "then", "some", "any",
    "all",
];

/// Tokenize a raw text blob into lowercased non-stop-word tokens. Splits on
/// whitespace and ASCII punctuation (preserving `_`, `-`, `.`, `/` to keep
/// file-path / symbol shapes intact).
pub fn tokenize(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '.' || ch == '/' {
            current.push(ch.to_ascii_lowercase());
        } else {
            if !current.is_empty() {
                push_token(&mut out, &current);
                current.clear();
            }
        }
    }
    if !current.is_empty() {
        push_token(&mut out, &current);
    }
    out
}

fn push_token(out: &mut HashSet<String>, tok: &str) {
    if tok.len() < 3 {
        return;
    }
    if STOP_WORDS.contains(&tok) {
        return;
    }
    out.insert(tok.to_string());
}

/// Heuristic test for "this token looks like a reference symbol" — file
/// paths, dotted identifiers, snake_case, CamelCase, commit SHAs. These
/// are the most discriminating tokens for overlap detection.
pub fn looks_like_symbol(tok: &str) -> bool {
    let has_path_sep = tok.contains('/') || tok.contains('.');
    let has_underscore = tok.contains('_');
    let has_upper = tok.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = tok.chars().any(|c| c.is_ascii_lowercase());
    let looks_sha = tok.len() >= 7
        && tok.len() <= 40
        && tok.chars().all(|c| c.is_ascii_hexdigit())
        && tok.chars().any(|c| c.is_ascii_digit())
        && tok.chars().any(|c| c.is_ascii_alphabetic());
    // A hyphen alone is ordinary prose ("factory-session", "branch-ci"),
    // not a discriminating file/API reference. Treating it as a symbol made
    // common operational vocabulary triple-count as root cause, solution,
    // and referenced-file evidence.
    has_path_sep || has_underscore || (has_upper && has_lower) || looks_sha
}

/// Convenience extractor: given a memory body (with or without frontmatter),
/// return `(body_tokens, file_refs)` where `file_refs` is the subset of
/// tokens that look like reference symbols. The caller can then build a
/// [`NewMemoryFacets`] or [`CandidateFacets`] by adding title/description/
/// module/track/tags from its own sources.
pub fn extract_facets_from_body(body: &str) -> (HashSet<String>, HashSet<String>) {
    // Strip YAML frontmatter if present — we score on body content, not
    // frontmatter keys. Matches cas-cli/src/store/markdown.rs:37.
    let stripped = strip_frontmatter(body);
    let tokens = tokenize(stripped);
    let refs: HashSet<String> = tokens.iter().filter(|t| looks_like_symbol(t)).cloned().collect();
    (tokens, refs)
}

fn strip_frontmatter(body: &str) -> &str {
    let t = body.trim_start();
    if !t.starts_with("---") {
        return body;
    }
    let parts: Vec<&str> = t.splitn(3, "---").collect();
    if parts.len() < 3 { body } else { parts[2] }
}

/// Run the 4-step overlap check. Pure function; does no I/O.
///
/// See module docs for workflow details and [`OverlapDecision`] for the
/// action table.
pub fn check_overlap(
    new: &NewMemoryFacets,
    candidates: &[CandidateFacets],
    interactive: bool,
) -> OverlapDecision {
    if candidates.is_empty() {
        return OverlapDecision::LowOverlap;
    }

    let mut scored: Vec<OverlapMatch> = candidates
        .iter()
        .map(|cand| {
            let scores = score_candidate(new, cand);
            OverlapMatch {
                slug: cand.slug.clone(),
                scores,
                matched_terms: matched_terms(new, cand),
                cap_reached: cand.related_count >= CROSS_REF_CAP,
            }
        })
        .collect();

    // Sort by net score descending — ties broken by file_refs dim (more
    // discriminating) then tags.
    scored.sort_by(|a, b| {
        b.scores.net().cmp(&a.scores.net()).then_with(|| {
            b.scores
                .referenced_files
                .cmp(&a.scores.referenced_files)
                .then_with(|| b.scores.tags.cmp(&a.scores.tags))
        })
    });

    let best_net = scored[0].scores.net();

    if best_net >= 4 {
        let all_high: Vec<OverlapMatch> =
            scored.iter().filter(|m| m.scores.net() >= 4).cloned().collect();
        return OverlapDecision::HighOverlap {
            best: scored.into_iter().next().unwrap(),
            all_high_scoring: all_high,
            recommendation: if interactive {
                OverlapRecommendation::SurfaceForUserDecision
            } else {
                OverlapRecommendation::UpdateExisting
            },
        };
    }

    if best_net >= 2 {
        // Take up to 3 matches with score 2–3.
        let mut links: Vec<OverlapMatch> = scored
            .into_iter()
            .filter(|m| m.scores.net() >= 2 && m.scores.net() <= 3)
            .take(CROSS_REF_CAP)
            .collect();
        let refresh_recommended = links.iter().any(|m| m.cap_reached);
        // Keep the deterministic descending order from the sort.
        links.sort_by(|a, b| b.scores.net().cmp(&a.scores.net()));
        return OverlapDecision::ModerateOverlap {
            links,
            refresh_recommended,
        };
    }

    OverlapDecision::LowOverlap
}

fn score_candidate(new: &NewMemoryFacets, cand: &CandidateFacets) -> DimensionScores {
    let mut s = DimensionScores::default();

    // --- Dimension 1: problem statement ---
    // Compare title + description tokens. A hit is ≥2 shared significant
    // tokens OR an exact-match on lowercased title.
    let new_problem = problem_tokens(&new.title, &new.description);
    let cand_problem = problem_tokens(&cand.title, &cand.description);
    let shared_problem: usize = new_problem.intersection(&cand_problem).count();
    if shared_problem >= 2 || (!new.title.is_empty() && new.title == cand.title) {
        s.problem_statement = 1;
    }

    // --- Dimension 2: root cause ---
    // Exact match on the `root_cause` enum value. When both sides lack
    // structured cause data, use a conservative whole-body similarity floor:
    // two shared labels must not impersonate the same causal explanation.
    match (new.root_cause.as_deref(), cand.root_cause.as_deref()) {
        (Some(a), Some(b)) if a == b => s.root_cause = 1,
        (None, None) => {
            if body_similarity(new, cand) >= 0.70 {
                s.root_cause = 1;
            }
        }
        _ => {}
    }

    // --- Dimension 3: solution approach ---
    // "Same fix shape" requires substantial body similarity, or two true
    // symbols (paths, APIs, flags) in common. Hyphenated prose is excluded
    // by `looks_like_symbol`, so shared factory labels cannot claim the same
    // solution while real implementation references still cross-link.
    let shared_body_symbols = new
        .body_tokens
        .intersection(&cand.body_tokens)
        .filter(|term| looks_like_symbol(term))
        .count();
    if body_similarity(new, cand) >= 0.50 || shared_body_symbols >= 2 {
        s.solution_approach = 1;
    }

    // --- Dimension 4: referenced files ---
    // 2+ shared file_refs OR 1 shared file_ref if it's central to both
    // (approximation: the only file_ref on one side).
    let shared_refs: HashSet<&String> = new.file_refs.intersection(&cand.file_refs).collect();
    if shared_refs.len() >= 2 {
        s.referenced_files = 1;
    } else if shared_refs.len() == 1 && (new.file_refs.len() == 1 || cand.file_refs.len() == 1) {
        s.referenced_files = 1;
    }

    // --- Dimension 5: tags ---
    // 2+ shared tags, OR 1 shared tag if it's highly specific (heuristic:
    // contains a hyphen or underscore, or length ≥ 5). Generic tags like
    // "bug" or "mcp" should not fire a hit on their own.
    let shared_tags: HashSet<&String> = new.tags.intersection(&cand.tags).collect();
    if shared_tags.len() >= 2 {
        s.tags = 1;
    } else if shared_tags.len() == 1 {
        let tag = *shared_tags.iter().next().unwrap();
        if tag.contains('-') || tag.contains('_') || tag.len() >= 5 {
            s.tags = 1;
        }
    }

    // --- Penalties ---
    // Module mismatch: both sides have a module AND they differ.
    if let (Some(nm), Some(cm)) = (&new.module, &cand.module) {
        if nm != cm {
            s.penalty -= 1;
        }
    }
    // Track mismatch: both sides have a track AND they differ.
    if let (Some(nt), Some(ct)) = (&new.track, &cand.track) {
        if nt != ct {
            s.penalty -= 1;
        }
    }

    s
}

fn problem_tokens(title: &str, description: &str) -> HashSet<String> {
    let mut combined = String::with_capacity(title.len() + description.len() + 1);
    combined.push_str(title);
    combined.push(' ');
    combined.push_str(description);
    tokenize(&combined)
}

/// Jaccard similarity over the memory body after existing stop-word removal.
/// This is deliberately content-based: metadata quality or a pair of shared
/// labels cannot by themselves make unrelated learnings duplicates.
fn body_similarity(new: &NewMemoryFacets, cand: &CandidateFacets) -> f32 {
    let union = new.body_tokens.union(&cand.body_tokens).count();
    if union == 0 {
        return 0.0;
    }
    new.body_tokens.intersection(&cand.body_tokens).count() as f32 / union as f32
}

fn matched_terms(new: &NewMemoryFacets, cand: &CandidateFacets) -> Vec<String> {
    let mut terms: BTreeSet<String> = new
        .body_tokens
        .intersection(&cand.body_tokens)
        .cloned()
        .collect();
    terms.extend(
        problem_tokens(&new.title, &new.description)
            .intersection(&problem_tokens(&cand.title, &cand.description))
            .cloned(),
    );
    terms.into_iter().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn set(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn base_new() -> NewMemoryFacets {
        NewMemoryFacets {
            title: "sqlite wal on ntfs3 causes mcp timeout".to_string(),
            description: "wal-mode sqlite hangs on ntfs3 during concurrent opens".to_string(),
            module: Some("cas-mcp".to_string()),
            track: Some("bug".to_string()),
            root_cause: Some("platform_incompatibility".to_string()),
            tags: tags(&["sqlite-wal", "ntfs3", "mcp-timeout"]),
            file_refs: set(&["cas-cli/src/store/markdown.rs", "cas-mcp/src/server.rs"]),
            body_tokens: set(&[
                "sqlite",
                "wal",
                "ntfs3",
                "posix_lock",
                "cas-cli/src/store/markdown.rs",
                "cas-mcp/src/server.rs",
                "shm_file",
                "timeout",
            ]),
        }
    }

    fn clone_as_candidate(n: &NewMemoryFacets, slug: &str, related: usize) -> CandidateFacets {
        CandidateFacets {
            slug: slug.to_string(),
            title: n.title.clone(),
            description: n.description.clone(),
            module: n.module.clone(),
            track: n.track.clone(),
            root_cause: n.root_cause.clone(),
            tags: n.tags.clone(),
            file_refs: n.file_refs.clone(),
            body_tokens: n.body_tokens.clone(),
            related_count: related,
        }
    }

    #[test]
    fn empty_candidates_is_low() {
        let n = base_new();
        match check_overlap(&n, &[], false) {
            OverlapDecision::LowOverlap => {}
            d => panic!("expected LowOverlap, got {d:?}"),
        }
    }

    #[test]
    fn perfect_clone_is_high_overlap_5() {
        let n = base_new();
        let c = clone_as_candidate(&n, "mem-001", 0);
        let decision = check_overlap(&n, &[c], false);
        match decision {
            OverlapDecision::HighOverlap {
                best,
                recommendation,
                ..
            } => {
                assert_eq!(best.slug, "mem-001");
                assert_eq!(best.scores.net(), 5);
                assert_eq!(recommendation, OverlapRecommendation::UpdateExisting);
            }
            d => panic!("expected HighOverlap, got {d:?}"),
        }
    }

    #[test]
    fn interactive_mode_surfaces_decision() {
        let n = base_new();
        let c = clone_as_candidate(&n, "mem-002", 0);
        let decision = check_overlap(&n, &[c], true);
        if let OverlapDecision::HighOverlap { recommendation, .. } = decision {
            assert_eq!(recommendation, OverlapRecommendation::SurfaceForUserDecision);
        } else {
            panic!("expected HighOverlap");
        }
    }

    #[test]
    fn module_mismatch_subtracts_one() {
        let n = base_new();
        let mut c = clone_as_candidate(&n, "mem-003", 0);
        c.module = Some("cas-core".to_string());
        let decision = check_overlap(&n, &[c], false);
        // Raw 5 - penalty 1 = 4, still HighOverlap.
        match decision {
            OverlapDecision::HighOverlap { best, .. } => {
                assert_eq!(best.scores.net(), 4);
                assert_eq!(best.scores.penalty, -1);
            }
            d => panic!("expected HighOverlap(4), got {d:?}"),
        }
    }

    #[test]
    fn track_and_module_mismatch_demotes_to_moderate() {
        let n = base_new();
        let mut c = clone_as_candidate(&n, "mem-004", 0);
        c.module = Some("cas-core".to_string());
        c.track = Some("knowledge".to_string());
        let decision = check_overlap(&n, &[c], false);
        // Raw 5 - 2 penalty = 3, ModerateOverlap.
        match decision {
            OverlapDecision::ModerateOverlap { links, .. } => {
                assert_eq!(links.len(), 1);
                assert_eq!(links[0].scores.net(), 3);
                assert_eq!(links[0].scores.penalty, -2);
            }
            d => panic!("expected ModerateOverlap, got {d:?}"),
        }
    }

    #[test]
    fn low_overlap_when_only_weak_signal() {
        let mut n = base_new();
        n.tags.clear();
        n.tags.insert("bug".to_string()); // generic single-word tag
        n.file_refs.clear();
        n.body_tokens.clear();
        n.body_tokens.insert("hello".to_string());

        let mut c = CandidateFacets::default();
        c.slug = "mem-005".to_string();
        c.title = "totally unrelated memory".to_string();
        c.description = "another topic entirely".to_string();
        c.module = Some("cas-core".to_string());
        c.track = Some("knowledge".to_string());
        c.tags.insert("bug".to_string());
        c.body_tokens.insert("world".to_string());

        match check_overlap(&n, &[c], false) {
            OverlapDecision::LowOverlap => {}
            d => panic!("expected LowOverlap, got {d:?}"),
        }
    }

    #[test]
    fn moderate_overlap_cross_ref_cap_triggers_refresh() {
        let n = base_new();
        // 2 dimensions match cleanly, the rest don't.
        let mut c = CandidateFacets::default();
        c.slug = "mem-006".to_string();
        c.title = n.title.clone();
        c.description = n.description.clone();
        c.file_refs = n.file_refs.clone();
        c.module = n.module.clone();
        c.track = n.track.clone();
        c.related_count = CROSS_REF_CAP; // already at cap

        let decision = check_overlap(&n, &[c], false);
        match decision {
            OverlapDecision::ModerateOverlap {
                links,
                refresh_recommended,
            } => {
                assert_eq!(links.len(), 1);
                assert!(links[0].cap_reached);
                assert!(refresh_recommended);
                let net = links[0].scores.net();
                assert!(net >= 2 && net <= 3, "expected 2-3, got {net}");
            }
            d => panic!("expected ModerateOverlap with cap, got {d:?}"),
        }
    }

    #[test]
    fn moderate_overlap_under_cap_no_refresh() {
        let n = base_new();
        let mut c = CandidateFacets::default();
        c.slug = "mem-007".to_string();
        c.title = n.title.clone();
        c.description = n.description.clone();
        c.file_refs = n.file_refs.clone();
        c.module = n.module.clone();
        c.track = n.track.clone();
        c.related_count = 1;

        let decision = check_overlap(&n, &[c], false);
        match decision {
            OverlapDecision::ModerateOverlap {
                refresh_recommended,
                links,
            } => {
                assert!(!refresh_recommended);
                assert!(!links[0].cap_reached);
            }
            d => panic!("expected ModerateOverlap, got {d:?}"),
        }
    }

    #[test]
    fn moderate_overlap_caps_at_three_links() {
        let n = base_new();
        let mk = |slug: &str| {
            let mut c = CandidateFacets::default();
            c.slug = slug.to_string();
            c.title = n.title.clone();
            c.description = n.description.clone();
            c.file_refs = n.file_refs.clone();
            c.module = n.module.clone();
            c.track = n.track.clone();
            c
        };
        let cands = vec![mk("a"), mk("b"), mk("c"), mk("d"), mk("e")];
        let decision = check_overlap(&n, &cands, false);
        match decision {
            OverlapDecision::ModerateOverlap { links, .. } => {
                assert_eq!(links.len(), CROSS_REF_CAP);
            }
            d => panic!("expected ModerateOverlap, got {d:?}"),
        }
    }

    #[test]
    fn multiple_high_scoring_candidates_reported() {
        let n = base_new();
        let c1 = clone_as_candidate(&n, "mem-008a", 0);
        let c2 = clone_as_candidate(&n, "mem-008b", 0);
        let decision = check_overlap(&n, &[c1, c2], false);
        match decision {
            OverlapDecision::HighOverlap {
                all_high_scoring, ..
            } => {
                assert_eq!(all_high_scoring.len(), 2);
            }
            d => panic!("expected HighOverlap, got {d:?}"),
        }
    }

    #[test]
    fn penalty_never_produces_negative_net() {
        let mut s = DimensionScores::default();
        s.problem_statement = 1;
        s.penalty = -3;
        assert_eq!(s.net(), 0);
    }

    #[test]
    fn tokenize_drops_stop_words_and_shorts() {
        let toks = tokenize("The quick brown fox is an animal");
        assert!(!toks.contains("the"));
        assert!(!toks.contains("is"));
        assert!(!toks.contains("an"));
        assert!(toks.contains("quick"));
        assert!(toks.contains("brown"));
        assert!(toks.contains("animal"));
    }

    #[test]
    fn tokenize_preserves_file_paths_and_identifiers() {
        let toks = tokenize("See cas-mcp/src/server.rs and fn handle_remember_request()");
        assert!(toks.contains("cas-mcp/src/server.rs"));
        assert!(toks.contains("handle_remember_request"));
    }

    #[test]
    fn looks_like_symbol_catches_paths_and_snake_case() {
        assert!(looks_like_symbol("cas-mcp/src/server.rs"));
        assert!(looks_like_symbol("handle_remember_request"));
        assert!(looks_like_symbol("CasCore"));
        assert!(looks_like_symbol("2dfe2aa1234567"));
        assert!(!looks_like_symbol("hello"));
        assert!(!looks_like_symbol("world"));
        assert!(
            !looks_like_symbol("factory-session"),
            "ordinary hyphenated prose is not a file/API symbol"
        );
    }

    #[test]
    fn extract_facets_strips_frontmatter() {
        let body =
            "---\nname: x\nmodule: cas-mcp\n---\n\n## Problem\nsqlite hangs in cas-mcp/server.rs";
        let (tokens, refs) = extract_facets_from_body(body);
        assert!(tokens.contains("sqlite"));
        assert!(tokens.contains("cas-mcp/server.rs"));
        assert!(!tokens.contains("module")); // frontmatter was stripped
        assert!(refs.contains("cas-mcp/server.rs"));
    }

    #[test]
    fn generic_single_tag_does_not_score() {
        let mut n = NewMemoryFacets::default();
        n.tags.insert("bug".to_string());
        let mut c = CandidateFacets::default();
        c.slug = "mem-tag".to_string();
        c.tags.insert("bug".to_string());
        let s = score_candidate(&n, &c);
        assert_eq!(s.tags, 0, "generic 'bug' tag should not fire");
    }

    #[test]
    fn specific_single_tag_scores() {
        let mut n = NewMemoryFacets::default();
        n.tags.insert("sqlite-wal".to_string());
        let mut c = CandidateFacets::default();
        c.slug = "mem-spec".to_string();
        c.tags.insert("sqlite-wal".to_string());
        let s = score_candidate(&n, &c);
        assert_eq!(s.tags, 1);
    }

    #[test]
    fn single_file_ref_central_scores() {
        let mut n = NewMemoryFacets::default();
        n.file_refs.insert("cas-mcp/src/server.rs".to_string());
        let mut c = CandidateFacets::default();
        c.slug = "mem-file".to_string();
        c.file_refs.insert("cas-mcp/src/server.rs".to_string());
        let s = score_candidate(&n, &c);
        assert_eq!(s.referenced_files, 1);
    }

    #[test]
    fn root_cause_enum_match_scores() {
        let mut n = NewMemoryFacets::default();
        n.root_cause = Some("race_condition".to_string());
        let mut c = CandidateFacets::default();
        c.slug = "mem-rc".to_string();
        c.root_cause = Some("race_condition".to_string());
        let s = score_candidate(&n, &c);
        assert_eq!(s.root_cause, 1);
    }

    #[test]
    fn root_cause_enum_mismatch_does_not_score() {
        let mut n = NewMemoryFacets::default();
        n.root_cause = Some("race_condition".to_string());
        let mut c = CandidateFacets::default();
        c.slug = "mem-rc2".to_string();
        c.root_cause = Some("config_error".to_string());
        let s = score_candidate(&n, &c);
        assert_eq!(s.root_cause, 0);
    }

    #[test]
    fn module_match_no_penalty() {
        let n = base_new();
        let c = clone_as_candidate(&n, "mem-m", 0);
        let s = score_candidate(&n, &c);
        assert_eq!(s.penalty, 0);
    }

    #[test]
    fn one_side_no_module_no_penalty() {
        let n = base_new();
        let mut c = clone_as_candidate(&n, "mem-nm", 0);
        c.module = None;
        let s = score_candidate(&n, &c);
        assert_eq!(s.penalty, 0);
    }

    #[test]
    fn cas_8c16_shared_factory_vocabulary_is_not_high_overlap() {
        let mut new = NewMemoryFacets {
            title: "worker context headroom scheduling".to_string(),
            ..Default::default()
        };
        new.body_tokens = tokenize(
            "Treat worker context headroom as a scheduling input. A factory-session handoff and shared-worker review do not decide capacity policy.",
        );

        let mut candidate = CandidateFacets {
            slug: "merge-backlog".to_string(),
            title: "worker context for merge-lane backlog coordination".to_string(),
            ..Default::default()
        };
        candidate.body_tokens = tokenize(
            "Two supervisors can collide on a merge-lane backlog before a factory-session handoff and shared-worker review.",
        );

        match check_overlap(&new, &[candidate], false) {
            OverlapDecision::LowOverlap => {}
            other => panic!("distinct session learning must store, got {other:?}"),
        }
    }

    #[test]
    fn cas_8c16_near_identical_unstructured_memory_is_still_high_and_auditable() {
        let title = "merge lane only on branch ci";
        let body = "merge a lane only on its own branch ci after factory-session handoff";
        let (body_tokens, file_refs) = extract_facets_from_body(body);
        let new = NewMemoryFacets {
            title: title.to_string(),
            tags: tags(&["branch-ci", "lane-ownership"]),
            body_tokens: body_tokens.clone(),
            file_refs: file_refs.clone(),
            ..Default::default()
        };
        let candidate = CandidateFacets {
            slug: "branch-ci-policy".to_string(),
            title: title.to_string(),
            tags: tags(&["branch-ci", "lane-ownership"]),
            body_tokens,
            file_refs,
            ..Default::default()
        };

        match check_overlap(&new, &[candidate], false) {
            OverlapDecision::HighOverlap { best, .. } => {
                assert!(best.scores.net() >= 4);
                assert!(best.matched_terms.contains(&"branch".to_string()));
                assert!(best.matched_terms.contains(&"lane".to_string()));
            }
            other => panic!("near-identical memory must be blocked, got {other:?}"),
        }
    }
}
