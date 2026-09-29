use super::*;

#[tokio::test]
async fn test_mock_server_handshake() {
    let server = MockFactoryServer::start().await;

    let result = connect_and_handshake(&server.url(), ClientType::Desktop).await;
    assert!(result.is_ok(), "Handshake should succeed: {result:?}");

    let (_, _, session_id) = result.unwrap();
    assert!(!session_id.is_empty(), "Should receive session ID");

    server.shutdown().await;
}

#[tokio::test]
async fn test_mock_server_ping_pong() {
    let server = MockFactoryServer::start().await;

    let (mut write, mut read, _) = connect_and_handshake(&server.url(), ClientType::Tui)
        .await
        .expect("Handshake failed");

    // Send Ping
    let ping = ClientMessage::Ping { id: 42 };
    let bytes = codec::encode(&ping).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Receive Pong
    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::Pong { id } => {
                assert_eq!(id, 42, "Pong should echo ping ID");
            }
            _ => panic!("Expected Pong, got {msg:?}"),
        }
    } else {
        panic!("Expected binary message");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_mock_server_version_mismatch() {
    let server = MockFactoryServer::start().await;

    let (ws_stream, _) = connect_async(&server.url()).await.expect("Connect failed");
    let (mut write, mut read) = ws_stream.split();

    // Send Connect with wrong version
    let connect_msg = ClientMessage::Connect {
        client_type: ClientType::Web,
        protocol_version: "0.0.0".to_string(),
        auth_token: None,
        session_id: None,
        capabilities: Default::default(),
    };
    let bytes = codec::encode(&connect_msg).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Should receive Error
    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::Error { code, message } => {
                assert_eq!(code, ErrorCode::VersionMismatch);
                assert!(message.contains("0.0.0"));
            }
            _ => panic!("Expected Error, got {msg:?}"),
        }
    } else {
        panic!("Expected binary message");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_mock_server_focus_updates_state() {
    let server = MockFactoryServer::start().await;

    let (mut write, mut read, _) = connect_and_handshake(&server.url(), ClientType::Desktop)
        .await
        .expect("Handshake failed");

    // Send Focus
    let focus = ClientMessage::Focus {
        pane_id: "supervisor".to_string(),
    };
    let bytes = codec::encode(&focus).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Receive FullState
    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::FullState { state } => {
                assert_eq!(
                    state.focused_pane,
                    Some("supervisor".to_string()),
                    "State should reflect focus change"
                );
            }
            _ => panic!("Expected FullState, got {msg:?}"),
        }
    } else {
        panic!("Expected binary message");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_mock_server_input_echo() {
    let server = MockFactoryServer::start().await;

    let (mut write, mut read, _) = connect_and_handshake(&server.url(), ClientType::Tui)
        .await
        .expect("Handshake failed");

    // Send input
    let input = ClientMessage::SendInput {
        pane_id: "worker-1".to_string(),
        data: b"hello".to_vec(),
    };
    let bytes = codec::encode(&input).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Receive PaneRowsUpdate echo
    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::PaneRowsUpdate { pane_id, rows, .. } => {
                assert_eq!(pane_id, "worker-1");
                assert!(!rows.is_empty());
            }
            _ => panic!("Expected PaneRowsUpdate, got {msg:?}"),
        }
    } else {
        panic!("Expected binary message");
    }

    server.shutdown().await;
}

// =============================================================================
// Reconnection Tests
// =============================================================================

