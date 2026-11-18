//! Integration tests for smooth scrolling functionality.
//!
//! These tests validate:
//! - Scrollback buffer access via ghostty_vt FFI functions
//! - Cache row generation for smooth client-side scrolling
//! - Edge cases: resize, large scrollback, fast scrolling

use cas_mux::Pane;

// =============================================================================
// Scrollback Info Tests
// =============================================================================

#[test]
fn test_scrollback_info_initial_state() {
    let pane = Pane::director("test", 24, 80).unwrap();
    let info = pane.scrollback_info();

    // At initial state, should be at bottom with viewport_rows matching terminal size
    assert_eq!(info.viewport_offset, 0, "Should start at bottom");
    assert_eq!(info.viewport_rows, 24, "Should match terminal rows");
    assert!(
        info.total_scrollback >= info.viewport_rows as u32,
        "Total scrollback should be at least viewport rows"
    );
}

#[test]
fn test_scrollback_info_after_content() {
    let mut pane = Pane::director("test", 10, 80).unwrap();

    // Generate content that exceeds viewport (create scrollback)
    for i in 0..50 {
        pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
    }

    let info = pane.scrollback_info();
    assert_eq!(info.viewport_offset, 0, "Should still be at bottom");
    assert_eq!(info.viewport_rows, 10);
    // Total scrollback should be larger now
    assert!(
        info.total_scrollback > 10,
        "Total scrollback should exceed viewport after adding content"
    );
}

#[test]
fn test_scrollback_info_after_scroll() {
    let mut pane = Pane::director("test", 10, 80).unwrap();

    // Generate scrollback content
    for i in 0..100 {
        pane.feed(format!("Line {i:03}\r\n").as_bytes()).unwrap();
    }

    // Scroll up
    pane.scroll(-20).unwrap();

    let info = pane.scrollback_info();
    assert!(
        info.viewport_offset > 0,
        "Viewport offset should be > 0 after scrolling up"
    );
}

// =============================================================================
// Cache Hit/Miss Tests
// =============================================================================

#[test]
fn test_cache_rows_empty_when_no_cache_requested() {
    let mut pane = Pane::director("test", 10, 80).unwrap();

    // Generate some content
    for i in 0..30 {
        pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
    }

    // Request snapshot with cache_window=0
    let (snapshot, cache_rows, cache_start) = pane.create_snapshot_with_cache(0).unwrap();

    assert_eq!(snapshot.rows, 10);
    assert!(
        cache_rows.is_empty(),
        "Should have no cache rows when cache_window=0"
    );
    assert!(
        cache_start.is_none(),
        "Should have no cache_start when cache_window=0"
    );
}


#[test]
fn test_cache_rows_content_matches_screen_position() {
    let mut pane = Pane::director("test", 5, 80).unwrap();
    let input: Vec<_> = (0..50).map(|i| format!("LINE-{i:03}-END")).collect();
    for line in &input {
        pane.feed(format!("{line}\r\n").as_bytes()).unwrap();
    }
    pane.scroll(-20).unwrap();
    let (snapshot, cache_rows, cache_start) = pane.create_snapshot_with_cache(10).unwrap();
    assert_eq!(snapshot.rows, 5);
    assert!(cache_start.is_some());
    assert!(!cache_rows.is_empty());
    for row in &cache_rows {
        assert_eq!(row.text.trim_end(), input[row.screen_row as usize]);
    }
}

// =============================================================================
// Scroll Operations Tests
// =============================================================================

#[test]
fn test_scroll_to_top_and_bottom() {
    let mut pane = Pane::director("test", 10, 80).unwrap();

    // Generate scrollback
    for i in 0..100 {
        pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
    }

    // Scroll to top
    pane.scroll_to_top().unwrap();
    let info_top = pane.scrollback_info();
    assert!(
        info_top.viewport_offset > 0,
        "Should have offset when scrolled to top"
    );

    // Scroll to bottom
    pane.scroll_to_bottom().unwrap();
    let info_bottom = pane.scrollback_info();
    assert_eq!(
        info_bottom.viewport_offset, 0,
        "Should have offset=0 when at bottom"
    );
}

