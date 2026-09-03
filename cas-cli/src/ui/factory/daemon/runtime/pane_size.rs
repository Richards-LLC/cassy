//! Who owns the PTY geometry of a factory pane (cas-37f8).
//!
//! A factory pane has exactly one PTY but many viewers: the operator's local
//! dashboard (the factory TUI), desktop GUI clients, WebSocket clients — which
//! is how the Commander hub arrives — and cloud relay web viewers.
//!
//! Historically the daemon took the *minimum* size across every viewer so that
//! nobody saw clipped content. That makes any viewer a geometry writer: a phone
//! attaching at a 412px viewport reports ~46 columns and the operator's
//! full-width dashboard pane collapses to 46 columns and stays there.
//!
//! The rule now is: **the local dashboard owns the geometry.** While it is
//! attached, its layout allocation is the pane size and remote viewers are
//! exactly that — viewers, which render the authoritative size (scaled,
//! letterboxed or scrolled) on their own side. Viewer sizes are still recorded
//! so that a headless / remote-only session is still driven by its viewers, and
//! so the smallest viewer takes over the moment the dashboard detaches.

pub(crate) use crate::ui::factory::protocol::PaneSizeAuthority;

/// The size a pane's PTY should have, and who decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneSizeDecision {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) authority: PaneSizeAuthority,
    /// True when at least one viewer asked for a smaller size than the one that
    /// was applied because the local dashboard owns the geometry. This is the
    /// event worth auditing: a remote viewer tried to shrink the operator's
    /// console and was refused.
    pub(crate) refused_viewer_shrink: bool,
}

fn usable(size: (u16, u16)) -> bool {
    size.0 > 0 && size.1 > 0
}

/// Decide a pane's PTY size.
///
/// `local_dashboard` is the size the local factory TUI's layout allocated to
/// this pane, and is `None` when no local dashboard is attached (headless or
/// remote-only session). `viewers` are the sizes reported by every other
/// attached viewer (GUI, WebSocket/hub, relay web).
///
/// Returns `None` when nothing usable is attached, in which case the pane keeps
/// whatever size it already has.
pub(crate) fn decide_pane_size(
    local_dashboard: Option<(u16, u16)>,
    viewers: impl IntoIterator<Item = (u16, u16)>,
) -> Option<PaneSizeDecision> {
    let local = local_dashboard.filter(|&size| usable(size));
    let viewers = viewers.into_iter().filter(|&size| usable(size));

    if let Some((cols, rows)) = local {
        // The dashboard owns the geometry. A viewer that wants less is refused;
        // a viewer that wants the same or more already fits, so there is
        // nothing to refuse and nothing to change.
        let refused_viewer_shrink = viewers.into_iter().any(|(c, r)| c < cols || r < rows);
        return Some(PaneSizeDecision {
            cols,
            rows,
            authority: PaneSizeAuthority::LocalDashboard,
            refused_viewer_shrink,
        });
    }

    let mut min: Option<(u16, u16)> = None;
    for (cols, rows) in viewers {
        min = Some(match min {
            Some((c, r)) => (c.min(cols), r.min(rows)),
            None => (cols, rows),
        });
    }
    min.map(|(cols, rows)| PaneSizeDecision {
        cols,
        rows,
        authority: PaneSizeAuthority::Viewer,
        refused_viewer_shrink: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_viewer_cannot_shrink_the_operators_dashboard_pane() {
        let decision = decide_pane_size(Some((203, 44)), [(46, 33)]).expect("a decision");

        assert_eq!((decision.cols, decision.rows), (203, 44));
        assert_eq!(decision.authority, PaneSizeAuthority::LocalDashboard);
        assert!(decision.refused_viewer_shrink);
    }

    #[test]
    fn remote_only_session_is_driven_by_its_viewers() {
        let decision = decide_pane_size(None, [(120, 40), (46, 33)]).expect("a decision");

        assert_eq!((decision.cols, decision.rows), (46, 33));
        assert_eq!(decision.authority, PaneSizeAuthority::Viewer);
        assert!(!decision.refused_viewer_shrink);
    }

    #[test]
    fn a_viewer_at_or_above_the_local_size_is_not_a_refusal() {
        let decision = decide_pane_size(Some((203, 44)), [(203, 44), (400, 90)]).expect("decision");

        assert_eq!((decision.cols, decision.rows), (203, 44));
        assert!(!decision.refused_viewer_shrink);
    }

    #[test]
    fn a_viewer_narrower_in_only_one_axis_is_still_refused() {
        let wide_but_short = decide_pane_size(Some((203, 44)), [(400, 20)]).expect("a decision");
        assert!(wide_but_short.refused_viewer_shrink);
        assert_eq!((wide_but_short.cols, wide_but_short.rows), (203, 44));

        let narrow_but_tall = decide_pane_size(Some((203, 44)), [(46, 200)]).expect("a decision");
        assert!(narrow_but_tall.refused_viewer_shrink);
    }

    #[test]
    fn the_smallest_viewer_takes_over_when_the_dashboard_detaches() {
        let attached = decide_pane_size(Some((203, 44)), [(46, 33)]).expect("a decision");
        let detached = decide_pane_size(None, [(46, 33)]).expect("a decision");

        assert_eq!((attached.cols, attached.rows), (203, 44));
        assert_eq!((detached.cols, detached.rows), (46, 33));
    }

    #[test]
    fn zero_sized_reports_are_ignored_rather_than_collapsing_the_pane() {
        let local_zero = decide_pane_size(Some((0, 0)), [(120, 40)]).expect("a decision");
        assert_eq!((local_zero.cols, local_zero.rows), (120, 40));
        assert_eq!(local_zero.authority, PaneSizeAuthority::Viewer);

        let viewer_zero = decide_pane_size(None, [(0, 0), (120, 40)]).expect("a decision");
        assert_eq!((viewer_zero.cols, viewer_zero.rows), (120, 40));
    }

    #[test]
    fn nothing_attached_leaves_the_pane_alone() {
        assert!(decide_pane_size(None, []).is_none());
        assert!(decide_pane_size(None, [(0, 10), (10, 0)]).is_none());
    }
}