#[tokio::test]
async fn test_mock_server_reconnect_no_resync() {
    let server = MockFactoryServer::start().await;

    // First, connect and get session ID
    let (write, read, session_id) = connect_and_handshake(&server.url(), ClientType::Desktop)
        .await
        .expect("Initial handshake failed");

    // Simulate disconnect by dropping the connection
    drop(write);
    drop(read);

    // Reconnect with a recent sequence number (no resync needed)
    let (ws_stream, _) = connect_async(&server.url())
        .await
        .expect("Reconnect failed");
    let (mut write, mut read) = ws_stream.split();

    // Send Reconnect message with recent seq (>= 1000)
    let reconnect_msg = ClientMessage::Reconnect {
        protocol_version: PROTOCOL_VERSION.to_string(),
        auth_token: None,
        client_type: ClientType::Tui,
        capabilities: ClientCapabilities {
            raw_pty_output: false,
            row_snapshots: true,
        },
        session_id: session_id.clone(),
        client_id: "original-client".to_string(),
        last_seq: 1500, // Recent enough, no resync needed
    };
    let bytes = codec::encode(&reconnect_msg).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Receive ReconnectAccepted
    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::ReconnectAccepted {
                new_client_id,
                resync_needed,
            } => {
                assert!(
                    new_client_id.contains(&session_id),
                    "Should include session ID in new client ID"
                );
                assert!(
                    !resync_needed,
                    "Should not need resync with recent sequence"
                );
            }
            _ => panic!("Expected ReconnectAccepted, got {msg:?}"),
        }
    } else {
        panic!("Expected binary message");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_mock_server_reconnect_with_resync() {
    let server = MockFactoryServer::start().await;

    // First, connect and get session ID
    let (write, read, session_id) = connect_and_handshake(&server.url(), ClientType::Tui)
        .await
        .expect("Initial handshake failed");

    // Simulate disconnect
    drop(write);
    drop(read);

    // Reconnect with old sequence number (resync needed)
    let (ws_stream, _) = connect_async(&server.url())
        .await
        .expect("Reconnect failed");
    let (mut write, mut read) = ws_stream.split();

    // Send Reconnect message with old seq (< 1000)
    let reconnect_msg = ClientMessage::Reconnect {
        protocol_version: PROTOCOL_VERSION.to_string(),
        auth_token: None,
        client_type: ClientType::Tui,
        capabilities: ClientCapabilities {
            raw_pty_output: false,
            row_snapshots: true,
        },
        session_id: session_id.clone(),
        client_id: "original-client".to_string(),
        last_seq: 500, // Too old, needs resync
    };
    let bytes = codec::encode(&reconnect_msg).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Receive ReconnectAccepted with resync_needed = true
    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::ReconnectAccepted {
                new_client_id,
                resync_needed,
            } => {
                assert!(
                    new_client_id.contains(&session_id),
                    "Should include session ID in new client ID"
                );
                assert!(resync_needed, "Should need resync with old sequence");
            }
            _ => panic!("Expected ReconnectAccepted, got {msg:?}"),
        }
    } else {
        panic!("Expected binary message");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_mock_server_reconnect_preserves_state() {
    let server = MockFactoryServer::start().await;

    // Connect and set some state
    let (mut write, mut read, session_id) =
        connect_and_handshake(&server.url(), ClientType::Desktop)
            .await
            .expect("Handshake failed");

    // Send Focus to set state
    let focus = ClientMessage::Focus {
        pane_id: "supervisor".to_string(),
    };
    let bytes = codec::encode(&focus).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Consume the FullState response
    let _ = timeout(Duration::from_secs(5), read.next()).await;

    // Simulate disconnect
    drop(write);
    drop(read);

    // Reconnect
    let (ws_stream, _) = connect_async(&server.url())
        .await
        .expect("Reconnect failed");
    let (mut write, mut read) = ws_stream.split();

    // Send Reconnect
    let reconnect_msg = ClientMessage::Reconnect {
        protocol_version: PROTOCOL_VERSION.to_string(),
        auth_token: None,
        client_type: ClientType::Desktop,
        capabilities: ClientCapabilities {
            raw_pty_output: true,
            row_snapshots: false,
        },
        session_id: session_id.clone(),
        client_id: "original-client".to_string(),
        last_seq: 1500,
    };
    let bytes = codec::encode(&reconnect_msg).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Receive ReconnectAccepted
    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        assert!(
            matches!(msg, ServerMessage::ReconnectAccepted { .. }),
            "Expected ReconnectAccepted, got {msg:?}"
        );
    }

    // After reconnect, can continue using the connection (send ping)
    let ping = ClientMessage::Ping { id: 99 };
    let bytes = codec::encode(&ping).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::Pong { id } => {
                assert_eq!(id, 99, "Pong should echo ping ID after reconnect");
            }
            _ => panic!("Expected Pong after reconnect, got {msg:?}"),
        }
    }

    server.shutdown().await;
}