#[test]
fn test_scroll_incremental() {
    let mut pane = Pane::director("test", 10, 80).unwrap();

    // Generate scrollback
    for i in 0..100 {
        pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
    }

    // Start at bottom
    let info1 = pane.scrollback_info();
    assert_eq!(info1.viewport_offset, 0);

    // Scroll up by 5 lines
    pane.scroll(-5).unwrap();
    let info2 = pane.scrollback_info();
    assert!(
        info2.viewport_offset >= 5,
        "Should scroll up by at least 5 lines"
    );

    // Scroll down by 3 lines
    pane.scroll(3).unwrap();
    let info3 = pane.scrollback_info();
    assert!(
        info3.viewport_offset < info2.viewport_offset,
        "Should scroll down (reduce offset)"
    );
}

// =============================================================================
// Performance Tests
// =============================================================================

// =============================================================================
// Edge Cases
// =============================================================================

#[test]
fn test_resize_during_scroll() {
    let mut pane = Pane::director("test", 24, 80).unwrap();

    // Generate content
    for i in 0..100 {
        pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
    }

    // Scroll up
    pane.scroll(-30).unwrap();
    let info_before = pane.scrollback_info();
    assert!(info_before.viewport_offset > 0);

    // Resize terminal
    pane.resize(48, 120).unwrap();

    // Should still be able to get scrollback info and snapshots
    let info_after = pane.scrollback_info();
    assert_eq!(info_after.viewport_rows, 48, "Should reflect new row count");

    // Snapshot should work after resize
    let result = pane.create_snapshot_with_cache(10);
    assert!(result.is_ok(), "Snapshot should succeed after resize");
}

#[test]
fn test_empty_terminal_scrollback() {
    let pane = Pane::director("test", 24, 80).unwrap();

    // Empty terminal should still provide valid scrollback info
    let info = pane.scrollback_info();
    assert_eq!(info.viewport_rows, 24);
    assert_eq!(info.viewport_offset, 0);

    // Snapshot of empty terminal should work
    let result = pane.create_snapshot_with_cache(10);
    assert!(result.is_ok());
}

#[test]
fn test_large_scrollback_buffer() {
    let mut pane = Pane::director("test", 24, 80).unwrap();

    // Generate large scrollback (10000 lines)
    for i in 0..10000 {
        pane.feed(format!("Line {i:05}\r\n").as_bytes()).unwrap();
    }

    let info = pane.scrollback_info();
    println!(
        "Scrollback info: total={}, viewport_rows={}, offset={}",
        info.total_scrollback, info.viewport_rows, info.viewport_offset
    );
    // Ghostty's default scrollback may be limited; just verify we have some scrollback
    assert!(
        info.total_scrollback > info.viewport_rows as u32,
        "Should have more scrollback than viewport rows"
    );

    // Scroll up (amount clamped to available scrollback)
    let scroll_amount = (info.total_scrollback / 2) as i32;
    pane.scroll(-scroll_amount).unwrap();

    // Snapshot should still work
    let result = pane.create_snapshot_with_cache(50);

    assert!(result.is_ok());
}

// =============================================================================
// Mux Integration Tests
// =============================================================================

#[test]
fn test_mux_scroll_with_cache() {
    use cas_mux::Mux;

    let mut mux = Mux::new(24, 80);
    let pane = Pane::director("test-pane", 24, 80).unwrap();
    mux.add_pane(pane);

    // Get mutable pane reference and feed content
    if let Some(pane) = mux.get_mut("test-pane") {
        for i in 0..100 {
            pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
        }
    }

    // Test scroll_pane_with_cache
    let result = mux.scroll_pane_with_cache("test-pane", -30, 20);
    assert!(result.is_ok());

    let (snapshot, cache_rows, cache_start, scroll_offset, scrollback_lines) = result.unwrap();

    // Verify returned data
    assert_eq!(snapshot.rows, 24);
    assert!(
        scroll_offset > 0,
        "Should have scroll offset after scrolling up"
    );
    assert!(scrollback_lines > 24, "Should have scrollback lines");

    // With cache_window=20, should have some cache rows
    // (unless we're at the very bottom)
    println!(
        "Cache rows: {}, cache_start: {:?}, offset: {}, total: {}",
        cache_rows.len(),
        cache_start,
        scroll_offset,
        scrollback_lines
    );
}

