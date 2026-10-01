//! cas-06a2: every pane's PTY winsize equals the inner rect the dashboard
//! renders it into, after every layout transition.
//!
//! Live incident: on a 255x60 terminal the supervisor PTY reported
//! `stty size` = 56 177 while its visible pane was 74 columns wide, so Claude
//! laid out for 177 columns and the mux clipped the rest. 177 is the
//! zero-worker supervisor width (70% of 255 minus borders); the daemon had
//! snapshotted it before workers spawned and re-applied it when a viewer
//! attached. These tests drive real `cat` PTYs through each transition and
//! compare the kernel winsize with the rendered geometry.

use super::FactoryApp;
use crate::ui::factory::input::LayoutSizes;
use cas_mux::Pane;
use ratatui::{Terminal, backend::TestBackend};

const COLS: u16 = 255;
const ROWS: u16 = 60;

fn cat_pane(name: &str) -> Option<Pane> {
    Pane::shell(name, std::env::temp_dir(), Some("cat"), 24, 80).ok()
}

/// A test app with a real PTY-backed supervisor pane, or `None` when this
/// environment cannot spawn a PTY (the test then skips rather than flakes).
fn app_with_supervisor() -> Option<FactoryApp> {
    let mut app = FactoryApp::for_test();
    let sup = app.supervisor_name.clone();
    app.mux.add_pane(cat_pane(&sup)?);
    Some(app)
}

/// Mirrors the production spawn path: the pane joins the mux and
/// `worker_names`, then `sync_pane_sizes` relayouts.
fn add_worker(app: &mut FactoryApp, name: &str) {
    app.mux.add_pane(cat_pane(name).expect("worker PTY"));
    app.worker_names.push(name.to_string());
    app.sync_pane_sizes().unwrap();
}

/// Mirrors the production shutdown path: retain + mux removal + relayout.
fn remove_worker(app: &mut FactoryApp, name: &str) {
    app.worker_names.retain(|n| n != name);
    app.mux.remove_pane(name);
    app.sync_pane_sizes().unwrap();
}

/// Render the full dashboard and assert that every rendered pane with a PTY
/// has a kernel winsize equal to its inner content rect, and that the
/// daemon-facing allocation agrees. Returns the supervisor's `(cols, rows)`.
fn assert_ptys_match_render(app: &mut FactoryApp, transition: &str) -> (u16, u16) {
    let mut terminal =
        Terminal::new(TestBackend::new(app.terminal_cols, app.terminal_rows)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let rendered = app.full_pty_content_areas.clone();
    let mut checked = 0;
    for (name, rect) in &rendered {
        let Some(pane) = app.mux.get(name) else {
            continue; // pending worker: in the layout, no PTY yet
        };
        assert_eq!(
            pane.pty_winsize(),
            Some((rect.height, rect.width)),
            "{transition}: PTY winsize of '{name}' must equal its rendered inner rect"
        );
        assert_eq!(
            app.dashboard_pane_size(name),
            Some((rect.width, rect.height)),
            "{transition}: dashboard allocation of '{name}' must equal its rendered inner rect"
        );
        checked += 1;
    }
    assert!(checked > 0, "{transition}: no PTY pane was rendered");

    let sup = &app.supervisor_name;
    let rect = rendered[sup];
    (rect.width, rect.height)
}

#[test]
fn pane_ptys_follow_every_layout_transition() {
    let Some(mut app) = app_with_supervisor() else {
        eprintln!("skipping: PTY spawn unavailable in this environment");
        return;
    };

    app.handle_resize(COLS, ROWS).unwrap();
    // The zero-worker supervisor width is the 177 measured live.
    assert_eq!(assert_ptys_match_render(&mut app, "startup"), (177, 56));

    for name in ["w1", "w2", "w3", "w4"] {
        add_worker(&mut app, name);
        assert_ptys_match_render(&mut app, &format!("worker {name} added"));
    }
    // Four workers on 255 columns: tabbed, supervisor gets the 30% slot. This
    // is the ~74-column pane the operator saw.
    assert_eq!(assert_ptys_match_render(&mut app, "four workers"), (74, 56));

    app.toggle_sidecar_collapsed();
    let collapsed = assert_ptys_match_render(&mut app, "sidecar collapsed");
    assert!(collapsed.0 > 74, "collapsing the sidecar widens the supervisor");
    app.toggle_sidecar_collapsed();
    assert_eq!(
        assert_ptys_match_render(&mut app, "sidecar expanded"),
        (74, 56),
        "expanding the sidecar must shrink the supervisor back"
    );

    app.resize_layout(|sizes| sizes.grow_supervisor(LayoutSizes::LARGE_STEP));
    assert_ptys_match_render(&mut app, "supervisor split grown");
    app.reset_layout();
    assert_eq!(assert_ptys_match_render(&mut app, "split reset"), (74, 56));

    app.add_pending_worker("booting".to_string(), false);
    assert_ptys_match_render(&mut app, "pending worker added");
    app.remove_pending_worker("booting");
    app.sync_pane_sizes().unwrap();
    assert_eq!(
        assert_ptys_match_render(&mut app, "pending worker abandoned"),
        (74, 56)
    );

    for name in ["w4", "w3", "w2"] {
        remove_worker(&mut app, name);
        assert_ptys_match_render(&mut app, &format!("worker {name} removed"));
    }

    app.handle_resize(200, 50).unwrap();
    assert_ptys_match_render(&mut app, "outer terminal shrunk");
    app.handle_resize(COLS, ROWS).unwrap();
    assert_ptys_match_render(&mut app, "outer terminal restored");

    remove_worker(&mut app, "w1");
    assert_eq!(
        assert_ptys_match_render(&mut app, "last worker removed"),
        (177, 56)
    );
}

/// The daemon decides PTY geometry from `dashboard_pane_size`. It must reflect
/// the layout as it is now, not as it was at the last terminal resize: workers
/// spawning after startup changed the layout without a terminal resize, and a
/// viewer attaching later re-applied the stale 177-column snapshot.
#[test]
fn dashboard_allocation_tracks_workers_spawned_after_the_last_terminal_resize() {
    let Some(mut app) = app_with_supervisor() else {
        eprintln!("skipping: PTY spawn unavailable in this environment");
        return;
    };
    app.handle_resize(COLS, ROWS).unwrap();
    let sup = app.supervisor_name.clone();
    assert_eq!(app.dashboard_pane_size(&sup), Some((177, 56)));

    // Layout changes with no terminal resize event at all.
    app.worker_names = vec!["w1".into(), "w2".into(), "w3".into(), "w4".into()];
    assert_eq!(
        app.dashboard_pane_size(&sup),
        Some((74, 56)),
        "the allocation is live layout state, not a resize-time snapshot"
    );
    app.sidecar_collapsed = true;
    assert_ne!(app.dashboard_pane_size(&sup), Some((74, 56)));
    assert_eq!(app.dashboard_pane_size("not-a-pane"), None);
}
