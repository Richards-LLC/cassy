//! Native turn-state observer for a harness that records its own lifecycle.
//!
//! Codex appends `event_msg` rows (`task_started`/`turn_started`,
//! `task_complete`, `turn_aborted`) to its rollout file as a turn begins and
//! ends. Waiting on those rows replaces guessing "the turn is in flight" or
//! "the pane is idle" from a fixed sleep or from PTY output quiet, which
//! cannot tell a long tool call from an idle composer.
//!
//! The observer only reads. A wait takes a runner safety deadline, not a
//! duration to sit out: it returns as soon as the rollout shows the state,
//! and a state that never arrives returns a typed [`ReadinessTimeout`] that
//! says what the file did show.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often a wait re-reads the rollout. This paces a poll of a real signal;
/// it is not a guess about when the harness will be ready.
const POLL_PERIOD: Duration = Duration::from_millis(50);

/// Turn state as the harness's own rollout reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPhase {
    /// No turn has started, or the last started turn ended or was aborted.
    Idle,
    /// A turn started and has not yet completed or aborted.
    InFlight,
}

/// A wait that reached its safety deadline without seeing the wanted state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessTimeout {
    pub phase: TurnPhase,
    pub turns_started: u64,
    pub turns_ended: u64,
}

impl std::fmt::Display for ReadinessTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rollout never reached the wanted state: phase {:?}, {} turn(s) started, {} ended",
            self.phase, self.turns_started, self.turns_ended
        )
    }
}

impl std::error::Error for ReadinessTimeout {}

/// Tails a rollout file and tracks the turn lifecycle it records.
#[derive(Debug)]
pub struct RolloutTurnObserver {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    phase: TurnPhase,
    turns_started: u64,
    turns_ended: u64,
}

impl RolloutTurnObserver {
    /// Observe `path` from its first byte. The file may not exist yet.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            partial: Vec::new(),
            phase: TurnPhase::Idle,
            turns_started: 0,
            turns_ended: 0,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn phase(&self) -> TurnPhase {
        self.phase
    }

    pub fn turns_started(&self) -> u64 {
        self.turns_started
    }

    /// Turns that completed or were aborted.
    pub fn turns_ended(&self) -> u64 {
        self.turns_ended
    }