#[test]
fn test_mux_scroll_with_cache_zero_window() {
    use cas_mux::Mux;

    let mut mux = Mux::new(24, 80);
    let pane = Pane::director("test-pane", 24, 80).unwrap();
    mux.add_pane(pane);

    if let Some(pane) = mux.get_mut("test-pane") {
        for i in 0..50 {
            pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
        }
    }

    // Zero cache window should return empty cache
    let result = mux.scroll_pane_with_cache("test-pane", -10, 0);
    assert!(result.is_ok());

    let (_, cache_rows, cache_start, _, _) = result.unwrap();
    assert!(cache_rows.is_empty());
    assert!(cache_start.is_none());
}

#[test]
fn test_feed_while_scrolled_preserves_viewport() {
    // Regression test: feeding new data while scrolled up must preserve
    // the user's viewport position (not jump to top or bottom).
    let mut pane = Pane::director("test", 10, 80).unwrap();

    // Generate scrollback
    for i in 0..50 {
        pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
    }
    assert_eq!(pane.scrollback_info().viewport_offset, 0);

    // Scroll up 20 lines
    pane.scroll(-20).unwrap();
    let before = pane.scrollback_info();
    assert_eq!(before.viewport_offset, 20);

    // Feed 5 new lines while scrolled up
    pane.feed(b"New1\r\nNew2\r\nNew3\r\nNew4\r\nNew5\r\n")
        .unwrap();
    let after = pane.scrollback_info();

    let new_lines = after.total_scrollback.saturating_sub(before.total_scrollback);
    let expected_offset = before.viewport_offset + new_lines;

    assert_eq!(
        after.viewport_offset, expected_offset,
        "Viewport should stay at old_offset + new_lines (same content visible)"
    );
    assert_eq!(pane.new_lines_below(), new_lines);
}

#[test]
fn test_repeated_feed_while_scrolled_no_drift() {
    // Verify that multiple feed() calls while scrolled don't cause
    // cumulative drift (the bug that sent viewport to the top).
    let mut pane = Pane::director("test", 10, 80).unwrap();

    for i in 0..100 {
        pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
    }

    pane.scroll(-30).unwrap();
    let initial_offset = pane.scrollback_info().viewport_offset;
    let mut total_new_lines = 0u32;

    // Simulate 10 rounds of agent output arriving while user reads earlier content
    for round in 0..10 {
        let before_total = pane.scrollback_info().total_scrollback;
        pane.feed(format!("Agent output round {round}\r\n").as_bytes())
            .unwrap();
        let after = pane.scrollback_info();
        total_new_lines += after.total_scrollback.saturating_sub(before_total);
    }

    let final_offset = pane.scrollback_info().viewport_offset;
    assert_eq!(
        final_offset,
        initial_offset + total_new_lines,
        "After 10 feed rounds, offset should be initial + total_new_lines (no drift)"
    );
    assert_eq!(pane.new_lines_below(), total_new_lines);
}

// =============================================================================
// Alt-screen Tracking Tests (cas-d5fa)
// =============================================================================

/// Entering alt-screen via ESC [ ? 1049 h must be detected.
#[test]
fn test_alt_screen_entry_1049() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    assert!(!pane.is_in_alt_screen(), "should start in normal screen");

    pane.feed(b"\x1b[?1049h").unwrap();
    assert!(pane.is_in_alt_screen(), "1049h should enter alt-screen");
}

