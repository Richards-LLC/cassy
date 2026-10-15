//! Render scheduler: when the daemon draws a TUI frame (factory loop v2 §5,
//! GH #1210, cas-77f39).
//!
//! The cost of a client frame is paid per frame in the host terminal, not per
//! byte: Konsole spends about 16 ms of its GUI thread on each one, and that
//! thread also handles the operator's keys. Before this scheduler every pass
//! with any PTY output drew a frame, so codex workers writing 38–236 times/s
//! kept a factory at ~30 frames/s and keystrokes echoed late.
//!
//! The scheduler decides per pass whether to draw, from the causes noted in
//! that pass:
//!
//! | Cause | Rule |
//! |---|---|
//! | Input, and echo (output from the pane that took input in the last 250 ms) | drawn now, uncapped |
//! | Layout, focus, notices, refreshes | drawn at most every 16 ms (coalesced) |
//! | Output from the focused pane | at most `render_max_fps` frames/s |
//! | Output from other visible panes | at most `render_unfocused_fps` frames/s |
//! | Output from panes not on screen | never; the pane's terminal state still updates |
//!
//! A drawn frame renders the whole TUI, so it carries every pending cause.
//! The causes it absorbed are counted as coalesced, and the output it carried
//! restarts that output's cap. No damage tracking is added: ratatui already
//! sends only changed cells, and an empty diff sends nothing.
//!
//! Time is passed in, so tests drive it with synthetic instants.

use std::time::{Duration, Instant};

/// How long output from the pane that just took input counts as its echo.
pub(crate) const ECHO_WINDOW: Duration = Duration::from_millis(250);
/// Layout frames are coalesced to at most one per this interval.
pub(crate) const LAYOUT_COALESCE: Duration = Duration::from_millis(16);
/// `[factory] render_max_fps` default.
pub(crate) const DEFAULT_RENDER_MAX_FPS: u32 = 15;
/// `[factory] render_unfocused_fps` default.
pub(crate) const DEFAULT_RENDER_UNFOCUSED_FPS: u32 = 5;

/// How a pane that produced output is shown right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneView {
    Focused,
    Unfocused,
    Hidden,
}

/// Why a frame was drawn: the highest-priority cause pending at that time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FrameCause {
    Input,
    Layout,
    OutputFocused,
    OutputUnfocused,
}

impl FrameCause {
    /// The `loop.json` key for frames of this cause.
    pub(crate) fn name(self) -> &'static str {
        match self {
            FrameCause::Input => "input",
            FrameCause::Layout => "layout",
            FrameCause::OutputFocused => "output_focused",
            FrameCause::OutputUnfocused => "output_unfocused",
        }
    }
}

/// A frame the scheduler wants drawn now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) cause: FrameCause,
    /// Other pending causes this frame also carries.
    pub(crate) coalesced: u32,
}

/// Output frame caps, from `[factory] render_max_fps` and
/// `render_unfocused_fps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderCaps {
    pub(crate) max_fps: u32,
    pub(crate) unfocused_fps: u32,
}

impl Default for RenderCaps {
    fn default() -> Self {
        Self {
            max_fps: DEFAULT_RENDER_MAX_FPS,
            unfocused_fps: DEFAULT_RENDER_UNFOCUSED_FPS,
        }
    }
}

impl RenderCaps {
    pub(crate) fn from_config(config: &crate::config::Config) -> Self {
        let factory = config.factory();
        Self {
            max_fps: factory.render_max_fps,
            unfocused_fps: factory.render_unfocused_fps,
        }
    }

    fn interval(fps: u32) -> Duration {
        Duration::from_secs(1) / fps.max(1)
    }
}

/// Decides, pass by pass, whether to draw a frame.
#[derive(Debug)]
pub(crate) struct RenderScheduler {
    focused_interval: Duration,
    unfocused_interval: Duration,
    input: bool,
    layout: bool,
    focused: bool,
    unfocused: bool,
    last_input: Option<(String, Instant)>,
    last_frame: Option<Instant>,
    last_focused_frame: Option<Instant>,
    last_unfocused_frame: Option<Instant>,
}

