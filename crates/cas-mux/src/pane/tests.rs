#[cfg(test)]
mod cases {
    use crate::pane::{Pane, PaneKind};

    #[test]
    fn test_pane_kind_as_str() {
        assert_eq!(PaneKind::Worker.as_str(), "worker");
        assert_eq!(PaneKind::Supervisor.as_str(), "supervisor");
        assert_eq!(PaneKind::Director.as_str(), "director");
    }

    #[test]
    fn test_director_pane() {
        let pane = Pane::director("test-director", 24, 80).expect("create pane");
        assert_eq!(pane.id(), "test-director");
        assert_eq!(pane.kind(), &PaneKind::Director);
        assert_eq!(pane.title(), "Director");
        assert!(!pane.has_exited());
    }

    #[test]
    fn test_pane_feed_and_dump() {
        let mut pane = Pane::director("test", 24, 80).expect("create pane");
        pane.feed(b"Hello, World!").expect("feed");
        let content = pane.dump_viewport().expect("dump viewport");
        assert!(content.contains("Hello, World!"));
    }

    #[test]
    fn test_pane_ansi_colors() {
        let mut pane = Pane::director("test", 24, 80).expect("create pane");
        pane.feed(b"\x1b[31mRed\x1b[0m Normal").expect("feed");
        let content = pane.dump_viewport().expect("dump viewport");
        assert!(content.contains("Red"));
        assert!(content.contains("Normal"));
    }

    #[test]
    fn test_pane_scroll() {
        let mut pane = Pane::director("test", 5, 80).expect("create pane");

        for i in 0..20 {
            pane.feed(format!("Line {i}\r\n").as_bytes())
                .expect("feed line");
        }

        assert!(pane.scroll(-5).is_ok());
        assert!(pane.scroll(3).is_ok());
        assert!(pane.scroll_to_top().is_ok());
        assert!(pane.scroll_to_bottom().is_ok());
    }

    #[test]
    fn test_scrollback_info() {
        let pane = Pane::director("test", 24, 80).expect("create pane");
        let info = pane.scrollback_info();

        assert_eq!(info.viewport_offset, 0);
        assert_eq!(info.viewport_rows, 24);
        assert!(info.total_scrollback >= info.viewport_rows as u32);
    }

    #[test]
    fn test_get_full_snapshot() {
        let mut pane = Pane::director("test", 5, 10).expect("create pane");
        for i in 0..5 {
            pane.feed(format!("Line {i}\r\n").as_bytes())
                .expect("feed line");
        }

        let snapshot = pane.get_full_snapshot().expect("snapshot");
        assert_eq!(snapshot.rows, 5);
        assert_eq!(snapshot.cols, 10);
        assert_eq!(snapshot.cells.len(), 50);
    }

    #[test]
    fn test_create_snapshot_no_cache() {
        let mut pane = Pane::director("test", 5, 10).expect("create pane");
        for i in 0..5 {
            pane.feed(format!("Row {i}\r\n").as_bytes())
                .expect("feed row");
        }

        let (snapshot, cache_rows, cache_start) = pane
            .create_snapshot_with_cache(0)
            .expect("snapshot with cache");
        assert_eq!(snapshot.rows, 5);
        assert!(cache_rows.is_empty());
        assert!(cache_start.is_none());
    }

    #[test]
    fn test_create_snapshot_with_cache() {
        let mut pane = Pane::director("test", 5, 80).expect("create pane");

        for i in 0..30 {
            pane.feed(format!("Line {i}\r\n").as_bytes())
                .expect("feed line");
        }

        pane.scroll(-10).expect("scroll up");

        let (snapshot, cache_rows, cache_start) = pane
            .create_snapshot_with_cache(20)
            .expect("snapshot with cache");
        assert_eq!(snapshot.rows, 5);
        assert!(cache_start.is_some());

        for row in &cache_rows {
            assert!(row.screen_row < pane.scrollback_lines());
        }
    }

    #[test]
    fn test_strip_literal_cursor_report_echo() {
        let input = b"hello ^[[12;34R world";
        let cleaned = Pane::strip_literal_cursor_reports(input);
        assert_eq!(cleaned.as_ref(), b"hello  world");
    }

    #[test]
    fn test_strip_literal_cursor_report_noop_for_normal_text() {
        let input = b"normal output with [brackets] and numbers 12;34R";
        let cleaned = Pane::strip_literal_cursor_reports(input);
        assert_eq!(cleaned.as_ref(), input);
    }

    // =========================================================================
    // update_alt_screen unit tests — verify the fixed outer-loop guard
    // (was `while i + 4 < data.len()`, now `while i < data.len()` with inner
    //  bounds checks). These test sequences short enough that the old guard
    //  would silently skip them.
    // =========================================================================

