//! Factory phone-home WebSocket client
//!
//! Connects the factory daemon to Cassy Cloud via Phoenix channels over WebSocket.
//! Pushes factory state and events for remote monitoring.
//!
//! Protocol: Phoenix channel JSON wire format `[join_ref, ref, topic, event, payload]`
//! Socket: `wss://{endpoint}/socket/websocket?token={token}`
//! Topic: `factory:{factory_id}`
//!

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Direct file-based logging for the cloud client.
/// tracing::info! doesn't work in the daemon because set_global_default silently fails.
fn cloud_log(factory_id: &str, msg: &str) {
    use std::io::Write;
    let log_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".cas")
        .join("logs")
        .join("factory")
        .join(factory_id);
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("cloud-client.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let now = chrono::Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(f, "[{now}] {msg}");
    }
}

/// Messages the daemon sends to the cloud client task
#[derive(Debug, Clone)]
pub enum CloudMessage {
    /// Push factory state snapshot
    State(Value),
    /// Push a factory event
    Event { event_type: String, payload: Value },
    /// Upload a recording chunk (JSON event data, base64 encoded)
    RecordingChunk {
        worker_name: String,
        chunk_index: u32,
        data_base64: String,
        started_at: String,
        ended_at: Option<String>,
    },
    /// Graceful shutdown
    Disconnect,
    /// Accept a relay attach request
    RelayAttachAccept {
        client_id: String,
        cols: u16,
        rows: u16,
    },
    /// Send PTY output to a relay client
    RelayPtyOutput { client_id: String, data: Vec<u8> },
    /// Send per-pane PTY output to cloud (for web terminal viewers)
    PaneOutput { pane: String, data: Vec<u8> },
    /// Send available pane list to cloud
    PaneList { panes: Vec<String> },
}

/// Incoming relay event from cloud (pushed to daemon via callback)
#[derive(Debug, Clone)]
pub enum RelayEvent {
    /// Remote user wants to attach
    AttachRequest {
        client_id: String,
        cols: u16,
        rows: u16,
        mode: String,
    },
    /// Remote user sent keyboard input
    PtyInput { client_id: String, data: Vec<u8> },
    /// Remote user resized their terminal
    Resize {
        client_id: String,
        cols: u16,
        rows: u16,
    },
    /// Remote user detached
    Detach { client_id: String },
    /// Web user started watching a pane
    PaneAttach {
        pane: String,
        cols: Option<u16>,
        rows: Option<u16>,
    },
    /// Web user resized their terminal (e.g. maximize/minimize)
    PaneResize { pane: String, cols: u16, rows: u16 },
    /// Web user stopped watching a pane
    PaneDetach { pane: String },
    /// Web user sent input to a specific pane
    PaneInput { pane: String, data: Vec<u8> },
}

/// Configuration for the cloud client
#[derive(Debug, Clone)]
pub struct CloudClientConfig {
    /// Cloud API endpoint (e.g., "https://petra-stella-cloud.vercel.app")
    pub endpoint: String,
    /// API token for authentication
    pub token: String,
    /// Factory ID (supervisor agent ID)
    pub factory_id: String,
    /// Device ID (from device registration)
    pub device_id: Option<String>,
    /// Cassy data directory (for writing to local queues on cloud commands)
    pub cas_dir: Option<std::path::PathBuf>,
    /// Factory session name (for prompt queue isolation)
    pub factory_session: Option<String>,
}

/// Handle for sending messages to the cloud client from the daemon
#[derive(Clone)]
pub struct CloudClientHandle {
    tx: mpsc::UnboundedSender<CloudMessage>,
    /// Receiver for relay events from cloud (daemon polls this)
    relay_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<RelayEvent>>>,
}

impl CloudClientHandle {
    /// Send a state snapshot to the cloud
    pub fn send_state(&self, state: Value) {
        let _ = self.tx.send(CloudMessage::State(state));
    }

    /// Send an event to the cloud
    pub fn send_event(&self, event_type: &str, payload: Value) {
        let _ = self.tx.send(CloudMessage::Event {
            event_type: event_type.to_string(),
            payload,
        });
    }

