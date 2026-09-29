//! Factory Server Integration Tests
//!
//! Tests for the WebSocket client-server communication layer in CAS Factory.
//! These tests verify the integration between:
//! - FactoryServer (WebSocket server accepting client connections)
//! - Factory protocol messages (ClientMessage, ServerMessage)
//! - State synchronization (FullState, DirectorUpdate, etc.)
//!
//! # Prerequisites
//!
//! Some tests are marked with #[ignore] pending implementation.
//! See cas-4d96 for the task to implement these tests.
//!
//! # Running
//!
//! ```bash
//! # Run all factory server tests
//! cargo test --test integration_factory factory_server_test::
//!
//! # Run including ignored tests (after cas-4d96 implementation)
//! cargo test --test integration_factory factory_server_test:: -- --include-ignored
//! ```

use std::net::SocketAddr;
use std::time::Duration;

use cas_factory_protocol::{
    ClientCapabilities, ClientMessage, ClientType, ErrorCode, PROTOCOL_VERSION, RowData,
    ServerMessage, SessionMode, SessionState, StyleRun, codec,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, connect_async};

// =============================================================================
// Test Fixtures
// =============================================================================

/// Mock Factory server for testing client behavior.
///
/// Listens on a random port and handles protocol messages.
struct MockFactoryServer {
    addr: SocketAddr,
    shutdown_tx: mpsc::Sender<()>,
}

impl MockFactoryServer {
    /// Start a mock server that responds with basic protocol messages.
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        if let Ok((stream, _)) = accept_result {
                            let ws_stream = accept_async(stream).await.unwrap();
                            let (mut write, mut read) = ws_stream.split();

                            // Handle messages
                            while let Some(Ok(msg)) = read.next().await {
                                if let Message::Binary(bytes) = msg {
                                    if let Ok(client_msg) = codec::decode::<ClientMessage>(&bytes) {
                                        let response = Self::handle_message(client_msg);
                                        if let Some(resp) = response {
                                            let resp_bytes = codec::encode(&resp).unwrap();
                                            let _ = write.send(Message::Binary(resp_bytes)).await;
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
    fn handle_message(msg: ClientMessage) -> Option<ServerMessage> {
        match msg {
            ClientMessage::Connect {
                protocol_version, ..
            } => {
                if protocol_version == PROTOCOL_VERSION {
                    Some(ServerMessage::Connected {
                        session_id: "test-session".to_string(),
                        client_id: 1,
                        mode: SessionMode::Live,
                    })
                } else {
                    Some(ServerMessage::Error {
                        code: ErrorCode::VersionMismatch,
                        message: format!("Expected {PROTOCOL_VERSION}, got {protocol_version}"),
                    })
                }
            }
            ClientMessage::Ping { id } => Some(ServerMessage::Pong { id }),
            ClientMessage::Focus { pane_id } => {
                // Send a FullState update showing the focused pane
                Some(ServerMessage::FullState {
                    state: SessionState {
                        focused_pane: Some(pane_id),
                        panes: vec![],
                        epic_id: None,
                        epic_title: None,
                        cols: 80,
                        rows: 24,
                    },
                })
            }
            ClientMessage::SendInput { pane_id, .. } => {
                // Echo back as PaneRowsUpdate (for testing)
                Some(ServerMessage::PaneRowsUpdate {
                    pane_id,
                    rows: vec![RowData {
                        row: 0,
                        runs: vec![StyleRun::new("[echo]")],
                    }],
                    cursor: None,
                    seq: 1,
                })
            }
            ClientMessage::Reconnect {
                session_id,
                last_seq,
                ..
            } => {
                // Simulate reconnection handling
                // If last_seq is old (< 1000), request full resync
                let resync_needed = last_seq < 1000;
                Some(ServerMessage::ReconnectAccepted {
                    new_client_id: format!("reconnected-{session_id}"),
                    resync_needed,
                })
            }
            ClientMessage::Pong { .. } => None, // Server doesn't respond to Pong
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

/// Helper to connect and complete handshake with a mock server.
async fn connect_and_handshake(
    url: &str,
    client_type: ClientType,
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
        client_type,
        protocol_version: PROTOCOL_VERSION.to_string(),
        auth_token: None,
        session_id: None,
        capabilities: Default::default(),
    };
    let bytes = codec::encode(&connect_msg).map_err(|e| format!("Encode failed: {e}"))?;
    write
        .send(Message::Binary(bytes))
        .await
        .map_err(|e| format!("Send failed: {e}"))?;

    // Receive Connected
    let response = timeout(Duration::from_secs(5), read.next())
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
// Protocol Handshake Tests
// =============================================================================

#[path = "factory_server_test_cases/tests.rs"]
mod tests;