impl RenderScheduler {
    pub(crate) fn new(caps: RenderCaps) -> Self {
        Self {
            focused_interval: RenderCaps::interval(caps.max_fps),
            unfocused_interval: RenderCaps::interval(caps.unfocused_fps),
            input: false,
            layout: false,
            focused: false,
            unfocused: false,
            last_input: None,
            last_frame: None,
            last_focused_frame: None,
            last_unfocused_frame: None,
        }
    }

    /// Operator input arrived this pass, for `pane` when it went to one.
    pub(crate) fn note_input(&mut self, pane: Option<&str>, now: Instant) {
        self.input = true;
        if let Some(pane) = pane {
            self.last_input = Some((pane.to_string(), now));
        }
    }

    /// Something other than pane output changed what the TUI shows.
    pub(crate) fn note_layout(&mut self) {
        self.layout = true;
    }

    /// `pane` produced output this pass.
    pub(crate) fn note_output(&mut self, pane: &str, view: PaneView, now: Instant) {
        if view == PaneView::Hidden {
            return;
        }
        let echo = self.last_input.as_ref().is_some_and(|(input_pane, at)| {
            input_pane == pane && now.saturating_duration_since(*at) <= ECHO_WINDOW
        });
        if echo {
            self.input = true;
            return;
        }
        match view {
            PaneView::Focused => self.focused = true,
            PaneView::Unfocused => self.unfocused = true,
            PaneView::Hidden => {}
        }
    }

    /// Whether any cause is waiting for a frame.
    pub(crate) fn pending(&self) -> bool {
        self.input || self.layout || self.focused || self.unfocused
    }