    /// Send a recording chunk to the cloud
    pub fn send_recording_chunk(
        &self,
        worker_name: &str,
        chunk_index: u32,
        data_base64: String,
        started_at: &str,
        ended_at: Option<&str>,
    ) {
        let _ = self.tx.send(CloudMessage::RecordingChunk {
            worker_name: worker_name.to_string(),
            chunk_index,
            data_base64,
            started_at: started_at.to_string(),
            ended_at: ended_at.map(|s| s.to_string()),
        });
    }

    /// Send disconnect and stop the client
    pub fn disconnect(&self) {
        let _ = self.tx.send(CloudMessage::Disconnect);
    }

    /// Accept a relay attach request
    pub fn relay_accept(&self, client_id: &str, cols: u16, rows: u16) {
        let _ = self.tx.send(CloudMessage::RelayAttachAccept {
            client_id: client_id.to_string(),
            cols,
            rows,
        });
    }

    /// Send PTY output to a relay client
    pub fn relay_output(&self, client_id: &str, data: Vec<u8>) {
        let _ = self.tx.send(CloudMessage::RelayPtyOutput {
            client_id: client_id.to_string(),
            data,
        });
    }

    /// Send per-pane PTY output to cloud
    pub fn send_pane_output(&self, pane: &str, data: Vec<u8>) {
        let _ = self.tx.send(CloudMessage::PaneOutput {
            pane: pane.to_string(),
            data,
        });
    }

    /// Send available pane list to cloud
    pub fn send_pane_list(&self, panes: Vec<String>) {
        let _ = self.tx.send(CloudMessage::PaneList { panes });
    }

    /// Try to receive pending relay events (non-blocking)
    pub fn try_recv_relay(&self) -> Vec<RelayEvent> {
        let mut events = Vec::new();
        if let Ok(mut rx) = self.relay_rx.try_lock() {
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
        }
        events
    }
}

/// Spawn the cloud client background task.
/// Returns a handle for sending messages and receiving relay events.
pub fn spawn_cloud_client(config: CloudClientConfig) -> CloudClientHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let (relay_tx, relay_rx) = mpsc::unbounded_channel();
    cloud_log(
        &config.factory_id,
        &format!(
            "spawn_cloud_client: endpoint={}, factory_id={}",
            config.endpoint, config.factory_id
        ),
    );
    tokio::spawn(cloud_client_task(config, rx, relay_tx));
    CloudClientHandle {
        tx,
        relay_rx: Arc::new(tokio::sync::Mutex::new(relay_rx)),
    }
}

use crate::ui::factory::phoenix::{encode_msg, ws_url};

/// Duration threshold for delta vs full state re-push on reconnect.
/// Within this window, only changed state is sent; beyond it, a full snapshot is pushed.
const RECONNECT_FULL_STATE_THRESHOLD: Duration = Duration::from_secs(300);

/// Maximum consecutive connection failures before giving up.
/// Prevents infinite retry spam (e.g., TLS not compiled in, invalid certs).
const MAX_CONSECUTIVE_FAILURES: u32 = 10;

/// Maximum buffered events when disconnected. Oldest are dropped when exceeded.
const MAX_EVENT_BUFFER: usize = 1000;

