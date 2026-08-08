//! Binary-epoch classification and the three-valued is-it-fixed verdict
//! (EPIC cas-6212 / cas-8d2a, spec §9 + §12 Q6).
//!
//! # The trap this module exists to retire
//!
//! A fix's *tag date* is not when the fix started running. cas-9d92 Phase 1
//! read a 33.5% → 19.3% undelivered-rate drop as v2.49.0's fix working; the
//! retraction established that the binary was installed at 21:02:26Z while
//! **pre-install daemons kept heartbeating until 21:36:37Z**. Everything in
//! that window was served by a mixture of both binaries and cannot be read as
//! post-fix evidence.
//!
//! So the timeline is three-valued, never two:
//!
//! ```text
//! CLEAN-PRE   : t < fix_started_running
//! MIXED       : fix_started_running <= t < last_heartbeat_of_any_older_binary
//! CLEAN-POST  : t >= last_heartbeat_of_any_older_binary
//! ```
//!
//! and so is the verdict, because "no symptom in a 45-minute window" is not
//! evidence of a fix. [`FixVerdict::InsufficientPostFixData`] is the direct
//! encoding of cas-9d92's own stated limit; a system that collapses it into
//! `FIXED-VERIFIED` reproduces the retracted finding automatically, at scale.
//!
//! # Why `binary_mtime`, not `version`
//!
//! Two builds on a release day carry the same `CARGO_PKG_VERSION`. The mtime of
//! the executable is what actually separates them, so "is this epoch running
//! the fix?" is answered as `binary_mtime >= fix_built_at`. An epoch with an
//! unknown binary (every backfilled row — `daemon_instances` records no binary
//! identity) can therefore **extend the MIXED window but never open a
//! CLEAN-POST one**. That asymmetry is deliberate: unknown must cost evidence,
//! not manufacture it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use cas_store::{EPOCH_KIND_DAEMON_START, HistoryEpoch};

/// Default `INSUFFICIENT-POST-FIX-DATA` threshold: post-boundary observations
/// required before an absence of the symptom counts as verification (spec §9).
pub const DEFAULT_SAMPLE_THRESHOLD: i64 = 100;

/// Where a timestamp falls on the binary timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EpochClass {
    CleanPre,
    /// Both binaries were serving. **Never** counted as post-fix — a hard rule
    /// in this layer, not a convention (spec §9).
    Mixed,
    CleanPost,
}

impl EpochClass {
    pub fn as_str(self) -> &'static str {
        match self {
            EpochClass::CleanPre => "CLEAN-PRE",
            EpochClass::Mixed => "MIXED",
            EpochClass::CleanPost => "CLEAN-POST",
        }
    }
}

/// The three-valued (in practice four-valued) answer to "is symptom X fixed",
/// per the spec §9 verdict table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixVerdict {
    /// Symptom occurs in CLEAN-POST.
    StillLive,
    /// No CLEAN-POST occurrence and the CLEAN-POST sample cleared the threshold.
    FixedVerified,
    /// No CLEAN-POST occurrence but too little post-boundary data to say so.
    InsufficientPostFixData,
    /// The fixed binary has not been observed running yet, so there is no
    /// CLEAN-POST epoch to look in at all.
    FixedUnverified,
}

impl FixVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            FixVerdict::StillLive => "STILL-LIVE",
            FixVerdict::FixedVerified => "FIXED-VERIFIED",
            FixVerdict::InsufficientPostFixData => "INSUFFICIENT-POST-FIX-DATA",
            FixVerdict::FixedUnverified => "FIXED-UNVERIFIED",
        }
    }

    /// Whether this verdict may be reported to a human as "fixed" without a
    /// qualifier. Exactly one variant may.
    pub fn is_affirmative(self) -> bool {
        matches!(self, FixVerdict::FixedVerified)
    }
}