    /// The frame to draw now, if one is due. Taking it clears every pending
    /// cause, since one frame renders them all.
    pub(crate) fn take_frame(&mut self, now: Instant) -> Option<Frame> {
        let due = |last: Option<Instant>, interval: Duration| {
            last.is_none_or(|last| now.saturating_duration_since(last) >= interval)
        };
        let cause = if self.input {
            FrameCause::Input
        } else if self.layout && due(self.last_frame, LAYOUT_COALESCE) {
            FrameCause::Layout
        } else if self.focused && due(self.last_focused_frame, self.focused_interval) {
            FrameCause::OutputFocused
        } else if self.unfocused && due(self.last_unfocused_frame, self.unfocused_interval) {
            FrameCause::OutputUnfocused
        } else {
            return None;
        };
        let pending = [self.input, self.layout, self.focused, self.unfocused]
            .into_iter()
            .filter(|pending| *pending)
            .count() as u32;
        if self.focused {
            self.last_focused_frame = Some(now);
        }
        if self.unfocused {
            self.last_unfocused_frame = Some(now);
        }
        self.last_frame = Some(now);
        self.input = false;
        self.layout = false;
        self.focused = false;
        self.unfocused = false;
        Some(Frame {
            cause,
            coalesced: pending.saturating_sub(1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One simulated second of passes every `pass` with `output` noted on
    /// each, returning the frames drawn.
    fn run_second(
        scheduler: &mut RenderScheduler,
        start: Instant,
        pass: Duration,
        mut each_pass: impl FnMut(&mut RenderScheduler, Instant),
    ) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut now = start;
        while now < start + Duration::from_secs(1) {
            each_pass(scheduler, now);
            frames.extend(scheduler.take_frame(now));
            now += pass;
        }
        frames
    }

    /// A pass every ~4.2 ms is a pane writing 236 times/s (GH #1210's
    /// busiest codex worker).
    const PASS_236_PER_SEC: Duration = Duration::from_micros(4_237);
    /// 200 writes/s (the acceptance criterion's fake panes).
    const PASS_200_PER_SEC: Duration = Duration::from_millis(5);

    #[test]
    fn flv2_output_frames_capped() {
        let caps = RenderCaps::default();
        let mut scheduler = RenderScheduler::new(caps);
        let start = Instant::now();
        let frames = run_second(&mut scheduler, start, PASS_236_PER_SEC, |s, now| {
            s.note_output("focused", PaneView::Focused, now);
        });
        assert!(
            frames.len() as u32 <= caps.max_fps + 1,
            "{} frames/s from one focused pane at 236 writes/s",
            frames.len()
        );
        assert!(
            frames.len() as u32 >= caps.max_fps - 1,
            "{} frames",
            frames.len()
        );
        assert!(frames.iter().all(|f| f.cause == FrameCause::OutputFocused));

        // N fake panes at 200 writes/s each: the focused one and three other
        // visible ones. The focused cap carries the others' output too.
        let mut scheduler = RenderScheduler::new(caps);
        let start = start + Duration::from_secs(10);
        let frames = run_second(&mut scheduler, start, PASS_200_PER_SEC, |s, now| {
            s.note_output("focused", PaneView::Focused, now);
            for pane in ["w1", "w2", "w3"] {
                s.note_output(pane, PaneView::Unfocused, now);
            }
        });
        assert!(
            frames.len() as u32 <= caps.max_fps + 1,
            "{} frames/s from 4 panes at 200 writes/s",
            frames.len()
        );
        assert!(frames.iter().all(|f| f.coalesced >= 1), "{frames:?}");

        // A configured cap is honoured.
        let caps = RenderCaps {
            max_fps: 8,
            unfocused_fps: 2,
        };
        let mut scheduler = RenderScheduler::new(caps);
        let frames = run_second(&mut scheduler, start, PASS_200_PER_SEC, |s, now| {
            s.note_output("focused", PaneView::Focused, now);
        });
        assert!(frames.len() <= 9, "{} frames at max_fps 8", frames.len());
    }

    #[test]
    fn flv2_unfocused_frames_capped() {
        let caps = RenderCaps::default();
        let mut scheduler = RenderScheduler::new(caps);
        let start = Instant::now();
        let frames = run_second(&mut scheduler, start, PASS_236_PER_SEC, |s, now| {
            for pane in ["w1", "w2", "w3"] {
                s.note_output(pane, PaneView::Unfocused, now);
            }
        });
        assert!(
            frames.len() as u32 <= caps.unfocused_fps + 1,
            "{} frames/s from unfocused panes",
            frames.len()
        );
        assert!(!frames.is_empty(), "visible output still repaints");
        assert!(
            frames
                .iter()
                .all(|f| f.cause == FrameCause::OutputUnfocused)
        );
    }

    #[test]
    fn flv2_invisible_pane_output_sends_no_frame() {
        let mut scheduler = RenderScheduler::new(RenderCaps::default());
        let start = Instant::now();
        let frames = run_second(&mut scheduler, start, PASS_236_PER_SEC, |s, now| {
            s.note_output("off-screen", PaneView::Hidden, now);
        });
        assert!(frames.is_empty(), "{frames:?}");
        assert!(!scheduler.pending());

        // Even right after input to it (a pane hidden since): no echo frame.
        scheduler.note_input(Some("off-screen"), start);
        assert!(
            scheduler.take_frame(start).is_some(),
            "the input itself draws"
        );
        scheduler.note_output("off-screen", PaneView::Hidden, start);
        assert_eq!(scheduler.take_frame(start), None);
    }

    #[test]
    fn flv2_input_frame_precedes_output_frame() {
        let mut scheduler = RenderScheduler::new(RenderCaps::default());
        let start = Instant::now();
        // An output frame was just drawn, so the next one is 66 ms away.
        scheduler.note_output("worker", PaneView::Focused, start);
        assert_eq!(
            scheduler.take_frame(start).map(|f| f.cause),
            Some(FrameCause::OutputFocused)
        );
        let now = start + Duration::from_millis(4);
        scheduler.note_output("worker", PaneView::Focused, now);
        assert_eq!(scheduler.take_frame(now), None, "output waits for its slot");

        // A key for the supervisor arrives: its frame is drawn this pass,
        // and the pending output rides along in it.
        scheduler.note_input(Some("supervisor"), now);
        let frame = scheduler
            .take_frame(now)
            .expect("input draws within the pass");
        assert_eq!(frame.cause, FrameCause::Input);
        assert_eq!(frame.coalesced, 1);

        // Its echo is uncapped: every pass with supervisor output inside the
        // echo window draws, while worker output stays capped.
        let mut echo_frames = 0;
        let mut t = now;
        for _ in 0..40 {
            t += Duration::from_millis(5);
            scheduler.note_output("supervisor", PaneView::Focused, t);
            scheduler.note_output("worker", PaneView::Unfocused, t);
            if let Some(frame) = scheduler.take_frame(t) {
                assert_eq!(frame.cause, FrameCause::Input);
                echo_frames += 1;
            }
        }
        assert_eq!(echo_frames, 40, "echo inside 250 ms is never capped");

        // Past the echo window the same output is capped again.
        t += ECHO_WINDOW;
        let frames = run_second(&mut scheduler, t, Duration::from_millis(5), |s, now| {
            s.note_output("supervisor", PaneView::Focused, now);
        });
        assert!(frames.len() <= 16, "{} frames", frames.len());
    }

    /// Frames drawn for a recorded codex pane's PTY reads, replayed as the
    /// daemon loop sees them (a pass every 4 ms), under the old rule (a frame
    /// on every pass with output) and the scheduler. Extra panes replay the
    /// same trace shifted in time. Returns (old frames, new frames).
    fn replay_codex_trace(panes: &[(PaneView, u64)]) -> (usize, usize) {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/codex-pty-reads-gh1210.json")).unwrap();
        let base: Vec<u64> = fixture["reads_ms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_u64().unwrap())
            .collect();
        let span = base.last().unwrap() + 1;
        let mut traces: Vec<(String, PaneView, Vec<u64>, usize)> = panes
            .iter()
            .enumerate()
            .map(|(i, (view, offset))| {
                let mut reads: Vec<u64> = base.iter().map(|t| (t + offset) % span).collect();
                reads.sort_unstable();
                (format!("codex-{i}"), *view, reads, 0)
            })
            .collect();
        let mut scheduler = RenderScheduler::new(RenderCaps::default());
        let start = Instant::now();
        let (mut old, mut new) = (0, 0);
        let mut t = 0;
        while t < span {
            let now = start + Duration::from_millis(t);
            let mut any_output = false;
            for (pane, view, reads, next) in &mut traces {
                let mut output = false;
                while *next < reads.len() && reads[*next] <= t {
                    output = true;
                    *next += 1;
                }
                if output {
                    any_output = true;
                    scheduler.note_output(pane, *view, now);
                }
            }
            old += usize::from(any_output);
            new += usize::from(scheduler.take_frame(now).is_some());
            t += 4;
        }
        (old, new)
    }

    /// GH #1210 acceptance: frames before and after for a real codex pane's
    /// recorded output (codex-cli 0.162.1 streaming a long answer, 61 s).
    /// Three time-shifted copies reproduce the issue's three codex workers:
    /// the old rule gives ~33 frames/s while streaming, as the issue
    /// measured (30.4). The same numbers come from the artifact's replay.
    #[test]
    fn flv2_codex_trace_frames_before_after() {
        use PaneView::{Focused, Hidden, Unfocused};
        assert_eq!(replay_codex_trace(&[(Focused, 0)]), (478, 356));
        assert_eq!(replay_codex_trace(&[(Unfocused, 0)]), (478, 201));
        assert_eq!(replay_codex_trace(&[(Hidden, 0)]), (478, 0));
        let workers = [(Unfocused, 0), (Unfocused, 333), (Unfocused, 777)];
        assert_eq!(replay_codex_trace(&workers), (1384, 226));
        let one_focused = [(Focused, 0), (Unfocused, 333), (Unfocused, 777)];
        assert_eq!(replay_codex_trace(&one_focused), (1384, 428));
    }

    #[test]
    fn layout_changes_coalesce_within_16_ms() {
        let mut scheduler = RenderScheduler::new(RenderCaps::default());
        let start = Instant::now();
        let frames = run_second(&mut scheduler, start, Duration::from_millis(4), |s, _| {
            s.note_layout();
        });
        assert!(
            (60..=63).contains(&frames.len()),
            "{} layout frames/s",
            frames.len()
        );
        assert!(frames.iter().all(|f| f.cause == FrameCause::Layout));
    }
}