/// The main cloud client loop with reconnection
async fn cloud_client_task(
    config: CloudClientConfig,
    mut rx: mpsc::UnboundedReceiver<CloudMessage>,
    relay_tx: mpsc::UnboundedSender<RelayEvent>,
) {
    let mut backoff_secs = 1u64;
    let max_backoff = 60u64;
    let mut event_buffer: Vec<CloudMessage> = Vec::new();
    let mut disconnected_at: Option<Instant> = None;
    let mut consecutive_failures: u32 = 0;

    loop {
        // Determine if this reconnect should push full state
        let needs_full_state = disconnected_at
            .map(|t| t.elapsed() >= RECONNECT_FULL_STATE_THRESHOLD)
            .unwrap_or(true); // First connection always sends full state

        cloud_log(
            &config.factory_id,
            &format!(
                "connect_and_run attempt (backoff={backoff_secs}s, needs_full_state={needs_full_state})"
            ),
        );
        match connect_and_run(
            &config,
            &mut rx,
            &mut event_buffer,
            needs_full_state,
            &relay_tx,
        )
        .await
        {
            Ok(ShouldStop::Yes) => {
                cloud_log(&config.factory_id, "Shutting down gracefully");
                tracing::info!("Cloud client shutting down gracefully");
                return;
            }
            Ok(ShouldStop::Reconnect) => {
                if disconnected_at.is_none() {
                    disconnected_at = Some(Instant::now());
                }
                // Reconnect means we were at least partially connected — reset circuit breaker
                consecutive_failures = 0;
                backoff_secs = 1;
                cloud_log(
                    &config.factory_id,
                    &format!("Connection lost, reconnecting in {backoff_secs}s"),
                );
                tracing::info!("Cloud connection lost, reconnecting in {}s", backoff_secs);
            }
            Err(e) => {
                if disconnected_at.is_none() {
                    disconnected_at = Some(Instant::now());
                }
                consecutive_failures += 1;
                cloud_log(
                    &config.factory_id,
                    &format!(
                        "ERROR: {e}, reconnecting in {backoff_secs}s (attempt {consecutive_failures}/{MAX_CONSECUTIVE_FAILURES})"
                    ),
                );
                // Only log at warn level on the first failure; demote to debug after that
                if consecutive_failures == 1 {
                    tracing::warn!(
                        "Cloud client error: {}, reconnecting in {}s",
                        e,
                        backoff_secs
                    );
                } else {
                    tracing::debug!(
                        "Cloud client error: {}, reconnecting in {}s (attempt {}/{})",
                        e,
                        backoff_secs,
                        consecutive_failures,
                        MAX_CONSECUTIVE_FAILURES
                    );
                }
            }
        }

        // Circuit breaker: give up after too many consecutive failures
        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            cloud_log(
                &config.factory_id,
                &format!("Giving up after {consecutive_failures} consecutive failures"),
            );
            tracing::warn!(
                "Cloud client giving up after {} consecutive failures — phone-home disabled for this session",
                consecutive_failures
            );
            return;
        }

        // Cap event buffer to prevent unbounded memory growth
        if event_buffer.len() > MAX_EVENT_BUFFER {
            let excess = event_buffer.len() - MAX_EVENT_BUFFER;
            event_buffer.drain(..excess);
            cloud_log(
                &config.factory_id,
                &format!("Dropped {excess} oldest buffered events (cap={MAX_EVENT_BUFFER})"),
            );
        }

        // Exponential backoff with jitter
        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(max_backoff);
    }
}

enum ShouldStop {
    Yes,
    Reconnect,
}