/// The computed boundaries of a fix's binary timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochBoundary {
    /// When the fixed binary was first observed *running* (an epoch start),
    /// not when it was built or tagged. `None` when no epoch is running a
    /// binary at least as new as the fix.
    pub fix_started_running: Option<DateTime<Utc>>,
    /// Start of the CLEAN-POST window: the last observed liveness of any epoch
    /// that began before the fix started running. `None` for the same reason
    /// `fix_started_running` is.
    pub clean_post_from: Option<DateTime<Utc>>,
    /// Epochs that began before the fix and were still observed alive after it
    /// — the processes that make the window MIXED.
    pub overlapping_older_epochs: usize,
    /// Epochs considered at all (parseable `started_at`).
    pub epochs_considered: usize,
    /// Epochs skipped because their binary identity is unknown, so they could
    /// not be used to open a CLEAN-POST window. Reported, never hidden.
    pub epochs_without_binary_identity: usize,
}

impl EpochBoundary {
    /// Classify one instant against this boundary. `None` when there is no
    /// post-fix epoch yet, in which case nothing can be CLEAN-POST.
    pub fn classify(&self, at: DateTime<Utc>) -> Option<EpochClass> {
        let fix = self.fix_started_running?;
        let clean = self.clean_post_from?;
        Some(if at < fix {
            EpochClass::CleanPre
        } else if at < clean {
            EpochClass::Mixed
        } else {
            EpochClass::CleanPost
        })
    }
}

fn parse(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// The last instant an epoch was observed alive.
///
/// Falls back to `started_at` when `ended_at` is absent: a process observed
/// only at start is evidence of liveness *at that instant*, and inventing an
/// open-ended tail from it would push the CLEAN-POST boundary to infinity for
/// every daemon that died before its first heartbeat.
fn last_alive(epoch: &HistoryEpoch) -> Option<DateTime<Utc>> {
    epoch
        .ended_at
        .as_deref()
        .and_then(parse)
        .or_else(|| parse(&epoch.started_at))
}

/// Compute the boundary for a fix whose binary was built at `fix_built_at`.
///
/// `fix_built_at` is a *build/commit* time, deliberately — feeding it a tag
/// date and getting a boundary out is the whole point: the answer comes back
/// as the first epoch whose binary is at least that new, so a caller cannot
/// accidentally treat the tag date itself as the start of post-fix data.
pub fn boundary_for(epochs: &[HistoryEpoch], fix_built_at: DateTime<Utc>) -> EpochBoundary {
    let mut considered = 0usize;
    let mut unknown_binary = 0usize;
    let mut fix_started: Option<DateTime<Utc>> = None;

    for epoch in epochs {
        if epoch.epoch_kind != EPOCH_KIND_DAEMON_START {
            continue;
        }
        let Some(started) = parse(&epoch.started_at) else {
            continue;
        };
        considered += 1;
        match epoch.binary_mtime.as_deref().and_then(parse) {
            Some(mtime) if mtime >= fix_built_at => {
                fix_started = Some(match fix_started {
                    Some(existing) => existing.min(started),
                    None => started,
                });
            }
            Some(_) => {}
            None => unknown_binary += 1,
        }
    }

    let Some(fix_started) = fix_started else {
        return EpochBoundary {
            fix_started_running: None,
            clean_post_from: None,
            overlapping_older_epochs: 0,
            epochs_considered: considered,
            epochs_without_binary_identity: unknown_binary,
        };
    };

    // The tail of every epoch that began before the fix started running,
    // whether or not we know which binary it was serving. An unknown-binary
    // daemon that was alive into the window is exactly the cas-9d92 shape.
    let mut clean_post_from = fix_started;
    let mut overlapping = 0usize;
    for epoch in epochs {
        if epoch.epoch_kind != EPOCH_KIND_DAEMON_START {
            continue;
        }
        let Some(started) = parse(&epoch.started_at) else {
            continue;
        };
        if started >= fix_started {
            continue;
        }
        let Some(alive_until) = last_alive(epoch) else {
            continue;
        };
        if alive_until > fix_started {
            overlapping += 1;
            clean_post_from = clean_post_from.max(alive_until);
        }
    }

    EpochBoundary {
        fix_started_running: Some(fix_started),
        clean_post_from: Some(clean_post_from),
        overlapping_older_epochs: overlapping,
        epochs_considered: considered,
        epochs_without_binary_identity: unknown_binary,
    }
}

/// A complete is-it-fixed answer. Every field the verdict rests on travels
/// with it — the caller is never handed a bare "fixed" (spec §9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixAssessment {
    pub verdict: FixVerdict,
    pub boundary: EpochBoundary,
    /// Symptom occurrences inside CLEAN-POST.
    pub clean_post_matches: i64,
    /// Total observations inside CLEAN-POST — the denominator the threshold is
    /// tested against.
    pub clean_post_sample: i64,
    /// Symptom occurrences inside the MIXED window. Reported so the caller can
    /// see the evidence that was *excluded*, rather than wondering why the
    /// verdict is not stronger.
    pub mixed_matches: i64,
    pub mixed_sample: i64,
    pub threshold: i64,
    /// One sentence a human can read without re-deriving the table.
    pub rationale: String,
}