/// Exiting alt-screen via ESC [ ? 1049 l must be detected.
#[test]
fn test_alt_screen_exit_1049() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    pane.feed(b"\x1b[?1049h").unwrap();
    assert!(pane.is_in_alt_screen());

    pane.feed(b"\x1b[?1049l").unwrap();
    assert!(!pane.is_in_alt_screen(), "1049l should leave alt-screen");
}

/// Variant mode 47 (older xterm): enter and exit.
#[test]
fn test_alt_screen_mode_47() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    pane.feed(b"\x1b[?47h").unwrap();
    assert!(pane.is_in_alt_screen(), "?47h should enter alt-screen");

    pane.feed(b"\x1b[?47l").unwrap();
    assert!(!pane.is_in_alt_screen(), "?47l should leave alt-screen");
}

/// Variant mode 1047: enter and exit.
#[test]
fn test_alt_screen_mode_1047() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    pane.feed(b"\x1b[?1047h").unwrap();
    assert!(pane.is_in_alt_screen(), "?1047h should enter alt-screen");

    pane.feed(b"\x1b[?1047l").unwrap();
    assert!(!pane.is_in_alt_screen(), "?1047l should leave alt-screen");
}

/// Sequences embedded in normal output are still detected.
#[test]
fn test_alt_screen_detected_in_mixed_output() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    // Some terminal output followed by the alt-screen sequence
    pane.feed(b"Hello, world!\r\n\x1b[?1049hmore output").unwrap();
    assert!(
        pane.is_in_alt_screen(),
        "should detect alt-screen inside mixed output"
    );
}

/// If alt-screen is entered and exited in the same feed chunk, the final
/// state (exited) must win.
#[test]
fn test_alt_screen_enter_then_exit_same_chunk() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    // Enter then immediately exit in one chunk — final state is exited
    pane.feed(b"\x1b[?1049h\x1b[?1049l").unwrap();
    assert!(
        !pane.is_in_alt_screen(),
        "last sequence wins: should be out of alt-screen"
    );
}

/// Scroll on an alt-screen pane silently no-ops at the ghostty layer
/// (`Pane::scroll` returns `Ok(())` but the viewport offset does not move).
/// That's why the UI must forward wheel events to the inner process via the
/// PTY rather than rely on the host scrollback — the host has none to give.
///
/// This test pins both halves of the contract: the call succeeds and the
/// viewport offset is unchanged. Either drifting in isolation is a
/// regression worth catching.
#[test]
fn test_alt_screen_scroll_is_noop() {
    let mut pane = Pane::director("test", 24, 80).unwrap();

    // Fill some scrollback in normal-screen mode first.
    for i in 0..50 {
        pane.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
    }

    // Enter alt-screen.
    pane.feed(b"\x1b[?1049h").unwrap();
    assert!(pane.is_in_alt_screen());

    let info_before = pane.scrollback_info();
    // Attempt to scroll. Ghostty silently no-ops on the alt-screen buffer.
    let result = pane.scroll(-5);
    let info_after = pane.scrollback_info();

    assert!(
        result.is_ok(),
        "Pane::scroll on alt-screen must return Ok(()) (silent no-op), got {result:?}"
    );
    assert_eq!(
        info_before.viewport_offset, info_after.viewport_offset,
        "viewport must not move when scrolling in alt-screen"
    );
}

// =============================================================================
// cas-e0b9: pane exit resets in_alt_screen
// =============================================================================
//
// When the inner process exits while still in alt-screen mode (e.g., a
// `kill -9` on a TUI that never had a chance to emit `\x1b[?1049l`), the
// pane's `in_alt_screen` flag must be cleared on exit. Otherwise the next
// process / a redraw would have wheel events mis-routed (forwarded to the
// PTY as arrow keys instead of driving host scrollback).