/// Single connection lifecycle: connect → join → push messages → handle disconnect
///
/// `request_full_state`: when true, sends a `factory.request_full_state` event after join
/// to signal the cloud that it should expect a full snapshot (reconnect after >5 min).
/// When false (reconnect within 5 min), the daemon relies on buffered events for delta sync.
async fn connect_and_run(
    config: &CloudClientConfig,
    rx: &mut mpsc::UnboundedReceiver<CloudMessage>,
    event_buffer: &mut Vec<CloudMessage>,
    request_full_state: bool,
    relay_tx: &mpsc::UnboundedSender<RelayEvent>,
) -> anyhow::Result<ShouldStop> {
    let url = ws_url(&config.endpoint, &config.token);

    cloud_log(
        &config.factory_id,
        &format!(
            "Connecting to WebSocket: {}...",
            &url[..url.find("token=").unwrap_or(50).min(50)]
        ),
    );

    // Connect WebSocket
    let (ws_stream, _response) = tokio_tungstenite::connect_async(&url).await?;
    let (mut write, mut read) = ws_stream.split();

    cloud_log(&config.factory_id, "WebSocket connected");
    tracing::info!("Connected to cloud WebSocket");

    let topic = format!("factory:{}", config.factory_id);
    let join_ref = "1";

    // Join the factory channel
    let join_payload = serde_json::json!({
        "agent_id": config.factory_id,
        "role": "daemon",
        "device_id": config.device_id,
    });
    let join_msg = encode_msg(Some(join_ref), &topic, "phx_join", &join_payload);
    write.send(Message::Text(join_msg)).await?;

    // Wait for join reply
    let join_timeout = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(join_timeout);

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(&text) {
                            if arr.len() >= 5 {
                                let event = arr[3].as_str().unwrap_or("");
                                if event == "phx_reply" {
                                    let status = arr[4].get("status")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("");
                                    if status == "ok" {
                                        cloud_log(&config.factory_id, &format!("Joined channel: {topic}"));
                                        tracing::info!("Joined cloud channel: {}", topic);
                                        break;
                                    } else {
                                        anyhow::bail!("Channel join rejected: {:?}", arr[4]);
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Ok(ShouldStop::Reconnect);
                    }
                    _ => {}
                }
            }
            _ = &mut join_timeout => {
                anyhow::bail!("Channel join timed out");
            }
        }
    }

    // Flush buffered events from previous disconnection
    for msg in event_buffer.drain(..) {
        if let Err(e) = send_cloud_message(&mut write, &topic, join_ref, &msg).await {
            tracing::warn!("Failed to flush buffered message: {}", e);
        }
    }

    // If reconnecting after a long disconnection (>5 min), signal the cloud to expect
    // a full state snapshot. For quick reconnects (<5 min), the buffered events above
    // provide delta sync. The daemon's main loop will push a fresh state on next tick.
    if request_full_state {
        let payload = serde_json::json!({"reason": "reconnect_full"});
        let msg = encode_msg(
            Some(join_ref),
            &topic,
            "factory.request_full_state",
            &payload,
        );
        let _ = write.send(Message::Text(msg)).await;
        cloud_log(&config.factory_id, "Sent factory.request_full_state");
        tracing::info!("Requested full state re-push after long disconnection");
    }

    cloud_log(&config.factory_id, "Entering main message loop");

    // Main message loop
    let heartbeat_interval = Duration::from_secs(30);
    let mut last_send = Instant::now();
    let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
    heartbeat_timer.tick().await; // Skip first immediate tick

    // Debounce: collect state updates within a 1-second window
    let mut pending_state: Option<Value> = None;
    let mut debounce_deadline: Option<tokio::time::Instant> = None;

    loop {
        let debounce_sleep = match debounce_deadline {
            Some(deadline) => tokio::time::sleep_until(deadline),
            None => tokio::time::sleep(Duration::from_secs(3600)), // effectively infinite
        };
        tokio::pin!(debounce_sleep);

        tokio::select! {
            // Incoming messages from daemon
            msg = rx.recv() => {
                let msg_desc = match &msg {
                    Some(CloudMessage::State(_)) => "State".to_string(),
                    Some(CloudMessage::Event { event_type, .. }) => format!("Event({event_type})"),
                    Some(CloudMessage::PaneOutput { pane, data }) => format!("PaneOutput(pane={}, {} bytes)", pane, data.len()),
                    Some(CloudMessage::PaneList { panes }) => format!("PaneList({panes:?})"),
                    Some(CloudMessage::Disconnect) => "Disconnect".to_string(),
                    Some(CloudMessage::RecordingChunk { .. }) => "RecordingChunk".to_string(),
                    Some(CloudMessage::RelayAttachAccept { .. }) => "RelayAttachAccept".to_string(),
                    Some(CloudMessage::RelayPtyOutput { .. }) => "RelayPtyOutput".to_string(),
                    None => "Channel closed".to_string(),
                };
                cloud_log(&config.factory_id, &format!("MPSC recv: {msg_desc}"));
                match msg {
                    Some(CloudMessage::Disconnect) => {
                        // Send disconnect event
                        let payload = serde_json::json!({"reason": "shutdown"});
                        let disconnect_msg = encode_msg(Some(join_ref), &topic, "factory.disconnect", &payload);
                        let _ = write.send(Message::Text(disconnect_msg)).await;

                        // Leave channel
                        let leave_msg = encode_msg(Some(join_ref), &topic, "phx_leave", &serde_json::json!({}));
                        let _ = write.send(Message::Text(leave_msg)).await;

                        let _ = write.send(Message::Close(None)).await;
                        return Ok(ShouldStop::Yes);
                    }
                    Some(CloudMessage::State(state)) => {
                        // Debounce: replace pending state, set deadline
                        pending_state = Some(state);
                        debounce_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(1));
                    }
                    Some(msg @ CloudMessage::Event { .. })
                    | Some(msg @ CloudMessage::RecordingChunk { .. }) => {
                        // Events and recordings are sent immediately (not debounced)
                        if let Err(e) = send_cloud_message(&mut write, &topic, join_ref, &msg).await {
                            tracing::warn!("Failed to send event: {}", e);
                            event_buffer.push(msg);
                            return Ok(ShouldStop::Reconnect);
                        }
                        last_send = Instant::now();
                    }
                    // Relay/pane messages are sent immediately (low-latency terminal I/O)
                    Some(msg @ CloudMessage::RelayAttachAccept { .. })
                    | Some(msg @ CloudMessage::RelayPtyOutput { .. })
                    | Some(msg @ CloudMessage::PaneOutput { .. })
                    | Some(msg @ CloudMessage::PaneList { .. }) => {
                        if let Err(e) = send_cloud_message(&mut write, &topic, join_ref, &msg).await {
                            tracing::warn!("Failed to send relay message: {}", e);
                            return Ok(ShouldStop::Reconnect);
                        }
                        last_send = Instant::now();
                    }
                    None => {
                        // Channel closed (daemon shutting down)
                        return Ok(ShouldStop::Yes);
                    }
                }
            }

            // Debounce timer fired — flush pending state
            _ = &mut debounce_sleep, if pending_state.is_some() => {
                if let Some(state) = pending_state.take() {
                    debounce_deadline = None;
                    let msg = CloudMessage::State(state);
                    if let Err(e) = send_cloud_message(&mut write, &topic, join_ref, &msg).await {
                        tracing::warn!("Failed to send state: {}", e);
                        event_buffer.push(msg);
                        return Ok(ShouldStop::Reconnect);
                    }
                    last_send = Instant::now();
                }
            }

            // Phoenix heartbeat (every 30s if no other messages)
            _ = heartbeat_timer.tick() => {
                if last_send.elapsed() >= Duration::from_secs(25) {
                    let hb_msg = encode_msg(Some(join_ref), "phoenix", "heartbeat", &serde_json::json!({}));
                    if let Err(e) = write.send(Message::Text(hb_msg)).await {
                        tracing::warn!("Heartbeat failed: {}", e);
                        return Ok(ShouldStop::Reconnect);
                    }
                    last_send = Instant::now();
                }
            }

            // Incoming WebSocket messages from cloud
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Log the event name from incoming messages
                        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(&text) {
                            if arr.len() >= 5 {
                                let event = arr[3].as_str().unwrap_or("?");
                                cloud_log(&config.factory_id, &format!("WS recv: {event}"));
                            }
                        }
                        handle_cloud_message(&text, relay_tx, config.cas_dir.as_deref(), config.factory_session.as_deref());
                    }
                    Some(Ok(Message::Close(frame))) => {
                        cloud_log(&config.factory_id, &format!("WebSocket closed: {frame:?}"));
                        tracing::info!("Cloud WebSocket closed");
                        return Ok(ShouldStop::Reconnect);
                    }
                    None => {
                        cloud_log(&config.factory_id, "WebSocket stream ended (None)");
                        tracing::info!("Cloud WebSocket closed");
                        return Ok(ShouldStop::Reconnect);
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Send a CloudMessage over the WebSocket
async fn send_cloud_message<S>(
    write: &mut S,
    topic: &str,
    join_ref: &str,
    msg: &CloudMessage,
) -> anyhow::Result<()>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let jr = Some(join_ref);
    let encoded = match msg {
        CloudMessage::State(state) => encode_msg(jr, topic, "factory.state", state),
        CloudMessage::Event {
            event_type,
            payload,
        } => {
            let wrapped = serde_json::json!({
                "event_type": event_type,
                "payload": payload,
            });
            encode_msg(jr, topic, "factory.event", &wrapped)
        }
        CloudMessage::RecordingChunk {
            worker_name,
            chunk_index,
            data_base64,
            started_at,
            ended_at,
        } => {
            let payload = serde_json::json!({
                "worker_name": worker_name,
                "chunk_index": chunk_index,
                "data": data_base64,
                "started_at": started_at,
                "ended_at": ended_at,
            });
            encode_msg(jr, topic, "factory.recording_chunk", &payload)
        }
        CloudMessage::Disconnect => encode_msg(
            jr,
            topic,
            "factory.disconnect",
            &serde_json::json!({"reason": "shutdown"}),
        ),
        CloudMessage::RelayAttachAccept {
            client_id,
            cols,
            rows,
        } => encode_msg(
            jr,
            topic,
            "relay.attach_accept",
            &serde_json::json!({
                "client_id": client_id,
                "cols": cols,
                "rows": rows,
            }),
        ),
        CloudMessage::RelayPtyOutput { client_id, data } => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(data);
            encode_msg(
                jr,
                topic,
                "relay.pty_output",
                &serde_json::json!({
                    "client_id": client_id,
                    "data": b64,
                }),
            )
        }
        CloudMessage::PaneOutput { pane, data } => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(data);
            encode_msg(
                jr,
                topic,
                "pane.output",
                &serde_json::json!({
                    "pane": pane,
                    "data": b64,
                }),
            )
        }
        CloudMessage::PaneList { panes } => encode_msg(
            jr,
            topic,
            "pane.list",
            &serde_json::json!({
                "panes": panes,
            }),
        ),
    };

    write.send(Message::Text(encoded)).await?;
    Ok(())
}

