//! Factory Server Network Latency Tests
//!
//! Tests that verify system behavior under simulated network latency conditions.
//! These tests ensure:
//! - Protocol messages work correctly with network delays
//! - Reconnection functions properly under latency
//! - Message ordering is preserved
//! - Health monitoring detects degraded connections
//!
//! # Running
//!
//! ```bash
//! cargo test --test integration_factory factory_latency_test::
//! ```

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use cas_factory_protocol::{
    ClientCapabilities, ClientMessage, ClientType, ConnectionQuality, PROTOCOL_VERSION, RowData,
    ServerMessage, SessionMode, StyleRun, codec,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, connect_async};

// =============================================================================
// Latency Simulation Infrastructure
// =============================================================================

/// Configuration for simulating network latency.
#[derive(Clone, Copy)]
struct LatencyConfig {
    /// One-way latency (RTT = 2 * one_way_latency).
    one_way_latency: Duration,
}

impl LatencyConfig {
    fn new(rtt_ms: u64) -> Self {
        Self {
            one_way_latency: Duration::from_millis(rtt_ms / 2),
        }
    }

    /// Simulate one-way network latency.
    async fn apply(&self) {
        sleep(self.one_way_latency).await;
    }
}

/// Mock Factory server with configurable latency simulation.
struct LatencyMockServer {
    addr: SocketAddr,
    shutdown_tx: mpsc::Sender<()>,
}