/// Apply the spec §9 verdict table.
///
/// `clean_post` / `mixed` are `(matches, sample)` pairs already measured over
/// the corresponding windows.
pub fn assess(
    boundary: EpochBoundary,
    clean_post: (i64, i64),
    mixed: (i64, i64),
    threshold: i64,
) -> FixAssessment {
    let (clean_post_matches, clean_post_sample) = clean_post;
    let (mixed_matches, mixed_sample) = mixed;
    let threshold = threshold.max(0);

    let (verdict, rationale) = if boundary.fix_started_running.is_none() {
        (
            FixVerdict::FixedUnverified,
            "no epoch has been observed running a binary at least as new as the fix, \
             so there is no post-fix window to look in"
                .to_string(),
        )
    } else if clean_post_matches > 0 {
        (
            FixVerdict::StillLive,
            format!(
                "the symptom occurred {clean_post_matches} time(s) after the clean-post boundary"
            ),
        )
    } else if clean_post_sample >= threshold {
        (
            FixVerdict::FixedVerified,
            format!(
                "no occurrence in {clean_post_sample} clean-post observations \
                 (threshold {threshold})"
            ),
        )
    } else {
        (
            FixVerdict::InsufficientPostFixData,
            format!(
                "no occurrence, but only {clean_post_sample} clean-post observation(s) \
                 against a threshold of {threshold}; the {mixed_sample} observation(s) in the \
                 MIXED window are served by both binaries and cannot count as post-fix"
            ),
        )
    };

    FixAssessment {
        verdict,
        boundary,
        clean_post_matches,
        clean_post_sample,
        mixed_matches,
        mixed_sample,
        threshold,
        rationale,
    }
}

/// Capture the epoch of the *currently running* executable.
///
/// `binary_mtime` is read from the file on disk; when the executable has been
/// replaced or unlinked under us (`cargo install`, a release drop) the mtime
/// describes the *new* file, so `exe_deleted` is recorded alongside it and the
/// pair is read together — a stale flag with a fresh mtime is the signature of
/// exactly the mixed-binary window this table exists to detect.
pub fn current_daemon_epoch(started_at: &str) -> HistoryEpoch {
    let identity = crate::mcp::socket::ExeIdentity::current();
    let binary_path = identity
        .as_ref()
        .map(|i| i.path().to_string_lossy().to_string());
    let binary_mtime = identity.as_ref().and_then(|i| {
        std::fs::metadata(i.path())
            .and_then(|m| m.modified())
            .ok()
            .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
    });

    HistoryEpoch {
        id: 0,
        epoch_kind: EPOCH_KIND_DAEMON_START.to_string(),
        binary_path,
        binary_mtime,
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        started_at: started_at.to_string(),
        // Seeded to the start instant so a daemon that dies before its first
        // heartbeat still carries an honest "observed alive at" stamp.
        ended_at: Some(started_at.to_string()),
        pid: Some(std::process::id() as i64),
        exe_deleted: identity.map(|i| i.is_stale()).unwrap_or(false),
        recorded_at: started_at.to_string(),
    }
}

/// How the fix's build time was determined, carried into the answer so a
/// reader can see whether the boundary rests on a commit or on a bare stamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixAnchor {
    /// Resolved from an indexed commit's `committed_at`.
    Commit { sha: String, committed_at: String },
    /// Supplied directly by the caller.
    Timestamp { at: String },
}