    #[test]
    fn update_alt_screen_handles_minimum_length_seq() {
        // Shortest valid sequence: ESC [ ? 4 7 h = 6 bytes (mode 47).
        // With the old guard (i + 4 < len) this was only reached when i == 0
        // AND len > 4, but if the data was exactly 6 bytes the loop ran while
        // 0 + 4 < 6, i.e. for i in 0..1. That still works — but let's confirm.
        let data = b"\x1b[?47h";
        assert!(Pane::update_alt_screen(data, false));
    }

    #[test]
    fn update_alt_screen_detects_1049_entry() {
        let data = b"\x1b[?1049h";
        assert!(Pane::update_alt_screen(data, false));
    }

    #[test]
    fn update_alt_screen_last_sequence_wins() {
        // Enter then exit in the same slice — last (exit) must win.
        let data = b"\x1b[?1049h\x1b[?1049l";
        assert!(!Pane::update_alt_screen(data, false));
    }

    #[test]
    fn update_alt_screen_preserves_current_on_empty_input() {
        assert!(Pane::update_alt_screen(b"", true));
        assert!(!Pane::update_alt_screen(b"", false));
    }

    // ---- memchr fast-path regression coverage (cas-219d) --------------------
    //
    // The scanner has a SIMD `memchr` fast-path that skips bulk non-ESC bytes.
    // These tests pin down the behavioural invariants that the optimisation
    // must preserve: large ESC-free inputs return the current state unchanged
    // (and don't burn CPU), and ESC bytes embedded in bulk text are still
    // matched against the DEC pattern correctly.

    #[test]
    fn update_alt_screen_esc_free_64k_preserves_state() {
        // 64 KiB of ASCII with no 0x1b byte — must return the current state
        // verbatim, regardless of polarity.
        let mut data = Vec::with_capacity(64 * 1024);
        let line = b"the quick brown fox jumps over the lazy dog 0123456789\n";
        while data.len() < 64 * 1024 {
            data.extend_from_slice(line);
        }
        data.truncate(64 * 1024);
        assert!(!data.contains(&0x1b));

        assert!(!Pane::update_alt_screen(&data, false));
        assert!(Pane::update_alt_screen(&data, true));
    }

    #[test]
    fn update_alt_screen_finds_match_after_long_run_of_ascii() {
        // A real DEC 1049 h sequence buried at the end of a 64 KiB ASCII blob.
        // This is the regression scenario for the memchr fast-path: it must
        // still find and act on the embedded sequence rather than bailing
        // because there was no ESC near the start.
        let mut data = vec![b'.'; 64 * 1024];
        data.extend_from_slice(b"\x1b[?1049h");
        assert!(Pane::update_alt_screen(&data, false));

        // Same shape, exiting: a 1049 l after a long run must flip state back.
        let mut data = vec![b'.'; 64 * 1024];
        data.extend_from_slice(b"\x1b[?1049l");
        assert!(!Pane::update_alt_screen(&data, true));
    }

    // ---- cas-e0b9: CSI sub-parameter handling --------------------------------
    //
    // ECMA-48 §5.4.2 allows sub-parameters after a parameter via `;` (param
    // separator) or `:` (sub-parameter separator). xterm emitters routinely
    // produce e.g. `\x1b[?1049;1h`. The scanner consumes any run of
    // `[0-9;:]` after the first parameter and then evaluates `h`/`l` against
    // the leading mode number — so the sub-parameter does not block
    // alt-screen detection.

    #[test]
    fn update_alt_screen_handles_sub_param_with_semicolon_cas_e0b9() {
        assert!(Pane::update_alt_screen(b"\x1b[?1049;1h", false));
        assert!(!Pane::update_alt_screen(b"\x1b[?1049;1l", true));
    }

    #[test]
    fn update_alt_screen_handles_sub_param_with_colon_cas_e0b9() {
        assert!(Pane::update_alt_screen(b"\x1b[?1049:1h", false));
        assert!(!Pane::update_alt_screen(b"\x1b[?1049:1l", true));
    }

    #[test]
    fn update_alt_screen_handles_multi_param_chain_cas_e0b9() {
        // Chain of additional parameters/sub-params — leading mode still wins.
        assert!(Pane::update_alt_screen(b"\x1b[?1049;1;2:3h", false));
        assert!(Pane::update_alt_screen(b"\x1b[?47;0h", false));
    }

    #[test]
    fn update_alt_screen_sub_param_truncated_no_terminator() {
        // Truncated mid-sub-param: scanner must not flip state, must not panic.
        assert!(!Pane::update_alt_screen(b"\x1b[?1049;", false));
        assert!(!Pane::update_alt_screen(b"\x1b[?1049;1", false));
    }