impl LatencyMockServer {
    /// Start a mock server with simulated latency.
    async fn start(latency: LatencyConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        tokio::spawn(async move {
            let mut seq_counter: u64 = 0;

            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        if let Ok((stream, _)) = accept_result {
                            let ws_stream = match accept_async(stream).await {
                                Ok(ws) => ws,
                                Err(_) => continue,
                            };
                            let (mut write, mut read) = ws_stream.split();
                            let latency = latency;

                            // Handle messages with latency
                            while let Some(Ok(msg)) = read.next().await {
                                if let Message::Binary(bytes) = msg {
                                    // Simulate incoming latency
                                    latency.apply().await;

                                    if let Ok(client_msg) = codec::decode::<ClientMessage>(&bytes) {
                                        let response = Self::handle_message(client_msg, &mut seq_counter);
                                        if let Some(resp) = response {
                                            // Simulate outgoing latency
                                            latency.apply().await;

                                            let resp_bytes = codec::encode(&resp).unwrap();
                                            if write.send(Message::Binary(resp_bytes)).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        break;
                    }
                }
            }
        });

        Self { addr, shutdown_tx }
    }

    /// Handle a client message and return an appropriate response.
    fn handle_message(msg: ClientMessage, seq_counter: &mut u64) -> Option<ServerMessage> {
        match msg {
            ClientMessage::Connect {
                protocol_version, ..
            } => {
                if protocol_version.starts_with(PROTOCOL_VERSION.split('.').next().unwrap_or("1")) {
                    Some(ServerMessage::Connected {
                        session_id: "latency-test-session".to_string(),
                        client_id: 1,
                        mode: SessionMode::Live,
                    })
                } else {
                    Some(ServerMessage::Error {
                        code: cas_factory_protocol::ErrorCode::VersionMismatch,
                        message: "Version mismatch".to_string(),
                    })
                }
            }
            ClientMessage::Ping { id } => Some(ServerMessage::Pong { id }),
            ClientMessage::SendInput { pane_id, data } => {
                *seq_counter += 1;
                // Echo back with sequence number to verify ordering
                Some(ServerMessage::PaneRowsUpdate {
                    pane_id,
                    rows: vec![RowData {
                        row: 0,
                        runs: vec![StyleRun::new(format!(
                            "[seq:{}] {}",
                            seq_counter,
                            String::from_utf8_lossy(&data)
                        ))],
                    }],
                    cursor: None,
                    seq: *seq_counter,
                })
            }
            ClientMessage::Reconnect {
                session_id,
                last_seq,
                ..
            } => {
                let resync_needed = last_seq < 1000;
                Some(ServerMessage::ReconnectAccepted {
                    new_client_id: format!("reconnected-{session_id}"),
                    resync_needed,
                })
            }
            ClientMessage::Pong { .. } => None,
            _ => None,
        }
    }

    fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(()).await;
    }
}

/// Helper to connect and complete handshake with latency mock server.
async fn connect_and_handshake(
    url: &str,
) -> Result<
    (
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        String,
    ),
    String,
> {
    let (ws_stream, _) = connect_async(url)
        .await
        .map_err(|e| format!("Connect failed: {e}"))?;
    let (mut write, mut read) = ws_stream.split();

    // Send Connect
    let connect_msg = ClientMessage::Connect {
        client_type: ClientType::Desktop,
        protocol_version: PROTOCOL_VERSION.to_string(),
        auth_token: None,
        session_id: None,
        capabilities: ClientCapabilities::default(),
    };
    let bytes = codec::encode(&connect_msg).map_err(|e| format!("Encode failed: {e}"))?;
    write
        .send(Message::Binary(bytes))
        .await
        .map_err(|e| format!("Send failed: {e}"))?;

    // Receive Connected (with timeout for latency)
    let response = timeout(Duration::from_secs(10), read.next())
        .await
        .map_err(|_| "Timeout waiting for Connected".to_string())?
        .ok_or_else(|| "Connection closed".to_string())?
        .map_err(|e| format!("Receive failed: {e}"))?;

    match response {
        Message::Binary(bytes) => {
            let msg: ServerMessage =
                codec::decode(&bytes).map_err(|e| format!("Decode failed: {e}"))?;
            match msg {
                ServerMessage::Connected { session_id, .. } => Ok((write, read, session_id)),
                ServerMessage::Error { message, .. } => Err(format!("Server error: {message}")),
                _ => Err("Unexpected response".to_string()),
            }
        }
        _ => Err("Expected binary message".to_string()),
    }
}

// =============================================================================
// Latency Tests
// =============================================================================

#[tokio::test]
async fn test_handshake_with_50ms_rtt() {
    let latency = LatencyConfig::new(50);
    let server = LatencyMockServer::start(latency).await;

    let start = Instant::now();
    let result = connect_and_handshake(&server.url()).await;
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "Handshake should succeed: {result:?}");
    // RTT is 50ms, handshake requires 1 round trip, expect ~50ms
    assert!(
        elapsed >= Duration::from_millis(40),
        "Should observe latency: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "Should complete reasonably: {elapsed:?}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_handshake_with_100ms_rtt() {
    let latency = LatencyConfig::new(100);
    let server = LatencyMockServer::start(latency).await;

    let start = Instant::now();
    let result = connect_and_handshake(&server.url()).await;
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "Handshake should succeed: {result:?}");
    // RTT is 100ms, expect ~100ms
    assert!(
        elapsed >= Duration::from_millis(80),
        "Should observe latency: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(300),
        "Should complete reasonably: {elapsed:?}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_handshake_with_200ms_rtt() {
    let latency = LatencyConfig::new(200);
    let server = LatencyMockServer::start(latency).await;

    let start = Instant::now();
    let result = connect_and_handshake(&server.url()).await;
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "Handshake should succeed: {result:?}");
    // RTT is 200ms, expect ~200ms
    assert!(
        elapsed >= Duration::from_millis(150),
        "Should observe latency: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "Should complete reasonably: {elapsed:?}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_ping_pong_under_latency() {
    let latency = LatencyConfig::new(100);
    let server = LatencyMockServer::start(latency).await;

    let (mut write, mut read, _) = connect_and_handshake(&server.url())
        .await
        .expect("Handshake failed");

    // Measure ping-pong RTT
    let start = Instant::now();
    let ping = ClientMessage::Ping { id: 42 };
    let bytes = codec::encode(&ping).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    let response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    let elapsed = start.elapsed();

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        assert!(
            matches!(msg, ServerMessage::Pong { id: 42 }),
            "Should receive Pong"
        );
    }

    // Should observe RTT of ~100ms
    assert!(
        elapsed >= Duration::from_millis(80),
        "Should observe latency: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(300),
        "Should complete reasonably: {elapsed:?}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_message_ordering_preserved_under_latency() {
    let latency = LatencyConfig::new(50);
    let server = LatencyMockServer::start(latency).await;

    let (mut write, mut read, _) = connect_and_handshake(&server.url())
        .await
        .expect("Handshake failed");

    // Send multiple inputs rapidly
    for i in 1..=5 {
        let input = ClientMessage::SendInput {
            pane_id: "test".to_string(),
            data: format!("msg{i}").into_bytes(),
        };
        let bytes = codec::encode(&input).unwrap();
        write.send(Message::Binary(bytes)).await.unwrap();
    }

    // Receive responses and verify ordering
    let mut received_seqs = Vec::new();
    for _ in 0..5 {
        let response = timeout(Duration::from_secs(10), read.next())
            .await
            .expect("Timeout")
            .expect("Connection closed")
            .expect("Receive error");

        if let Message::Binary(bytes) = response {
            let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
            if let ServerMessage::PaneRowsUpdate { seq, .. } = msg {
                received_seqs.push(seq);
            }
        }
    }

    // Verify messages arrived in order
    assert_eq!(received_seqs.len(), 5, "Should receive all 5 responses");
    for i in 0..4 {
        assert!(
            received_seqs[i] < received_seqs[i + 1],
            "Messages should arrive in order: {received_seqs:?}"
        );
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_reconnection_under_latency() {
    let latency = LatencyConfig::new(100);
    let server = LatencyMockServer::start(latency).await;

    // Initial connection
    let (write, read, session_id) = connect_and_handshake(&server.url())
        .await
        .expect("Initial handshake failed");

    // Simulate disconnect
    drop(write);
    drop(read);

    // Reconnect with latency
    let start = Instant::now();
    let (ws_stream, _) = connect_async(&server.url())
        .await
        .expect("Reconnect failed");
    let (mut write, mut read) = ws_stream.split();

    // Send Reconnect message
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
        last_seq: 1500, // Recent enough, no resync
    };
    let bytes = codec::encode(&reconnect_msg).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    // Receive ReconnectAccepted
    let response = timeout(Duration::from_secs(10), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    let elapsed = start.elapsed();

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::ReconnectAccepted { resync_needed, .. } => {
                assert!(!resync_needed, "Should not need resync with recent seq");
            }
            _ => panic!("Expected ReconnectAccepted, got {msg:?}"),
        }
    }

    // Should observe latency during reconnect
    assert!(
        elapsed >= Duration::from_millis(80),
        "Should observe latency during reconnect: {elapsed:?}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_reconnection_with_resync_under_latency() {
    let latency = LatencyConfig::new(100);
    let server = LatencyMockServer::start(latency).await;

    // Initial connection
    let (write, read, session_id) = connect_and_handshake(&server.url())
        .await
        .expect("Initial handshake failed");

    // Simulate disconnect
    drop(write);
    drop(read);

    // Reconnect with old sequence (needs resync)
    let (ws_stream, _) = connect_async(&server.url())
        .await
        .expect("Reconnect failed");
    let (mut write, mut read) = ws_stream.split();

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
        last_seq: 100, // Too old, needs resync
    };
    let bytes = codec::encode(&reconnect_msg).unwrap();
    write.send(Message::Binary(bytes)).await.unwrap();

    let response = timeout(Duration::from_secs(10), read.next())
        .await
        .expect("Timeout")
        .expect("Connection closed")
        .expect("Receive error");

    if let Message::Binary(bytes) = response {
        let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
        match msg {
            ServerMessage::ReconnectAccepted { resync_needed, .. } => {
                assert!(resync_needed, "Should need resync with old seq");
            }
            _ => panic!("Expected ReconnectAccepted, got {msg:?}"),
        }
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_multiple_operations_under_latency() {
    let latency = LatencyConfig::new(50);
    let server = LatencyMockServer::start(latency).await;

    let (mut write, mut read, _) = connect_and_handshake(&server.url())
        .await
        .expect("Handshake failed");

    let start = Instant::now();

    // Perform multiple operations
    for i in 0..10 {
        // Send input
        let input = ClientMessage::SendInput {
            pane_id: "test".to_string(),
            data: format!("input{i}").into_bytes(),
        };
        let bytes = codec::encode(&input).unwrap();
        write.send(Message::Binary(bytes)).await.unwrap();

        // Wait for response
        let response = timeout(Duration::from_secs(5), read.next())
            .await
            .expect("Timeout")
            .expect("Connection closed")
            .expect("Receive error");

        if let Message::Binary(bytes) = response {
            let msg: ServerMessage = codec::decode(&bytes).expect("Decode failed");
            assert!(
                matches!(msg, ServerMessage::PaneRowsUpdate { .. }),
                "Should receive PaneRowsUpdate"
            );
        }
    }

    let elapsed = start.elapsed();

    // 10 round trips at ~50ms RTT should take ~500ms minimum
    assert!(
        elapsed >= Duration::from_millis(400),
        "Should accumulate latency over operations: {elapsed:?}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_health_monitoring_latency_simulation() {
    // Test that ConnectionHealth message can represent latency
    let health_good = ServerMessage::ConnectionHealth {
        rtt_ms: 20,
        quality: ConnectionQuality::Good,
    };
    let encoded = codec::encode(&health_good).unwrap();
    let decoded: ServerMessage = codec::decode(&encoded).unwrap();
    assert!(
        matches!(
            decoded,
            ServerMessage::ConnectionHealth {
                rtt_ms: 20,
                quality: ConnectionQuality::Good,
            }
        ),
        "Good quality with low RTT"
    );

    let health_degraded = ServerMessage::ConnectionHealth {
        rtt_ms: 150,
        quality: ConnectionQuality::Degraded,
    };
    let encoded = codec::encode(&health_degraded).unwrap();
    let decoded: ServerMessage = codec::decode(&encoded).unwrap();
    assert!(
        matches!(
            decoded,
            ServerMessage::ConnectionHealth {
                rtt_ms: 150,
                quality: ConnectionQuality::Degraded,
            }
        ),
        "Degraded quality with higher RTT"
    );

    let health_poor = ServerMessage::ConnectionHealth {
        rtt_ms: 500,
        quality: ConnectionQuality::Poor,
    };
    let encoded = codec::encode(&health_poor).unwrap();
    let decoded: ServerMessage = codec::decode(&encoded).unwrap();
    assert!(
        matches!(
            decoded,
            ServerMessage::ConnectionHealth {
                rtt_ms: 500,
                quality: ConnectionQuality::Poor,
            }
        ),
        "Poor quality with high RTT"
    );
}
