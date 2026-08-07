//! Baseline-vs-replay comparison.
//!
//! The asymmetry here is deliberate: **losing** a baseline hit is a
//! regression, **gaining** a new hit is not. The migration is allowed to
//! surface more or reorder upward; it is not allowed to make previously
//! retrievable knowledge unreachable.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{Baseline, ChannelStatus, Hit, QueryResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegressionKind {
    /// A baseline hit's content is absent from the replay results entirely.
    MissingHit,
    /// A baseline hit is still present but fell further than the tolerance.
    RankDrop,
    /// A channel that worked at capture time cannot be probed now.
    ChannelLost,
    /// The baseline has a case the replayed query set no longer runs.
    CaseMissing,
}

impl RegressionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RegressionKind::MissingHit => "missing_hit",
            RegressionKind::RankDrop => "rank_drop",
            RegressionKind::ChannelLost => "channel_lost",
            RegressionKind::CaseMissing => "case_missing",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Regression {
    pub case_id: String,
    pub kind: RegressionKind,
    /// Fingerprint of the affected hit, when the regression is hit-scoped.
    pub fp: Option<String>,
    /// Baseline id of the affected hit, for tracing back to the legacy store.
    pub baseline_id: Option<String>,
    pub label: Option<String>,
    pub baseline_rank: Option<usize>,
    pub replay_rank: Option<usize>,
    pub detail: String,
}

/// Per-case outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseReport {
    pub case_id: String,
    pub channel: String,
    pub query: String,
    pub baseline_hits: usize,
    pub replay_hits: usize,
    /// Baseline hits still retrievable within tolerance.
    pub retained: usize,
    /// Hits present in the replay that the baseline did not have. Informational.
    pub new_hits: usize,
    /// Rank tolerance actually applied to this case.
    pub tolerance: usize,
    pub regressions: Vec<Regression>,
}

impl CaseReport {
    pub fn passed(&self) -> bool {
        self.regressions.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub baseline_machine: String,
    pub baseline_captured_at: String,
    pub rank_tolerance: usize,
    pub cases: Vec<CaseReport>,
    /// Corpus-level notes (e.g. entry-count drift) that are not per-case
    /// regressions but are worth eyeballing in review.
    pub notes: Vec<String>,
}

impl Report {
    pub fn total_regressions(&self) -> usize {
        self.cases.iter().map(|c| c.regressions.len()).sum()
    }

    pub fn passed(&self) -> bool {
        self.total_regressions() == 0
    }

    pub fn regressed_cases(&self) -> impl Iterator<Item = &CaseReport> {
        self.cases.iter().filter(|c| !c.passed())
    }

    /// Human-readable report body.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "retrieval parity vs baseline captured {} on {} (rank tolerance {})\n\n",
            self.baseline_captured_at, self.baseline_machine, self.rank_tolerance
        ));
        for case in &self.cases {
            let mark = if case.passed() { "PASS" } else { "REGRESS" };
            out.push_str(&format!(
                "[{mark}] {} ({} \"{}\") baseline={} replay={} retained={} new={}\n",
                case.case_id,
                case.channel,
                case.query,
                case.baseline_hits,
                case.replay_hits,
                case.retained,
                case.new_hits,
            ));
            for r in &case.regressions {
                out.push_str(&format!("         {} — {}\n", r.kind.as_str(), r.detail));
            }
        }
        for note in &self.notes {
            out.push_str(&format!("\nnote: {note}"));
        }
        out.push_str(&format!(
            "\n\n{} case(s), {} passed, {} regression(s)\n",
            self.cases.len(),
            self.cases.iter().filter(|c| c.passed()).count(),
            self.total_regressions()
        ));
        out
    }
}

/// Compare a replay run against a baseline using one tolerance for every case.
pub fn diff_baseline(baseline: &Baseline, replay: &[QueryResult], rank_tolerance: usize) -> Report {
    diff_baseline_with(baseline, replay, rank_tolerance, |_| rank_tolerance)
}