/// Handle incoming messages from the cloud
fn handle_cloud_message(
    text: &str,
    relay_tx: &mpsc::UnboundedSender<RelayEvent>,
    cas_dir: Option<&std::path::Path>,
    factory_session: Option<&str>,
) {
    let Ok(arr) = serde_json::from_str::<Vec<Value>>(text) else {
        return;
    };
    if arr.len() < 5 {
        return;
    }

    let event = arr[3].as_str().unwrap_or("");
    let payload = &arr[4];

    match event {
        "cloud.connected" => {
            tracing::info!("Cloud acknowledged connection");
        }
        "cloud.heartbeat_ack" => {
            // No-op, connection is alive
        }
        "cloud.command" => {
            let command_type = payload
                .get("command_type")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let params = payload
                .get("params")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            tracing::info!("Cloud command received: {}", command_type);
            if let Some(dir) = cas_dir {
                handle_cloud_command(dir, command_type, &params, factory_session);
            }
        }
        "cloud.shutdown" => {
            tracing::warn!("Cloud requested factory shutdown");
        }
        // Terminal relay events
        "cloud.attach_request" => {
            let client_id = payload
                .get("client_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cols = payload.get("cols").and_then(|v| v.as_u64()).unwrap_or(120) as u16;
            let rows = payload.get("rows").and_then(|v| v.as_u64()).unwrap_or(40) as u16;
            let mode = payload
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("interactive")
                .to_string();
            tracing::info!("Relay attach request from client {}", client_id);
            let _ = relay_tx.send(RelayEvent::AttachRequest {
                client_id,
                cols,
                rows,
                mode,
            });
        }
        "cloud.pty_input" => {
            let client_id = payload
                .get("client_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let data = payload
                .get("data")
                .and_then(|v| v.as_str())
                .map(|s| {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(s)
                        .unwrap_or_else(|_| s.as_bytes().to_vec())
                })
                .unwrap_or_default();
            let _ = relay_tx.send(RelayEvent::PtyInput { client_id, data });
        }
        "cloud.resize" => {
            let client_id = payload
                .get("client_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cols = payload.get("cols").and_then(|v| v.as_u64()).unwrap_or(120) as u16;
            let rows = payload.get("rows").and_then(|v| v.as_u64()).unwrap_or(40) as u16;
            let _ = relay_tx.send(RelayEvent::Resize {
                client_id,
                cols,
                rows,
            });
        }
        "cloud.detach" => {
            let client_id = payload
                .get("client_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            tracing::info!("Relay detach from client {}", client_id);
            let _ = relay_tx.send(RelayEvent::Detach { client_id });
        }
        // Per-pane terminal relay events (from web UI)
        "cloud.pane_attach" => {
            let pane = payload
                .get("pane")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cols = payload
                .get("cols")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16);
            let rows = payload
                .get("rows")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16);
            tracing::info!(
                "Web pane attach: {} (cols={:?}, rows={:?})",
                pane,
                cols,
                rows
            );
            let _ = relay_tx.send(RelayEvent::PaneAttach { pane, cols, rows });
        }
        "cloud.pane_resize" => {
            let pane = payload
                .get("pane")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cols = payload.get("cols").and_then(|v| v.as_u64()).unwrap_or(120) as u16;
            let rows = payload.get("rows").and_then(|v| v.as_u64()).unwrap_or(40) as u16;
            tracing::info!("Web pane resize: {} ({}x{})", pane, cols, rows);
            let _ = relay_tx.send(RelayEvent::PaneResize { pane, cols, rows });
        }
        "cloud.pane_detach" => {
            let pane = payload
                .get("pane")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            tracing::info!("Web pane detach: {}", pane);
            let _ = relay_tx.send(RelayEvent::PaneDetach { pane });
        }
        "cloud.pane_input" => {
            let pane = payload
                .get("pane")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let data = payload
                .get("data")
                .and_then(|v| v.as_str())
                .map(|s| {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(s)
                        .unwrap_or_else(|_| s.as_bytes().to_vec())
                })
                .unwrap_or_default();
            let _ = relay_tx.send(RelayEvent::PaneInput { pane, data });
        }
        "phx_reply" | "phx_error" | "phx_close" => {
            // Protocol messages, no action needed
        }
        _ => {
            tracing::debug!("Unhandled cloud event: {}", event);
        }
    }
}

/// Handle a cloud command by writing to the appropriate local queue.
///
/// Commands arrive from the web UI via Phoenix channel broadcast.
/// The daemon's main loop picks up queued items on its next tick.
fn handle_cloud_command(
    cas_dir: &std::path::Path,
    command_type: &str,
    params: &Value,
    factory_session: Option<&str>,
) {
    match command_type {
        "message" => {
            let target = params
                .get("target")
                .and_then(|t| t.as_str())
                .unwrap_or("supervisor");
            let message = params.get("message").and_then(|m| m.as_str()).unwrap_or("");
            let summary = params.get("summary").and_then(|s| s.as_str());
            if message.is_empty() {
                tracing::warn!("Cloud message: empty message, skipping");
                return;
            }
            match crate::store::open_prompt_queue_store(cas_dir) {
                Ok(queue) => {
                    let result = queue.enqueue_with_summary(
                        "cloud",
                        target,
                        message,
                        factory_session,
                        summary,
                    );
                    match result {
                        Ok(id) => {
                            tracing::info!("Cloud message queued (id={}) to '{}'", id, target)
                        }
                        Err(e) => {
                            tracing::error!("Failed to enqueue cloud message: {}", e)
                        }
                    }
                }
                Err(e) => tracing::error!("Failed to open prompt queue: {}", e),
            }
        }
        "spawn_workers" => {
            let count = params.get("count").and_then(|c| c.as_i64()).unwrap_or(1) as i32;
            let names: Vec<String> = params
                .get("names")
                .and_then(|n| n.as_str())
                .map(|s| {
                    s.split(',')
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let isolate = params
                .get("isolate")
                .and_then(|i| i.as_bool())
                .unwrap_or(false);
            // cas-2992: build per-worker spec from optional cli/model/effort fields
            // forwarded by the cloud relay.  Invalid values are logged and ignored so
            // a malformed cloud message doesn't bring down the handler.
            let spec_json_owned = {
                let cli = params.get("cli").and_then(|v| v.as_str());
                let model = params.get("model").and_then(|v| v.as_str());
                let effort = params.get("effort").and_then(|v| v.as_str());
                match crate::mcp::tools::service::factory_ops::build_spawn_spec_json_with_project_config(
                    cli,
                    model,
                    effort,
                    Some(cas_dir.join("config.toml")),
                ) {
                    Ok(j) => Some(j),
                    Err(e) => {
                        tracing::warn!(
                            "Cloud spawn_workers: invalid spec override ({}); ignoring",
                            e
                        );
                        None
                    }
                }
            };
            match crate::store::open_spawn_queue_store(cas_dir) {
                Ok(queue) => match queue.enqueue_spawn(
                    count,
                    &names,
                    isolate,
                    spec_json_owned.as_deref(),
                    factory_session,
                    // cas-6913: task_id pre-assignment is MCP-only for now —
                    // the cloud relay protocol has no task_id field yet.
                    None,
                ) {
                    Ok(id) => {
                        tracing::info!("Cloud spawn_workers queued (id={}): count={}", id, count)
                    }
                    Err(e) => tracing::error!("Failed to enqueue cloud spawn: {}", e),
                },
                Err(e) => tracing::error!("Failed to open spawn queue: {}", e),
            }
        }
        "shutdown_workers" => {
            let count = params
                .get("count")
                .and_then(|c| c.as_i64())
                .map(|c| c as i32);
            let names: Vec<String> = params
                .get("worker_names")
                .and_then(|n| n.as_str())
                .map(|s| {
                    s.split(',')
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let force = params
                .get("force")
                .and_then(|f| f.as_bool())
                .unwrap_or(false);
            match crate::store::open_spawn_queue_store(cas_dir) {
                Ok(queue) => match queue.enqueue_shutdown(count, &names, force, factory_session) {
                    Ok(id) => tracing::info!("Cloud shutdown_workers queued (id={})", id),
                    Err(e) => tracing::error!("Failed to enqueue cloud shutdown: {}", e),
                },
                Err(e) => tracing::error!("Failed to open spawn queue: {}", e),
            }
        }
        "sync_all_workers" => {
            // Sync is handled by writing to the spawn queue as a special no-op
            // that the daemon interprets. For now, log it.
            tracing::info!("Cloud sync_all_workers command received (handled by daemon tick)");
        }
        "stop_factory" => {
            tracing::warn!("Cloud stop_factory command received");
            // The server already calls Agents.shutdown_factory — the daemon will
            // detect the shutdown state on its next heartbeat/state check.
        }
        _ => {
            tracing::warn!("Unknown cloud command: {}", command_type);
        }
    }
}

/// Serialize DirectorData into a JSON Value suitable for the cloud
pub fn serialize_factory_state(
    factory_id: &str,
    supervisor_name: &str,
    data: &cas_factory::DirectorData,
) -> Value {
    let agents: Vec<Value> = data
        .agents
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "name": a.name,
                "status": format!("{:?}", a.status),
                "current_task": a.current_task,
                "last_heartbeat": a.last_heartbeat.map(|t| t.to_rfc3339()),
            })
        })
        .collect();

    let ready_tasks: Vec<Value> = data
        .ready_tasks
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "title": t.title,
                "status": t.status,
                "priority": t.priority,
                "assignee": t.assignee,
            })
        })
        .collect();

    let in_progress_tasks: Vec<Value> = data
        .in_progress_tasks
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "title": t.title,
                "status": t.status,
                "priority": t.priority,
                "assignee": t.assignee,
            })
        })
        .collect();

    serde_json::json!({
        "factory_id": factory_id,
        "supervisor_name": supervisor_name,
        "agents": agents,
        "ready_tasks": ready_tasks,
        "in_progress_tasks": in_progress_tasks,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

// Phoenix protocol tests (ws_url, encode_msg, refs) are in ui::factory::phoenix::tests