#[test]
fn test_pane_exit_resets_alt_screen_cas_e0b9() {
    let mut pane = Pane::director("test", 24, 80).unwrap();

    // Enter alt-screen.
    pane.feed(b"\x1b[?1049h").unwrap();
    assert!(pane.is_in_alt_screen(), "precondition: pane is in alt-screen");

    // Simulate the inner process exiting without sending `\x1b[?1049l`.
    pane.mark_exited(Some(137)); // SIGKILL exit code

    assert!(pane.has_exited(), "precondition: mark_exited took effect");
    assert_eq!(pane.exit_code(), Some(137), "exit_code is preserved");

    assert!(
        !pane.is_in_alt_screen(),
        "in_alt_screen must be cleared on pane exit (cas-e0b9)"
    );
}

/// Mux::focused_is_in_alt_screen reflects the focused pane's alt-screen state.
#[test]
fn test_mux_focused_is_in_alt_screen() {
    use cas_mux::Mux;

    let mut mux = Mux::new(24, 80);
    let pane = Pane::director("test-pane", 24, 80).unwrap();
    mux.add_pane(pane);
    mux.focus("test-pane");

    assert!(!mux.focused_is_in_alt_screen());

    if let Some(p) = mux.get_mut("test-pane") {
        p.feed(b"\x1b[?1049h").unwrap();
    }
    assert!(mux.focused_is_in_alt_screen());

    if let Some(p) = mux.get_mut("test-pane") {
        p.feed(b"\x1b[?1049l").unwrap();
    }
    assert!(!mux.focused_is_in_alt_screen());
}

// =============================================================================
// Split-chunk DEC sequence tests (cas-d5fa P1 #2 — partial_esc carry buffer)
//
// PTY output arrives in arbitrary chunks; an ESC [ ? 1049 h sequence can be
// split across two consecutive feed() calls.  The partial_esc carry buffer
// ensures split sequences are always seen whole.
// =============================================================================

/// Sequence split after the digits: chunk1=`\x1b[?104`, chunk2=`9h`.
/// The partial_esc buffer must carry the first chunk so the combined data
/// `\x1b[?1049h` is scanned on the second feed() call.
#[test]
fn test_alt_screen_split_chunk_1049_digits() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    assert!(!pane.is_in_alt_screen());

    pane.feed(b"\x1b[?104").unwrap();  // incomplete — digits not finished
    assert!(!pane.is_in_alt_screen(), "partial sequence must not set alt-screen");

    pane.feed(b"9h").unwrap();         // completes \x1b[?1049h
    assert!(pane.is_in_alt_screen(), "split sequence must be detected after second chunk");
}

/// Sequence split at the `[`: chunk1 ends with bare `\x1b`, chunk2=`[?1049h`.
#[test]
fn test_alt_screen_split_at_esc() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    pane.feed(b"some output\x1b").unwrap();   // chunk ends on bare ESC
    assert!(!pane.is_in_alt_screen());

    pane.feed(b"[?1049h").unwrap();           // completes the sequence
    assert!(pane.is_in_alt_screen(), "ESC split at chunk boundary must be handled");
}

/// Sequence split at `?`: chunk1=`\x1b[`, chunk2=`?1049h`.
#[test]
fn test_alt_screen_split_at_bracket() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    pane.feed(b"\x1b[").unwrap();
    assert!(!pane.is_in_alt_screen());

    pane.feed(b"?1049h").unwrap();
    assert!(pane.is_in_alt_screen(), "ESC [ split must be carried over");
}

/// Verify exit sequence also works when split: chunk1=`\x1b[?104`, chunk2=`9l`.
#[test]
fn test_alt_screen_split_exit_sequence() {
    let mut pane = Pane::director("test", 24, 80).unwrap();
    // Enter alt-screen in one clean chunk first.
    pane.feed(b"\x1b[?1049h").unwrap();
    assert!(pane.is_in_alt_screen());

    // Exit via a split sequence.
    pane.feed(b"\x1b[?104").unwrap();
    assert!(pane.is_in_alt_screen(), "still in alt-screen mid-sequence");

    pane.feed(b"9l").unwrap();
    assert!(!pane.is_in_alt_screen(), "split exit sequence must be detected");
}