/// Compare a replay run against a baseline, resolving each case's rank
/// tolerance through `tolerance_for` (so a query set's per-case overrides are
/// honoured). `default_tolerance` is recorded in the report header only.
pub fn diff_baseline_with(
    baseline: &Baseline,
    replay: &[QueryResult],
    default_tolerance: usize,
    tolerance_for: impl Fn(&str) -> usize,
) -> Report {
    let by_id: HashMap<&str, &QueryResult> = replay.iter().map(|r| (r.id.as_str(), r)).collect();

    let mut cases = Vec::with_capacity(baseline.results.len());
    for base_case in &baseline.results {
        cases.push(match by_id.get(base_case.id.as_str()) {
            Some(replayed) => diff_case(base_case, replayed, tolerance_for(&base_case.id)),
            // A baseline case with no counterpart means the query set shrank
            // out from under the baseline. Silently skipping it would let
            // someone "fix" a regression by deleting the query that found it.
            None => CaseReport {
                case_id: base_case.id.clone(),
                channel: base_case.channel.to_string(),
                query: base_case.query.clone(),
                baseline_hits: base_case.hits.len(),
                replay_hits: 0,
                retained: 0,
                new_hits: 0,
                tolerance: tolerance_for(&base_case.id),
                regressions: vec![Regression {
                    case_id: base_case.id.clone(),
                    kind: RegressionKind::CaseMissing,
                    fp: None,
                    baseline_id: None,
                    label: None,
                    baseline_rank: None,
                    replay_rank: None,
                    detail: format!(
                        "baseline case '{}' is not in the replayed query set",
                        base_case.id
                    ),
                }],
            },
        });
    }

    let mut notes = Vec::new();
    let base_ids: std::collections::HashSet<&str> =
        baseline.results.iter().map(|r| r.id.as_str()).collect();
    let added: Vec<&str> = replay
        .iter()
        .map(|r| r.id.as_str())
        .filter(|id| !base_ids.contains(id))
        .collect();
    if !added.is_empty() {
        notes.push(format!(
            "{} query case(s) present in the query set but not in the baseline \
             (not compared; re-capture to cover them): {}",
            added.len(),
            added.join(", ")
        ));
    }

    Report {
        baseline_machine: baseline.machine.clone(),
        baseline_captured_at: baseline.captured_at.clone(),
        rank_tolerance: default_tolerance,
        cases,
        notes,
    }
}

fn diff_case(base: &QueryResult, replayed: &QueryResult, tolerance: usize) -> CaseReport {
    let mut regressions = Vec::new();

    // A channel that worked before and does not now is a regression on its
    // own; comparing hit lists past that point would just report every hit as
    // missing and bury the actual cause.
    if base.status.is_ok() && !replayed.status.is_ok() {
        let reason = match &replayed.status {
            ChannelStatus::Unavailable { reason } => reason.clone(),
            ChannelStatus::Ok => unreachable!("guarded by is_ok above"),
        };
        regressions.push(Regression {
            case_id: base.id.clone(),
            kind: RegressionKind::ChannelLost,
            fp: None,
            baseline_id: None,
            label: None,
            baseline_rank: None,
            replay_rank: None,
            detail: format!("channel {} became unavailable: {reason}", base.channel),
        });
        return CaseReport {
            case_id: base.id.clone(),
            channel: base.channel.to_string(),
            query: base.query.clone(),
            baseline_hits: base.hits.len(),
            replay_hits: replayed.hits.len(),
            retained: 0,
            new_hits: 0,
            tolerance,
            regressions,
        };
    }

    // Fingerprint -> best (lowest) rank in the replay. Duplicate content at
    // several ranks counts as retained at its best position.
    let mut replay_ranks: HashMap<&str, usize> = HashMap::new();
    for hit in &replayed.hits {
        replay_ranks
            .entry(hit.fp.as_str())
            .and_modify(|r| *r = (*r).min(hit.rank))
            .or_insert(hit.rank);
    }

    let mut retained = 0usize;
    for base_hit in &base.hits {
        match replay_ranks.get(base_hit.fp.as_str()) {
            None => regressions.push(missing(base, base_hit)),
            Some(&replay_rank) => {
                let drop = replay_rank.saturating_sub(base_hit.rank);
                if drop > tolerance {
                    regressions.push(Regression {
                        case_id: base.id.clone(),
                        kind: RegressionKind::RankDrop,
                        fp: Some(base_hit.fp.clone()),
                        baseline_id: Some(base_hit.id.clone()),
                        label: Some(base_hit.label.clone()),
                        baseline_rank: Some(base_hit.rank),
                        replay_rank: Some(replay_rank),
                        detail: format!(
                            "\"{}\" fell from rank {} to {} ({} positions, tolerance {})",
                            base_hit.label, base_hit.rank, replay_rank, drop, tolerance
                        ),
                    });
                } else {
                    retained += 1;
                }
            }
        }
    }

    let base_fps: std::collections::HashSet<&str> =
        base.hits.iter().map(|h| h.fp.as_str()).collect();
    let new_hits = replay_ranks
        .keys()
        .filter(|fp| !base_fps.contains(*fp))
        .count();

    CaseReport {
        case_id: base.id.clone(),
        channel: base.channel.to_string(),
        query: base.query.clone(),
        baseline_hits: base.hits.len(),
        replay_hits: replayed.hits.len(),
        retained,
        new_hits,
        tolerance,
        regressions,
    }
}