// =============================================================================
// FactoryServer Integration Tests (Scaffolding)
//
// These tests are scaffolding awaiting implementation in cas-4d96.
// The FactoryServer is now available as a standalone component.
// =============================================================================

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_live_mode_startup() {
    // TODO (cas-687e): Test that FactoryServer starts in live mode
    //
    // 1. Create a FactoryServer with live mode config
    // 2. Start the server
    // 3. Connect a client
    // 4. Verify Connected response with mode: Live
    // 5. Verify initial FullState is sent
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_playback_mode_startup() {
    // TODO (cas-687e): Test that FactoryServer starts in playback mode
    //
    // 1. Create a FactoryServer with playback mode config
    // 2. Load a test recording
    // 3. Connect a client
    // 4. Verify Connected response with mode: Playback
    // 5. Verify PlaybackOpened is sent with metadata
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_multiple_clients() {
    // TODO (cas-687e): Test multiple simultaneous client connections
    //
    // 1. Start FactoryServer
    // 2. Connect 3 clients (TUI, Desktop, Web)
    // 3. Verify all receive Connected
    // 4. Send Focus from one client
    // 5. Verify all clients receive FullState update
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_client_disconnect() {
    // TODO (cas-687e): Test graceful client disconnect handling
    //
    // 1. Start FactoryServer
    // 2. Connect client
    // 3. Verify handshake
    // 4. Disconnect client
    // 5. Verify server handles disconnect gracefully
    // 6. Connect new client
    // 7. Verify new client works normally
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_spawn_workers() {
    // TODO (cas-687e): Test SpawnWorkers command
    //
    // 1. Start FactoryServer in live mode
    // 2. Connect client
    // 3. Send SpawnWorkers { count: 2, names: ["test-1", "test-2"] }
    // 4. Verify PaneAdded events for each worker
    // 5. Verify FullState shows new panes
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_shutdown_workers() {
    // TODO (cas-687e): Test ShutdownWorkers command
    //
    // 1. Start FactoryServer with workers
    // 2. Connect client
    // 3. Verify workers exist in FullState
    // 4. Send ShutdownWorkers { count: Some(1) }
    // 5. Verify PaneRemoved event
    // 6. Verify FullState reflects removal
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_terminal_io() {
    // TODO (cas-687e): Test terminal I/O flow
    //
    // 1. Start FactoryServer with PTY
    // 2. Connect client
    // 3. Send terminal input: "\x1b[A" (up arrow)
    // 4. Verify PaneRowsUpdate received
    // 5. Verify rows contain expected content
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_resize() {
    // TODO (cas-687e): Test Resize command
    //
    // 1. Start FactoryServer
    // 2. Connect client
    // 3. Verify initial dimensions in FullState
    // 4. Send Resize { cols: 120, rows: 40 }
    // 5. Verify FullState update with new dimensions
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_inject_prompt() {
    // TODO (cas-687e): Test InjectPrompt command
    //
    // 1. Start FactoryServer with workers
    // 2. Connect client
    // 3. Send InjectPrompt { pane_id: "worker-1", prompt: "Test prompt" }
    // 4. Verify prompt is delivered to worker PTY
    // 5. Optionally verify response in PaneRowsUpdate
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_director_updates() {
    // TODO (cas-687e): Test DirectorUpdate broadcasting
    //
    // 1. Start FactoryServer with CAS integration
    // 2. Connect client
    // 3. Create a task in CAS
    // 4. Verify DirectorUpdate received with task data
    // 5. Update task status
    // 6. Verify new DirectorUpdate received
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_playback_seek() {
    // TODO (cas-687e): Test playback seeking
    //
    // 1. Start FactoryServer in playback mode with test recording
    // 2. Connect client
    // 3. Send PlaybackSeek { timestamp_ms: 5000 }
    // 4. Verify PlaybackSnapshot received with correct timestamp
    // 5. Verify snapshot content matches recording at that time
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_playback_speed() {
    // TODO (cas-687e): Test playback speed control
    //
    // 1. Start FactoryServer in playback mode
    // 2. Connect client
    // 3. Send PlaybackSetSpeed { speed: 2.0 }
    // 4. Measure time between PlaybackSnapshot events
    // 5. Verify events come at 2x normal rate
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_error_handling() {
    // TODO (cas-687e): Test error responses
    //
    // 1. Start FactoryServer
    // 2. Connect client
    // 3. Send Focus { pane_id: "nonexistent" }
    // 4. Verify Error response with code: PaneNotFound
    // 5. Send invalid mode operation
    // 6. Verify Error with code: InvalidMode
    todo!("Implement after cas-687e")
}

#[tokio::test]
#[ignore = "See cas-4d96: Implement FactoryServer integration tests"]
async fn test_factory_server_graceful_shutdown() {
    // TODO (cas-687e): Test server shutdown
    //
    // 1. Start FactoryServer
    // 2. Connect multiple clients
    // 3. Trigger server shutdown
    // 4. Verify all clients receive Close frame
    // 5. Verify server resources are cleaned up
    todo!("Implement after cas-687e")
}

// =============================================================================
// Integration with Real FactoryServer (Post cas-687e)
// =============================================================================

/// Placeholder for FactoryServer config builder
/// Will be replaced with actual type after cas-687e
#[allow(dead_code)]
struct FactoryServerConfig {
    mode: SessionMode,
    port: Option<u16>,
    recording_path: Option<String>,
}

#[allow(dead_code)]
impl FactoryServerConfig {
    fn live() -> Self {
        Self {
            mode: SessionMode::Live,
            port: None,
            recording_path: None,
        }
    }

    fn playback(recording_path: &str) -> Self {
        Self {
            mode: SessionMode::Playback,
            port: None,
            recording_path: Some(recording_path.to_string()),
        }
    }

    fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }
}

/// Placeholder for FactoryServer handle
/// Will be replaced with actual type after cas-687e
#[allow(dead_code)]
struct FactoryServer {
    addr: SocketAddr,
}

#[allow(dead_code)]
impl FactoryServer {
    /// Start a FactoryServer (placeholder for cas-687e)
    async fn start(_config: FactoryServerConfig) -> Result<Self, String> {
        Err("FactoryServer not yet extracted (cas-687e)".to_string())
    }

    fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    async fn shutdown(self) -> Result<(), String> {
        Ok(())
    }
}
