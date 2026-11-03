#[cfg(test)]
mod cases {
    use crate::PROTOCOL_VERSION;
    use crate::messages::*;

    #[test]
    fn test_client_message_serde() {
        let msg = ClientMessage::Connect {
            client_type: ClientType::Tui,
            protocol_version: "1.0.0".to_string(),
            auth_token: None,
            session_id: Some("test-session".to_string()),
            capabilities: ClientCapabilities::default(),
        };
        // pin: independent protocol fixture catches renamed tags/fields and decode drift.
        let fixture = serde_json::json!({
          "type": "connect",
          "client_type": "tui",
          "protocol_version": "1.0.0",
          "session_id": "test-session"
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), fixture);
        assert_eq!(
            serde_json::from_value::<ClientMessage>(fixture).unwrap(),
            msg
        );
    }

    #[test]
    fn test_client_message_with_auth_token() {
        let msg = ClientMessage::Connect {
            client_type: ClientType::Tui,
            protocol_version: "1.0.0".to_string(),
            auth_token: Some("abc123".to_string()),
            session_id: None,
            capabilities: ClientCapabilities::default(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("auth_token"));
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_server_message_serde() {
        let msg = ServerMessage::Connected {
            session_id: "test-session".to_string(),
            client_id: 1,
            mode: SessionMode::Live,
        };
        // pin: independent protocol fixture catches renamed tags/fields and decode drift.
        let fixture = serde_json::json!({
          "type": "connected",
          "session_id": "test-session",
          "client_id": 1,
          "mode": "live"
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), fixture);
        assert_eq!(
            serde_json::from_value::<ServerMessage>(fixture).unwrap(),
            msg
        );
    }

    #[test]
    fn test_terminal_snapshot_empty() {
        let snapshot = TerminalSnapshot::empty(80, 24);
        assert_eq!(snapshot.cells.len(), 80 * 24);
        assert_eq!(snapshot.cols, 80);
        assert_eq!(snapshot.rows, 24);
    }

    #[test]
    fn test_pane_rows_update_serde() {
        let msg = ServerMessage::PaneRowsUpdate {
            pane_id: "supervisor".to_string(),
            rows: vec![
                RowData {
                    row: 0,
                    runs: vec![
                        StyleRun::new("Hello "),
                        StyleRun::with_style("world", (0, 255, 0), (0, 0, 0), STYLE_BOLD),
                    ],
                },
                RowData {
                    row: 1,
                    runs: vec![StyleRun::new("$ ")],
                },
            ],
            cursor: Some(CursorPosition { x: 2, y: 1 }),
            seq: 42,
        };
        // pin: independent protocol fixture catches renamed tags/fields and decode drift.
        let fixture = serde_json::json!({
          "type": "pane_rows_update",
          "pane_id": "supervisor",
          "rows": [
            {
              "row": 0,
              "runs": [
                {
                  "text": "Hello ",
                  "fg": [
                    204,
                    204,
                    204
                  ],
                  "bg": [
                    0,
                    0,
                    0
                  ],
                  "flags": 0
                },
                {
                  "text": "world",
                  "fg": [
                    0,
                    255,
                    0
                  ],
                  "bg": [
                    0,
                    0,
                    0
                  ],
                  "flags": 1
                }
              ]
            },
            {
              "row": 1,
              "runs": [
                {
                  "text": "$ ",
                  "fg": [
                    204,
                    204,
                    204
                  ],
                  "bg": [
                    0,
                    0,
                    0
                  ],
                  "flags": 0
                }
              ]
            }
          ],
          "cursor": {
            "x": 2,
            "y": 1
          },
          "seq": 42
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), fixture);
        assert_eq!(
            serde_json::from_value::<ServerMessage>(fixture).unwrap(),
            msg
        );
    }

    #[test]
    fn test_style_run_helpers() {
        let run = StyleRun::new("test");
        assert_eq!(run.text, "test");
        assert_eq!(run.fg, (204, 204, 204));
        assert_eq!(run.bg, (0, 0, 0));
        assert_eq!(run.flags, 0);

        let styled =
            StyleRun::with_style("bold", (255, 0, 0), (0, 0, 255), STYLE_BOLD | STYLE_ITALIC);
        assert_eq!(styled.text, "bold");
        assert_eq!(styled.fg, (255, 0, 0));
        assert_eq!(styled.bg, (0, 0, 255));
        assert_eq!(styled.flags, STYLE_BOLD | STYLE_ITALIC);
    }

    #[test]
    fn test_row_data_serde() {
        let row = RowData {
            row: 5,
            runs: vec![
                StyleRun::new("prefix: "),
                StyleRun::with_style("error", (255, 0, 0), (0, 0, 0), 0),
            ],
        };
        // pin: independent protocol fixture catches renamed tags/fields and decode drift.
        let fixture = serde_json::json!({
          "row": 5,
          "runs": [
            {
              "text": "prefix: ",
              "fg": [
                204,
                204,
                204
              ],
              "bg": [
                0,
                0,
                0
              ],
              "flags": 0
            },
            {
              "text": "error",
              "fg": [
                255,
                0,
                0
              ],
              "bg": [
                0,
                0,
                0
              ],
              "flags": 0
            }
          ]
        });
        assert_eq!(serde_json::to_value(&row).unwrap(), fixture);
        assert_eq!(serde_json::from_value::<RowData>(fixture).unwrap(), row);
    }

    #[test]
    fn test_cache_row_serde() {
        let cache_row = CacheRow {
            screen_row: 100,
            text: "Hello, scrollback!".to_string(),
            style_runs: vec![
                StyleRun::new("Hello, "),
                StyleRun::with_style("scrollback", (0, 255, 0), (0, 0, 0), STYLE_BOLD),
                StyleRun::new("!"),
            ],
        };
        // pin: independent protocol fixture catches renamed tags/fields and decode drift.
        let fixture = serde_json::json!({
          "screen_row": 100,
          "text": "Hello, scrollback!",
          "style_runs": [
            {
              "text": "Hello, ",
              "fg": [
                204,
                204,
                204
              ],
              "bg": [
                0,
                0,
                0
              ],
              "flags": 0
            },
            {
              "text": "scrollback",
              "fg": [
                0,
                255,
                0
              ],
              "bg": [
                0,
                0,
                0
              ],
              "flags": 1
            },
            {
              "text": "!",
              "fg": [
                204,
                204,
                204
              ],
              "bg": [
                0,
                0,
                0
              ],
              "flags": 0
            }
          ]
        });
        assert_eq!(serde_json::to_value(&cache_row).unwrap(), fixture);
        assert_eq!(
            serde_json::from_value::<CacheRow>(fixture).unwrap(),
            cache_row
        );
    }

    #[test]
    fn test_pane_snapshot_with_cache_rows() {
        let msg = ServerMessage::PaneSnapshot {
            pane_id: "worker-1".to_string(),
            scroll_offset: 50,
            scrollback_lines: 1000,
            snapshot: TerminalSnapshot::empty(1, 1),
            snapshot_rows: vec![],
            cache_rows: vec![
                CacheRow {
                    screen_row: 26,
                    text: "cached line 1".to_string(),
                    style_runs: vec![StyleRun::new("cached line 1")],
                },
                CacheRow {
                    screen_row: 27,
                    text: "cached line 2".to_string(),
                    style_runs: vec![StyleRun::new("cached line 2")],
                },
            ],
            cache_start_row: Some(26),
            request_id: None,
        };
        // pin: independent protocol fixture catches renamed tags/fields and decode drift.
        let fixture = serde_json::json!({
          "type": "pane_snapshot",
          "pane_id": "worker-1",
          "scroll_offset": 50,
          "scrollback_lines": 1000,
          "snapshot": {
            "cells": [
              {
                "codepoint": 0,
                "fg": [
                  204,
                  204,
                  204
                ],
                "bg": [
                  0,
                  0,
                  0
                ],
                "flags": 0,
                "width": 1
              }
            ],
            "cursor": {
              "x": 0,
              "y": 0
            },
            "cols": 1,
            "rows": 1
          },
          "cache_rows": [
            {
              "screen_row": 26,
              "text": "cached line 1",
              "style_runs": [
                {
                  "text": "cached line 1",
                  "fg": [
                    204,
                    204,
                    204
                  ],
                  "bg": [
                    0,
                    0,
                    0
                  ],
                  "flags": 0
                }
              ]
            },
            {
              "screen_row": 27,
              "text": "cached line 2",
              "style_runs": [
                {
                  "text": "cached line 2",
                  "fg": [
                    204,
                    204,
                    204
                  ],
                  "bg": [
                    0,
                    0,
                    0
                  ],
                  "flags": 0
                }
              ]
            }
          ],
          "cache_start_row": 26
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), fixture);
        assert_eq!(
            serde_json::from_value::<ServerMessage>(fixture).unwrap(),
            msg
        );
    }

    #[test]
    fn test_pane_snapshot_without_cache_rows_backward_compatible() {
        // Test that messages without cache fields (old format) can be decoded
        let json = r#"{"type":"pane_snapshot","pane_id":"test","scroll_offset":0,"scrollback_lines":100,"snapshot":{"cells":[],"cursor":{"x":0,"y":0},"cols":80,"rows":24}}"#;
        let decoded: ServerMessage = serde_json::from_str(json).unwrap();
        match decoded {
            ServerMessage::PaneSnapshot {
                cache_rows,
                cache_start_row,
                snapshot_rows,
                ..
            } => {
                assert!(cache_rows.is_empty());
                assert!(cache_start_row.is_none());
                assert!(snapshot_rows.is_empty());
            }
            _ => panic!("Expected PaneSnapshot"),
        }
    }

    #[test]
    fn test_scroll_with_cache_window() {
        let msg = ClientMessage::Scroll {
            pane_id: "worker-1".to_string(),
            delta: -10,
            cache_window: 50,
            target_offset: None,
            request_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("cache_window"));
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_scroll_without_cache_window_backward_compatible() {
        // Test that messages without cache_window (old format) can be decoded
        let json = r#"{"type":"scroll","pane_id":"test","delta":5}"#;
        let decoded: ClientMessage = serde_json::from_str(json).unwrap();
        match decoded {
            ClientMessage::Scroll { cache_window, .. } => {
                assert_eq!(cache_window, 0);
            }
            _ => panic!("Expected Scroll"),
        }
    }

    #[test]
    fn test_scroll_zero_cache_window_skipped() {
        // Test that cache_window=0 is not serialized (skip_serializing_if)
        let msg = ClientMessage::Scroll {
            pane_id: "worker-1".to_string(),
            delta: 5,
            cache_window: 0,
            target_offset: None,
            request_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("cache_window"));
    }

    #[test]
    fn test_reconnect_message_serde() {
        let msg = ClientMessage::Reconnect {
            protocol_version: PROTOCOL_VERSION.to_string(),
            auth_token: Some("token-abc".to_string()),
            client_type: ClientType::Tui,
            capabilities: ClientCapabilities {
                raw_pty_output: false,
                row_snapshots: true,
            },
            session_id: "session-123".to_string(),
            client_id: "client-456".to_string(),
            last_seq: 42,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("reconnect"));
        assert!(json.contains("protocol_version"));
        assert!(json.contains("client_type"));
        assert!(json.contains("capabilities"));
        assert!(json.contains("session_id"));
        assert!(json.contains("client_id"));
        assert!(json.contains("last_seq"));
        assert!(json.contains("auth_token"));
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_reconnect_accepted_message_serde() {
        let msg = ServerMessage::ReconnectAccepted {
            new_client_id: "new-client-789".to_string(),
            resync_needed: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("reconnect_accepted"));
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);

        // Test with resync needed
        let msg_resync = ServerMessage::ReconnectAccepted {
            new_client_id: "new-client-999".to_string(),
            resync_needed: true,
        };
        let json_resync = serde_json::to_string(&msg_resync).unwrap();
        let decoded_resync: ServerMessage = serde_json::from_str(&json_resync).unwrap();
        assert_eq!(msg_resync, decoded_resync);
    }

    #[test]
    fn test_connection_health_message_serde() {
        let qualities = [
            ConnectionQuality::Excellent,
            ConnectionQuality::Good,
            ConnectionQuality::Fair,
            ConnectionQuality::Poor,
            ConnectionQuality::Degraded,
        ];

        for quality in qualities {
            let msg = ServerMessage::ConnectionHealth {
                rtt_ms: 50,
                quality,
            };
            let json = serde_json::to_string(&msg).unwrap();
            assert!(json.contains("connection_health"));
            assert!(json.contains("rtt_ms"));
            assert!(json.contains("quality"));
            let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_batch_message_serde() {
        let msg = ServerMessage::Batch {
            messages: vec![
                ServerMessage::Pong { id: 1 },
                ServerMessage::ConnectionHealth {
                    rtt_ms: 25,
                    quality: ConnectionQuality::Excellent,
                },
                ServerMessage::PaneRowsUpdate {
                    pane_id: "supervisor".to_string(),
                    rows: vec![RowData {
                        row: 0,
                        runs: vec![StyleRun::new("Hello")],
                    }],
                    cursor: None,
                    seq: 1,
                },
            ],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("batch"));
        assert!(json.contains("messages"));
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_batch_empty() {
        let msg = ServerMessage::Batch { messages: vec![] };
        // pin: independent protocol fixture catches renamed tags/fields and decode drift.
        let fixture = serde_json::json!({
          "type": "batch",
          "messages": []
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), fixture);
        assert_eq!(
            serde_json::from_value::<ServerMessage>(fixture).unwrap(),
            msg
        );
    }

    #[test]
    fn test_connection_quality_serde() {
        // Test each quality level serializes correctly
        assert_eq!(
            serde_json::to_string(&ConnectionQuality::Excellent).unwrap(),
            "\"excellent\""
        );
        assert_eq!(
            serde_json::to_string(&ConnectionQuality::Good).unwrap(),
            "\"good\""
        );
        assert_eq!(
            serde_json::to_string(&ConnectionQuality::Fair).unwrap(),
            "\"fair\""
        );
        assert_eq!(
            serde_json::to_string(&ConnectionQuality::Poor).unwrap(),
            "\"poor\""
        );
        assert_eq!(
            serde_json::to_string(&ConnectionQuality::Degraded).unwrap(),
            "\"degraded\""
        );
    }

    #[test]
    fn test_server_ping_serde() {
        let msg = ServerMessage::Ping { id: 12345 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("ping"));
        assert!(json.contains("12345"));
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_client_pong_serde() {
        let msg = ClientMessage::Pong { id: 54321 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("pong"));
        assert!(json.contains("54321"));
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_boot_progress_serde() {
        let msg = ServerMessage::BootProgress {
            step: "Loading configuration".to_string(),
            step_num: 1,
            total_steps: 5,
            completed: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("boot_progress"));
        assert!(json.contains("Loading configuration"));
        assert!(json.contains("step_num"));
        assert!(json.contains("total_steps"));
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);

        // Test completed step
        let completed = ServerMessage::BootProgress {
            step: "Loading configuration".to_string(),
            step_num: 1,
            total_steps: 5,
            completed: true,
        };
        let json_completed = serde_json::to_string(&completed).unwrap();
        let decoded_completed: ServerMessage = serde_json::from_str(&json_completed).unwrap();
        assert_eq!(completed, decoded_completed);
    }

    #[test]
    fn test_boot_agent_progress_serde() {
        // Test supervisor
        let supervisor = ServerMessage::BootAgentProgress {
            name: "strong-cardinal".to_string(),
            is_supervisor: true,
            progress: 0.5,
            ready: false,
        };
        let json = serde_json::to_string(&supervisor).unwrap();
        assert!(json.contains("boot_agent_progress"));
        assert!(json.contains("strong-cardinal"));
        assert!(json.contains("is_supervisor"));
        assert!(json.contains("progress"));
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(supervisor, decoded);

        // Test worker ready
        let worker_ready = ServerMessage::BootAgentProgress {
            name: "swift-fox".to_string(),
            is_supervisor: false,
            progress: 1.0,
            ready: true,
        };
        let json_ready = serde_json::to_string(&worker_ready).unwrap();
        let decoded_ready: ServerMessage = serde_json::from_str(&json_ready).unwrap();
        assert_eq!(worker_ready, decoded_ready);
    }

    #[test]
    fn test_boot_complete_serde() {
        let msg = ServerMessage::BootComplete;
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("boot_complete"));
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }
}