    #[test]
    fn update_alt_screen_unknown_mode_with_sub_param_ignored() {
        // A non-alt-screen mode (e.g. 25 = cursor visibility) with sub-params
        // must not accidentally flip alt-screen state.
        assert!(!Pane::update_alt_screen(b"\x1b[?25;1h", false));
        assert!(Pane::update_alt_screen(b"\x1b[?25;1h", true));
    }

    #[test]
    fn update_alt_screen_sparse_non_dec_esc_ignored() {
        // ESC bytes followed by non-'[' must not be treated as DEC sequences.
        // The fast-path advances past each ESC; correctness is verified by
        // the final state being the unchanged `current` value.
        let mut data = Vec::with_capacity(8 * 1024);
        let chunk = b"normal text\x1bX more text\x1bY tail";
        while data.len() < 8 * 1024 {
            data.extend_from_slice(chunk);
        }
        assert!(!Pane::update_alt_screen(&data, false));
        assert!(Pane::update_alt_screen(&data, true));
    }

    // =========================================================================
    // trailing_dec_partial unit tests — verify carry-buffer detection
    // =========================================================================

    #[test]
    fn trailing_dec_partial_bare_esc() {
        let data = b"hello\x1b";
        let partial = Pane::trailing_dec_partial(data);
        assert_eq!(partial, b"\x1b");
    }

    #[test]
    fn trailing_dec_partial_esc_bracket() {
        let data = b"abc\x1b[";
        let partial = Pane::trailing_dec_partial(data);
        assert_eq!(partial, b"\x1b[");
    }

    #[test]
    fn trailing_dec_partial_esc_bracket_question() {
        let data = b"\x1b[?";
        let partial = Pane::trailing_dec_partial(data);
        assert_eq!(partial, b"\x1b[?");
    }

    #[test]
    fn trailing_dec_partial_esc_bracket_question_digits() {
        let data = b"junk\x1b[?104";
        let partial = Pane::trailing_dec_partial(data);
        assert_eq!(partial, b"\x1b[?104");
    }

    #[test]
    fn trailing_dec_partial_complete_sequence_not_partial() {
        // A complete sequence should NOT be kept — it ends with h/l.
        let data = b"\x1b[?1049h";
        let partial = Pane::trailing_dec_partial(data);
        assert!(partial.is_empty(), "complete sequence must not be carried: {partial:?}");
    }

    #[test]
    fn trailing_dec_partial_empty_input() {
        assert!(Pane::trailing_dec_partial(b"").is_empty());
    }

    #[test]
    fn trailing_dec_partial_no_esc() {
        assert!(Pane::trailing_dec_partial(b"hello world").is_empty());
    }

    /// Test that scrolling an empty pane (no scrollback) returns Ok (no-op, not an error).
    ///
    /// AC #5 (cas-3b18): `RUST_LOG=info` must produce no "Failed to scroll focused pane:"
    /// or "Failed to scroll terminal: code …" log lines during normal scroll operations.
    /// That warning fires only if `Pane::scroll` returns `Err`.  This test pins the
    /// ghostty_vt contract: scrolling a viewport with no scrollback above the visible
    /// region must be a silent no-op (return code 0), not an error.
    #[test]
    fn test_scroll_empty_pane_is_ok_not_error() {
        let mut pane = Pane::director("test-empty", 24, 80).expect("create pane");
        // No content fed — scrollback is empty (just the 24-row visible viewport).
        assert!(
            pane.scroll(-3).is_ok(),
            "scrolling empty pane up should be a silent no-op (Ok), not an error"
        );
        assert!(
            pane.scroll(3).is_ok(),
            "scrolling empty pane down should be a silent no-op (Ok), not an error"
        );
    }

    /// cas-4b99: per-byte KeyStream paste must not mark turn_in_flight; Enter after
    /// terminator must. Pane-local tracker survives split packets across delivers.
    #[test]
    fn deliver_user_input_bracketed_paste_stream_cas_4b99() {
        use crate::pane::UserInputKind;

        let pane = Pane::director("paste-stream", 10, 40).expect("create pane");
        assert!(!pane.is_turn_in_flight());

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // Split start + body with CR/LF + split end — director has no PTY so write
        // fails, but submit marking runs before write.
        let chunks: &[&[u8]] = &[
            b"\x1b[20",
            b"0~line1\nline2\r",
            b"\x1b[201",
            b"~",
        ];
        rt.block_on(async {
            for chunk in chunks {
                let _ = pane
                    .deliver_user_input(chunk, UserInputKind::KeyStream)
                    .await;
            }
        });
        assert!(
            !pane.is_turn_in_flight(),
            "embedded CR/LF inside bracketed paste must not mark submit"
        );

        rt.block_on(async {
            let _ = pane
                .deliver_user_input(b"\r", UserInputKind::KeyStream)
                .await;
        });
        assert!(
            pane.is_turn_in_flight(),
            "Enter after paste terminator must mark submit"
        );

        // StructuredPaste never marks even with embedded CR (independent of tracker).
        pane.clear_turn_in_flight();
        let paste = b"\x1b[200~x\ny\r\x1b[201~";
        rt.block_on(async {
            let _ = pane
                .deliver_user_input(paste, UserInputKind::StructuredPaste)
                .await;
        });
        assert!(!pane.is_turn_in_flight());

        // Pure predicates (fresh tracker).
        assert!(Pane::key_stream_is_submit(b"\r"));
        assert!(!Pane::key_stream_is_submit(paste));
        assert!(Pane::key_stream_is_submit(b"\x1b[200~a\nb\x1b[201~\r"));
    }