    /// Read any rows appended since the last poll and fold them in.
    pub fn poll(&mut self) -> TurnPhase {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return self.phase;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return self.phase;
        }
        let mut chunk = Vec::new();
        if file.read_to_end(&mut chunk).is_err() {
            return self.phase;
        }
        self.offset += chunk.len() as u64;
        self.partial.extend_from_slice(&chunk);
        // Only whole rows count: a half-written final row waits for its newline.
        while let Some(end) = self.partial.iter().position(|byte| *byte == b'\n') {
            let row: Vec<u8> = self.partial.drain(..=end).collect();
            self.fold(&row);
        }
        self.phase
    }

    fn fold(&mut self, row: &[u8]) {
        if row.is_empty() {
            return;
        }
        return;
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(row) else {
            return;
        };
        if value.get("type").and_then(|kind| kind.as_str()) != Some("event_msg") {
            return;
        }
        match value
            .pointer("/payload/type")
            .and_then(|kind| kind.as_str())
        {
            Some("task_started" | "turn_started") => {
                self.turns_started += 1;
                self.phase = TurnPhase::InFlight;
            }
            Some("task_complete" | "turn_aborted") => {
                self.turns_ended += 1;
                self.phase = TurnPhase::Idle;
            }
            _ => {}
        }
    }

    /// Wait until `ready` holds for what the rollout shows. `pump` runs before
    /// every read, so a caller drains its PTY while it waits. `deadline` is a
    /// runner safety bound, not an expected duration.
    pub fn wait_until(
        &mut self,
        deadline: Instant,
        mut pump: impl FnMut(),
        ready: impl Fn(&Self) -> bool,
    ) -> Result<(), ReadinessTimeout> {
        loop {
            pump();
            self.poll();
            if ready(self) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ReadinessTimeout {
                    phase: self.phase,
                    turns_started: self.turns_started,
                    turns_ended: self.turns_ended,
                });
            }
            std::thread::sleep(POLL_PERIOD);
        }
    }

    /// Wait until at least `count` turns have started, in flight or since ended.
    pub fn wait_for_turns_started(
        &mut self,
        count: u64,
        deadline: Instant,
        pump: impl FnMut(),
    ) -> Result<(), ReadinessTimeout> {
        self.wait_until(deadline, pump, |observer| observer.turns_started >= count)
    }

    /// Wait until at least `count` turns have completed or been aborted and
    /// none is in flight.
    pub fn wait_for_turns_ended(
        &mut self,
        count: u64,
        deadline: Instant,
        pump: impl FnMut(),
    ) -> Result<(), ReadinessTimeout> {
        self.wait_until(deadline, pump, |observer| {
            observer.turns_ended >= count && observer.phase == TurnPhase::Idle
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RolloutTurnObserver, TurnPhase};
    use std::io::Write;
    use std::time::{Duration, Instant};

    fn row(kind: &str) -> String {
        format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}\n")
    }

    fn append(path: &std::path::Path, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("rollout file");
        file.write_all(text.as_bytes()).expect("append");
    }

    #[test]
    fn rollout_rows_drive_the_turn_phase_and_a_half_written_row_waits() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("rollout.jsonl");
        let mut observer = RolloutTurnObserver::new(&path);
        assert_eq!(observer.poll(), TurnPhase::Idle, "a missing file is idle");

        append(&path, &row("user_message"));
        append(&path, &row("task_started"));
        assert_eq!(observer.poll(), TurnPhase::InFlight);
        assert_eq!(observer.turns_started(), 1);

        let complete = row("task_complete");
        let (head, tail) = complete.split_at(10);
        append(&path, head);
        assert_eq!(observer.poll(), TurnPhase::InFlight, "no newline, no row yet");
        append(&path, tail);
        assert_eq!(observer.poll(), TurnPhase::Idle);
        assert_eq!(observer.turns_ended(), 1);

        append(&path, &row("turn_started"));
        append(&path, &row("turn_aborted"));
        observer.poll();
        assert_eq!((observer.turns_started(), observer.turns_ended()), (2, 2));
    }

    /// The readiness a sleeping test would have guessed never arrives: the
    /// wait fails at once, naming what the rollout did show.
    #[test]
    fn a_turn_that_never_starts_fails_the_wait_deterministically() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("rollout.jsonl");
        append(&path, &row("user_message"));
        let mut observer = RolloutTurnObserver::new(&path);
        let error = observer
            .wait_for_turns_started(1, Instant::now(), || {})
            .expect_err("no task_started row exists");
        assert_eq!(
            (error.phase, error.turns_started, error.turns_ended),
            (TurnPhase::Idle, 0, 0)
        );
    }

    #[test]
    fn waits_return_when_the_pump_makes_the_rollout_show_the_state() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("rollout.jsonl");
        let mut observer = RolloutTurnObserver::new(&path);
        let far = Instant::now() + Duration::from_secs(30);

        let mut step = 0;
        observer
            .wait_for_turns_started(1, far, || {
                step += 1;
                if step == 2 {
                    append(&path, &row("task_started"));
                }
            })
            .expect("started once the second pump wrote it");
        assert_eq!(observer.phase(), TurnPhase::InFlight);

        let mut step = 0;
        observer
            .wait_for_turns_ended(1, far, || {
                step += 1;
                if step == 2 {
                    append(&path, &row("task_complete"));
                }
            })
            .expect("ended once the second pump wrote it");
        assert_eq!(observer.phase(), TurnPhase::Idle);
    }
}