/// A verdict request as the surfaces receive it.
#[derive(Debug, Clone)]
pub struct VerdictRequest {
    /// Substring matched against `events.event_type` and `events.summary`.
    pub symptom: String,
    /// Commit that carries the fix (full SHA or prefix), or `None` when
    /// `fix_at` is given.
    pub fix_commit: Option<String>,
    /// The fix's build/commit time, RFC3339. Not the start of post-fix data —
    /// see [`boundary_for`].
    pub fix_at: Option<String>,
    pub threshold: i64,
}

/// A complete answer, ready to render as prose or JSON.
#[derive(Debug, Clone, Serialize)]
pub struct VerdictResponse {
    pub symptom: String,
    pub anchor: FixAnchor,
    pub verdict: String,
    pub assessment: FixAssessment,
    /// Epoch rows available to the classifier at all. Zero means the timeline
    /// has never been recorded — reported so an empty answer is not read as a
    /// quiet one.
    pub epochs_recorded: usize,
}

/// Answer "is symptom X fixed" against the running-binary timeline.
///
/// The single production path for the verdict, in the same discipline as
/// [`crate::history::search::run`].
pub fn run(
    cas_root: &std::path::Path,
    req: &VerdictRequest,
) -> anyhow::Result<VerdictResponse> {
    use anyhow::{Context, anyhow};
    use cas_store::{HistoryStore, SqliteHistoryStore};

    let store = SqliteHistoryStore::open(cas_root).context("open history store")?;

    let anchor = match (&req.fix_commit, &req.fix_at) {
        (Some(sha), _) => {
            let hit = store
                .commit_hit_by_sha(sha)?
                .ok_or_else(|| anyhow!("commit {sha} is not in the history index (run `cas history backfill`)"))?;
            FixAnchor::Commit {
                sha: hit.commit.sha.clone(),
                committed_at: hit.commit.committed_at.clone(),
            }
        }
        (None, Some(at)) => FixAnchor::Timestamp {
            at: crate::history::search::parse_time_bound(at)?,
        },
        (None, None) => {
            return Err(anyhow!("one of --fix-commit or --fix-at is required"));
        }
    };

    let built_at_raw = match &anchor {
        FixAnchor::Commit { committed_at, .. } => committed_at.clone(),
        FixAnchor::Timestamp { at } => at.clone(),
    };
    let built_at = parse(&built_at_raw)
        .ok_or_else(|| anyhow!("unparseable fix timestamp: {built_at_raw}"))?;

    // The epoch corpus is small (one row per daemon start), so it is read whole
    // and classified in Rust — string comparison of RFC3339 in SQL is exactly
    // the kind of shortcut that mis-orders a `Z` against a `+00:00`.
    let epochs = store.list_epochs(None, usize::MAX)?;
    let boundary = boundary_for(&epochs, built_at);

    let symptom = (!req.symptom.trim().is_empty()).then(|| req.symptom.clone());
    let (clean_post, mixed) = match (boundary.fix_started_running, boundary.clean_post_from) {
        (Some(fix), Some(clean)) => {
            let post = store.observation_counts(
                &clean.to_rfc3339(),
                None,
                symptom.as_deref(),
            )?;
            let mixed = store.observation_counts(
                &fix.to_rfc3339(),
                Some(&clean.to_rfc3339()),
                symptom.as_deref(),
            )?;
            (
                (post.matches, post.sample),
                (mixed.matches, mixed.sample),
            )
        }
        _ => ((0, 0), (0, 0)),
    };

    let assessment = assess(boundary, clean_post, mixed, req.threshold);
    Ok(VerdictResponse {
        symptom: req.symptom.clone(),
        anchor,
        verdict: assessment.verdict.as_str().to_string(),
        assessment,
        epochs_recorded: epochs.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch(started: &str, ended: Option<&str>, mtime: Option<&str>, pid: i64) -> HistoryEpoch {
        HistoryEpoch {
            id: 0,
            epoch_kind: EPOCH_KIND_DAEMON_START.to_string(),
            binary_path: Some("/usr/local/bin/cas".into()),
            binary_mtime: mtime.map(str::to_string),
            version: Some("2.49.0".into()),
            started_at: started.to_string(),
            ended_at: ended.map(str::to_string),
            pid: Some(pid),
            exe_deleted: false,
            recorded_at: started.to_string(),
        }
    }

    fn at(ts: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&Utc)
    }

    /// The cas-9d92 shape, replayed: the fix installs at 21:02:26Z, a pre-install
    /// daemon keeps heartbeating until 21:36:35Z. Nothing before 21:36:35Z is
    /// post-fix.
    #[test]
    fn mixed_window_runs_to_the_last_old_heartbeat() {
        let epochs = vec![
            // Old daemon, started before the fix build, alive well into the window.
            epoch(
                "2026-08-07T19:00:00Z",
                Some("2026-08-07T21:36:35Z"),
                Some("2026-08-06T00:00:00Z"),
                111,
            ),
            // The fixed binary, first observed running at 21:02:26Z.
            epoch(
                "2026-08-07T21:02:26Z",
                Some("2026-08-07T23:00:00Z"),
                Some("2026-08-07T20:55:00Z"),
                222,
            ),
        ];
        let b = boundary_for(&epochs, at("2026-08-07T20:50:00Z"));

        assert_eq!(b.fix_started_running, Some(at("2026-08-07T21:02:26Z")));
        assert_eq!(b.clean_post_from, Some(at("2026-08-07T21:36:35Z")));
        assert_eq!(b.overlapping_older_epochs, 1);

        assert_eq!(
            b.classify(at("2026-08-07T20:00:00Z")),
            Some(EpochClass::CleanPre)
        );
        assert_eq!(
            b.classify(at("2026-08-07T21:10:00Z")),
            Some(EpochClass::Mixed),
            "the retraction's window must not read as post-fix"
        );
        assert_eq!(
            b.classify(at("2026-08-07T21:36:35Z")),
            Some(EpochClass::CleanPost),
            "the boundary instant itself is clean-post"
        );
    }

    /// AC(3): an absent symptom with a thin post-fix window must never come
    /// back as "fixed".
    #[test]
    fn thin_clean_post_window_is_insufficient_not_fixed() {
        let epochs = vec![epoch(
            "2026-08-07T21:02:26Z",
            Some("2026-08-07T21:47:00Z"),
            Some("2026-08-07T20:55:00Z"),
            222,
        )];
        let b = boundary_for(&epochs, at("2026-08-07T20:50:00Z"));
        // 17 rows / ~45 min — cas-9d92's own stated limit.
        let a = assess(b, (0, 17), (4, 40), DEFAULT_SAMPLE_THRESHOLD);

        assert_eq!(a.verdict, FixVerdict::InsufficientPostFixData);
        assert!(!a.verdict.is_affirmative());
        assert_eq!(a.clean_post_sample, 17);
        assert!(a.rationale.contains("17"));
    }

    /// AC(3) again, from the other side: with no post-fix epoch at all the
    /// answer is FIXED-UNVERIFIED, never FIXED-VERIFIED, no matter how quiet
    /// the log has been.
    #[test]
    fn no_post_fix_epoch_is_unverified_even_with_a_silent_log() {
        let epochs = vec![epoch(
            "2026-08-01T10:00:00Z",
            Some("2026-08-07T23:00:00Z"),
            Some("2026-07-30T00:00:00Z"),
            111,
        )];
        let b = boundary_for(&epochs, at("2026-08-07T20:50:00Z"));
        assert!(b.fix_started_running.is_none());
        assert!(b.classify(at("2026-08-08T00:00:00Z")).is_none());

        let a = assess(b, (0, 100_000), (0, 0), DEFAULT_SAMPLE_THRESHOLD);
        assert_eq!(a.verdict, FixVerdict::FixedUnverified);
    }

    #[test]
    fn an_occurrence_after_the_boundary_is_still_live() {
        let epochs = vec![epoch(
            "2026-08-07T21:02:26Z",
            Some("2026-08-08T03:00:00Z"),
            Some("2026-08-07T20:55:00Z"),
            222,
        )];
        let b = boundary_for(&epochs, at("2026-08-07T20:50:00Z"));
        let a = assess(b, (3, 4_000), (10, 40), DEFAULT_SAMPLE_THRESHOLD);
        assert_eq!(a.verdict, FixVerdict::StillLive);
    }

    #[test]
    fn a_large_quiet_clean_post_window_verifies() {
        let epochs = vec![epoch(
            "2026-08-07T21:02:26Z",
            Some("2026-08-08T03:00:00Z"),
            Some("2026-08-07T20:55:00Z"),
            222,
        )];
        let b = boundary_for(&epochs, at("2026-08-07T20:50:00Z"));
        let a = assess(b, (0, 4_000), (2, 40), DEFAULT_SAMPLE_THRESHOLD);
        assert_eq!(a.verdict, FixVerdict::FixedVerified);
        assert!(a.verdict.is_affirmative());
    }

    /// A backfilled epoch carries no binary identity. It must never be the
    /// thing that opens a CLEAN-POST window — that would let "we do not know
    /// what was running" masquerade as "the fix was running".
    #[test]
    fn unknown_binary_epochs_never_open_a_clean_post_window() {
        let epochs = vec![
            epoch("2026-08-07T22:00:00Z", Some("2026-08-07T23:00:00Z"), None, 333),
            epoch("2026-08-07T22:30:00Z", Some("2026-08-07T23:30:00Z"), None, 444),
        ];
        let b = boundary_for(&epochs, at("2026-08-07T20:50:00Z"));
        assert!(b.fix_started_running.is_none());
        assert_eq!(b.epochs_without_binary_identity, 2);
        assert_eq!(
            assess(b, (0, 9_999), (0, 0), DEFAULT_SAMPLE_THRESHOLD).verdict,
            FixVerdict::FixedUnverified
        );
    }

    /// …but an unknown-binary daemon still alive into the window *does* widen
    /// MIXED. Unknown costs evidence; it never manufactures it.
    #[test]
    fn unknown_binary_epochs_still_widen_the_mixed_window() {
        let epochs = vec![
            epoch("2026-08-07T19:00:00Z", Some("2026-08-07T22:15:00Z"), None, 111),
            epoch(
                "2026-08-07T21:02:26Z",
                Some("2026-08-08T00:00:00Z"),
                Some("2026-08-07T20:55:00Z"),
                222,
            ),
        ];
        let b = boundary_for(&epochs, at("2026-08-07T20:50:00Z"));
        assert_eq!(b.clean_post_from, Some(at("2026-08-07T22:15:00Z")));
        assert_eq!(b.overlapping_older_epochs, 1);
        assert_eq!(
            b.classify(at("2026-08-07T22:00:00Z")),
            Some(EpochClass::Mixed)
        );
    }

    /// An epoch with no observed liveness after its start contributes its start
    /// instant only — not an unbounded tail that would make everything MIXED
    /// forever.
    #[test]
    fn an_epoch_without_an_end_does_not_extend_the_window_indefinitely() {
        let epochs = vec![
            epoch("2026-08-07T19:00:00Z", None, Some("2026-08-06T00:00:00Z"), 111),
            epoch(
                "2026-08-07T21:02:26Z",
                Some("2026-08-08T00:00:00Z"),
                Some("2026-08-07T20:55:00Z"),
                222,
            ),
        ];
        let b = boundary_for(&epochs, at("2026-08-07T20:50:00Z"));
        assert_eq!(
            b.clean_post_from,
            Some(at("2026-08-07T21:02:26Z")),
            "a daemon last seen before the fix cannot make the window MIXED"
        );
        assert_eq!(b.overlapping_older_epochs, 0);
    }

    #[test]
    fn non_daemon_epoch_kinds_are_ignored_by_the_boundary() {
        let mut install = epoch(
            "2026-08-07T21:02:26Z",
            None,
            Some("2026-08-07T20:55:00Z"),
            0,
        );
        install.epoch_kind = cas_store::EPOCH_KIND_BINARY_INSTALL.to_string();
        let b = boundary_for(&[install], at("2026-08-07T20:50:00Z"));
        assert!(b.fix_started_running.is_none());
        assert_eq!(b.epochs_considered, 0);
    }
}