fn missing(base: &QueryResult, hit: &Hit) -> Regression {
    Regression {
        case_id: base.id.clone(),
        kind: RegressionKind::MissingHit,
        fp: Some(hit.fp.clone()),
        baseline_id: Some(hit.id.clone()),
        label: Some(hit.label.clone()),
        baseline_rank: Some(hit.rank),
        replay_rank: None,
        detail: format!(
            "\"{}\" (baseline id {}, rank {}) is no longer retrievable",
            hit.label, hit.id, hit.rank
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval_parity::{BASELINE_VERSION, Channel, CorpusStats};

    fn hit(rank: usize, fp: &str) -> Hit {
        Hit {
            rank,
            id: format!("p-{fp}"),
            fp: fp.to_string(),
            label: format!("memory {fp}"),
            entry_type: "learning".into(),
            tier: "working".into(),
        }
    }

    fn result(id: &str, hits: Vec<Hit>) -> QueryResult {
        QueryResult {
            id: id.to_string(),
            channel: Channel::List,
            query: String::new(),
            status: ChannelStatus::Ok,
            hits,
        }
    }

    fn baseline(results: Vec<QueryResult>) -> Baseline {
        Baseline {
            version: BASELINE_VERSION,
            machine: "test".into(),
            captured_at: "2026-01-01T00:00:00Z".into(),
            cas_dir: "/tmp/.cas".into(),
            corpus: CorpusStats::default(),
            results,
        }
    }

    #[test]
    fn identical_results_are_parity() {
        let hits = vec![hit(0, "aa"), hit(1, "bb")];
        let base = baseline(vec![result("c1", hits.clone())]);
        let report = diff_baseline(&base, &[result("c1", hits)], 3);
        assert!(report.passed(), "{}", report.render());
        assert_eq!(report.cases[0].retained, 2);
        assert_eq!(report.cases[0].new_hits, 0);
    }

    #[test]
    fn deleted_memory_is_reported_as_exactly_one_missing_hit() {
        let base = baseline(vec![result(
            "c1",
            vec![hit(0, "aa"), hit(1, "bb"), hit(2, "cc")],
        )]);
        // "bb" deleted; the survivors close ranks.
        let replayed = result("c1", vec![hit(0, "aa"), hit(1, "cc")]);
        let report = diff_baseline(&base, &[replayed], 3);

        assert!(!report.passed());
        assert_eq!(report.total_regressions(), 1, "{}", report.render());
        let r = &report.cases[0].regressions[0];
        assert_eq!(r.kind, RegressionKind::MissingHit);
        assert_eq!(r.fp.as_deref(), Some("bb"));
        assert_eq!(r.baseline_rank, Some(1));
        assert_eq!(report.cases[0].retained, 2);
    }

    #[test]
    fn id_change_alone_is_not_a_regression() {
        // The whole point of fingerprinting: post-migration re-keying must not
        // read as knowledge loss.
        let base = baseline(vec![result("c1", vec![hit(0, "aa")])]);
        let mut moved = hit(0, "aa");
        moved.id = "knowledge-page-17".into();
        let report = diff_baseline(&base, &[result("c1", vec![moved])], 3);
        assert!(report.passed(), "{}", report.render());
    }

    #[test]
    fn rank_drop_respects_tolerance() {
        let base = baseline(vec![result("c1", vec![hit(0, "aa")])]);
        let replayed = |rank: usize| result("c1", vec![hit(rank, "aa")]);

        let within = diff_baseline(&base, &[replayed(3)], 3);
        assert!(within.passed(), "a drop equal to tolerance passes");

        let beyond = diff_baseline(&base, &[replayed(4)], 3);
        assert_eq!(beyond.total_regressions(), 1);
        let r = &beyond.cases[0].regressions[0];
        assert_eq!(r.kind, RegressionKind::RankDrop);
        assert_eq!(r.replay_rank, Some(4));
    }

    #[test]
    fn rank_improvement_is_never_a_regression() {
        let base = baseline(vec![result("c1", vec![hit(7, "aa")])]);
        let report = diff_baseline(&base, &[result("c1", vec![hit(0, "aa")])], 0);
        assert!(report.passed(), "{}", report.render());
    }

    #[test]
    fn extra_hits_are_informational_only() {
        let base = baseline(vec![result("c1", vec![hit(0, "aa")])]);
        let replayed = result("c1", vec![hit(0, "aa"), hit(1, "zz")]);
        let report = diff_baseline(&base, &[replayed], 3);
        assert!(report.passed());
        assert_eq!(report.cases[0].new_hits, 1);
    }

    #[test]
    fn lost_channel_is_a_regression_and_short_circuits() {
        let base = baseline(vec![result("c1", vec![hit(0, "aa"), hit(1, "bb")])]);
        let mut down = result("c1", vec![]);
        down.status = ChannelStatus::Unavailable {
            reason: "index gone".into(),
        };
        let report = diff_baseline(&base, &[down], 3);
        assert_eq!(
            report.total_regressions(),
            1,
            "one channel_lost, not one-per-hit: {}",
            report.render()
        );
        assert_eq!(
            report.cases[0].regressions[0].kind,
            RegressionKind::ChannelLost
        );
    }

    #[test]
    fn channel_that_was_already_down_is_not_a_new_regression() {
        let mut base_case = result("c1", vec![]);
        base_case.status = ChannelStatus::Unavailable {
            reason: "no index".into(),
        };
        let base = baseline(vec![base_case.clone()]);
        let report = diff_baseline(&base, &[base_case], 3);
        assert!(report.passed(), "{}", report.render());
    }

    #[test]
    fn deleting_a_query_case_cannot_hide_a_regression() {
        let base = baseline(vec![result("c1", vec![hit(0, "aa")])]);
        let report = diff_baseline(&base, &[], 3);
        assert_eq!(report.total_regressions(), 1);
        assert_eq!(
            report.cases[0].regressions[0].kind,
            RegressionKind::CaseMissing
        );
    }

    #[test]
    fn new_query_cases_are_noted_not_failed() {
        let base = baseline(vec![result("c1", vec![hit(0, "aa")])]);
        let report = diff_baseline(
            &base,
            &[result("c1", vec![hit(0, "aa")]), result("c2", vec![])],
            3,
        );
        assert!(report.passed());
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].contains("c2"), "{:?}", report.notes);
    }

    #[test]
    fn duplicate_content_matches_at_its_best_rank() {
        let base = baseline(vec![result("c1", vec![hit(0, "aa")])]);
        let replayed = result("c1", vec![hit(0, "aa"), hit(9, "aa")]);
        let report = diff_baseline(&base, &[replayed], 0);
        assert!(report.passed(), "{}", report.render());
    }
}