    /// cas-1a4d: attached-client input leaves a pane dirty until the operator
    /// submits or explicitly clears the composer. Structured paste is draft
    /// content even though it must not mark a turn in flight.
    #[test]
    fn composer_dirty_tracks_draft_submit_and_clear_cas_1a4d() {
        use crate::pane::UserInputKind;

        let pane = Pane::director("composer-state", 10, 40).expect("create pane");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        assert!(!pane.is_composer_dirty());
        rt.block_on(async {
            let _ = pane
                .deliver_user_input(b"draft", UserInputKind::KeyStream)
                .await;
        });
        assert!(pane.is_composer_dirty(), "printable input creates a draft");

        rt.block_on(async {
            let _ = pane
                .deliver_user_input(b"\x15", UserInputKind::KeyStream)
                .await;
        });
        assert!(!pane.is_composer_dirty(), "Ctrl+U clears the composer");

        rt.block_on(async {
            let _ = pane
                .deliver_user_input(b"pasted text", UserInputKind::StructuredPaste)
                .await;
        });
        assert!(
            pane.is_composer_dirty(),
            "structured paste is unsubmitted composer content"
        );

        rt.block_on(async {
            let _ = pane
                .deliver_user_input(b"\r", UserInputKind::KeyStream)
                .await;
        });
        assert!(
            !pane.is_composer_dirty(),
            "Enter submits and clears the draft"
        );
        assert!(pane.is_turn_in_flight(), "Enter still starts a turn");
    }

    /// cas-0b64: CSI/SS3 cursor navigation is control input, not draft text.
    /// The streaming cases matter because GUI/relay input can split one key
    /// sequence across multiple frames.
    #[test]
    fn composer_navigation_keys_do_not_create_a_draft_cas_0b64() {
        use crate::pane::UserInputKind;

        let pane = Pane::director("composer-navigation", 10, 40).expect("create pane");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            for navigation in [
                b"\x1b[A".as_slice(),
                b"\x1b[B".as_slice(),
                b"\x1b[C".as_slice(),
                b"\x1b[D".as_slice(),
                b"\x1bOA".as_slice(),
            ] {
                let _ = pane
                    .deliver_user_input(navigation, UserInputKind::KeyStream)
                    .await;
                assert!(
                    !pane.is_composer_dirty(),
                    "navigation sequence {navigation:?} must not create a draft"
                );
            }

            let _ = pane
                .deliver_user_input(b"\x1b", UserInputKind::KeyStream)
                .await;
            let _ = pane
                .deliver_user_input(b"[A", UserInputKind::KeyStream)
                .await;
        });
        assert!(
            !pane.is_composer_dirty(),
            "a split arrow-key sequence must remain navigation-only"
        );
    }

    /// Regression (cas-ebc1 final review): the OSC 8 feed-time gate must detect
    /// a hyperlink introducer split across 3+ tiny feed chunks. The old carry
    /// logic kept the tail of the NEW chunk only, dropping earlier carried
    /// bytes ("\x1b" then "]" lost the ESC).
    #[test]
    fn test_hyperlink_gate_detects_introducer_split_across_many_feeds() {
        let mut pane = Pane::director("osc8-split", 4, 40).expect("create pane");
        assert!(!pane.has_hyperlinks());
        for chunk in [b"\x1b".as_ref(), b"]".as_ref(), b"8".as_ref(), b";".as_ref()] {
            pane.feed(chunk).expect("feed chunk");
        }
        assert!(
            pane.has_hyperlinks(),
            "introducer split across 4 single-byte feeds must set has_hyperlinks"
        );

        // Non-link control sequences never set the flag.
        let mut plain = Pane::director("osc8-plain", 4, 40).expect("create pane");
        for chunk in [b"\x1b".as_ref(), b"[".as_ref(), b"1m".as_ref(), b"hi".as_ref()] {
            plain.feed(chunk).expect("feed chunk");
        }
        assert!(!plain.has_hyperlinks());
    }
}
