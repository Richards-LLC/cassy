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
