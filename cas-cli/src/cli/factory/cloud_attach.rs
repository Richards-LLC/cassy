//! Cloud relay attach — connect to a remote factory via Cassy Cloud WebSocket
//!
//! Opens a WebSocket to the cloud's terminal relay channel, sends user.attach,
//! receives PTY frames and renders in the local terminal, forwards keystrokes.

use anyhow::{Result, bail};
use base64::Engine;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::io::{self, Write};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

use crate::cloud::CloudConfig;
use crate::ui::components::{Formatter, Renderable, StatusLine};
use crate::ui::factory::phoenix::{encode_msg, ws_url};
use crate::ui::theme::ActiveTheme;

/// Execute cloud relay attach: connect to cloud, join terminal_relay channel,
/// forward I/O between local terminal and remote factory.
pub fn execute_cloud_attach(factory_id: &str) -> Result<()> {
    let cloud_config =
        CloudConfig::load().map_err(|e| anyhow::anyhow!("Failed to load cloud config: {e}"))?;

    if !cloud_config.is_logged_in() {
        bail!(
            "Not logged in to Cassy Cloud. Run 'cas auth login' first.\n\
             Cloud relay requires authentication to route terminal traffic."
        );
    }

    let token = cloud_config.token.unwrap_or_default();
    let endpoint = cloud_config.endpoint;

    {
        let theme = ActiveTheme::default();
        let mut stderr = io::stderr();
        let mut fmt = Formatter::stdout(&mut stderr, theme);
        StatusLine::info(format!(
            "Connecting to factory {factory_id} via cloud relay..."
        ))
        .render(&mut fmt)?;
    }

    // Run the async relay loop
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(relay_loop(&endpoint, &token, factory_id))
}

async fn relay_loop(endpoint: &str, token: &str, factory_id: &str) -> Result<()> {
    let url = ws_url(endpoint, token);

    // Connect WebSocket
    let (ws_stream, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to cloud: {e}"))?;
    let (mut write, mut read) = ws_stream.split();

    let topic = format!("terminal_relay:{factory_id}");
    let join_ref = "1";

    // Get terminal size
    let (cols, rows) = terminal::size().unwrap_or((120, 40));

    // Join the terminal relay channel
    let join_payload = serde_json::json!({
        "role": "user",
        "cols": cols,
        "rows": rows,
    });
    let join_msg = encode_msg(Some(join_ref), &topic, "phx_join", &join_payload);
    write.send(Message::Text(join_msg)).await?;

    // Wait for join reply + client_id
    #[allow(unused_assignments)]
    let mut client_id = String::new();
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
                                        // Extract client_id from response
                                        client_id = arr[4]
                                            .get("response")
                                            .and_then(|r| r.get("client_id"))
                                            .and_then(|c| c.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        break;
                                    } else {
                                        bail!("Channel join rejected: {:?}", arr[4]);
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        bail!("Connection closed during join");
                    }
                    _ => {}
                }
            }
            _ = &mut join_timeout => {
                bail!("Channel join timed out");
            }
        }
    }

    {
        let theme = ActiveTheme::default();
        let mut stderr = io::stderr();
        let mut fmt = Formatter::stdout(&mut stderr, theme);
        StatusLine::success(format!(
            "Connected to factory {} (client {}). Press Ctrl+D to detach.",
            factory_id,
            &client_id[..client_id.len().min(8)]
        ))
        .render(&mut fmt)?;
    }

    // Wait for relay.attach_accept before entering raw mode
    let attach_timeout = tokio::time::sleep(Duration::from_secs(15));
    tokio::pin!(attach_timeout);

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(&text) {
                            if arr.len() >= 5 {
                                let event = arr[3].as_str().unwrap_or("");
                                match event {
                                    "relay.attach_accept" => {
                                        break;
                                    }
                                    "relay.attach_reject" => {
                                        let reason = arr[4].get("reason")
                                            .and_then(|r| r.as_str())
                                            .unwrap_or("unknown");
                                        bail!("Factory rejected attach: {reason}");
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        bail!("Connection closed waiting for attach accept");
                    }
                    _ => {}
                }
            }
            _ = &mut attach_timeout => {
                bail!("Timed out waiting for factory to accept attach");
            }
        }
    }

    // Enter raw terminal mode
    enable_raw_mode()?;
    // Enter alternate screen, hide cursor
    write!(io::stdout(), "\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H")?;
    let _ = io::stdout().flush();

    // Main relay loop
    let result = relay_io_loop(&mut write, &mut read, &topic, &client_id).await;

    // Restore terminal
    disable_raw_mode()?;
    write!(io::stdout(), "\x1b[?1049l\x1b[?25h")?;
    let _ = io::stdout().flush();

    {
        let theme = ActiveTheme::default();
        let mut stderr = io::stderr();
        let mut fmt = Formatter::stdout(&mut stderr, theme);
        if let Err(e) = result {
            StatusLine::error(format!("Relay disconnected: {e}")).render(&mut fmt)?;
        } else {
            StatusLine::info(format!("Detached from factory {factory_id}.")).render(&mut fmt)?;
        }
    }

    Ok(())
}

async fn relay_io_loop<W, R>(
    write: &mut W,
    read: &mut R,
    topic: &str,
    _client_id: &str,
) -> Result<()>
where
    W: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut heartbeat_timer = tokio::time::interval(Duration::from_secs(30));
    heartbeat_timer.tick().await; // Skip first immediate tick

    let mut pending_resize: Option<(u16, u16)> = None;
    let mut pending_resize_at = tokio::time::Instant::now();

    loop {
        // Poll terminal events (non-blocking, 10ms timeout)
        let has_event = tokio::task::block_in_place(|| event::poll(Duration::from_millis(10)))?;

        if has_event {
            let evt = tokio::task::block_in_place(event::read)?;
            match evt {
                Event::Key(key) => {
                    // Ctrl+D = detach
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('d'))
                    {
                        let msg = encode_msg(None, topic, "user.detach", &serde_json::json!({}));
                        let _ = write.send(Message::Text(msg)).await;
                        return Ok(());
                    }

                    // Convert key to bytes and send
                    let data = key_to_bytes(&key);
                    if !data.is_empty() {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                        let payload = serde_json::json!({"data": b64});
                        let msg = encode_msg(None, topic, "user.input", &payload);
                        write.send(Message::Text(msg)).await?;
                    }
                }
                Event::Resize(cols, rows) => {
                    pending_resize = Some((cols, rows));
                    pending_resize_at = tokio::time::Instant::now();
                }
                _ => {}
            }
        }

        // Send debounced resize
        if let Some((cols, rows)) = pending_resize {
            if pending_resize_at.elapsed() >= Duration::from_millis(50) {
                let payload = serde_json::json!({"cols": cols, "rows": rows});
                let msg = encode_msg(None, topic, "user.resize", &payload);
                let _ = write.send(Message::Text(msg)).await;
                pending_resize = None;
            }
        }

        // Check for incoming WebSocket messages (non-blocking)
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_relay_message(&text)?;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Err(anyhow::anyhow!("Cloud connection closed"));
                    }
                    _ => {}
                }
            }
            _ = heartbeat_timer.tick() => {
                let hb = encode_msg(None, "phoenix", "heartbeat", &serde_json::json!({}));
                write.send(Message::Text(hb)).await?;
            }
            _ = tokio::time::sleep(Duration::from_millis(5)) => {
                // Brief sleep when no WS messages, then loop back to poll terminal events
            }
        }
    }
}

/// Handle an incoming relay message from the cloud
fn handle_relay_message(text: &str) -> Result<()> {
    let Ok(arr) = serde_json::from_str::<Vec<Value>>(text) else {
        return Ok(());
    };
    if arr.len() < 5 {
        return Ok(());
    }

    let event = arr[3].as_str().unwrap_or("");
    let payload = &arr[4];

    match event {
        "relay.pty_output" => {
            if let Some(data_b64) = payload.get("data").and_then(|d| d.as_str()) {
                if let Ok(data) = base64::engine::general_purpose::STANDARD.decode(data_b64) {
                    let mut stdout = io::stdout();
                    let _ = stdout.write_all(&data);
                    let _ = stdout.flush();
                }
            }
        }
        "relay.attach_reject" => {
            let reason = payload
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");
            return Err(anyhow::anyhow!("Factory rejected: {reason}"));
        }
        "phx_reply" | "phx_error" | "phx_close" => {}
        _ => {}
    }

    Ok(())
}

/// Convert a key event to terminal bytes (simplified version of client.rs key_to_bytes)
fn key_to_bytes(key: &crossterm::event::KeyEvent) -> Vec<u8> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            if c.is_ascii_alphabetic() {
                return vec![(c.to_ascii_lowercase() as u8) - b'a' + 1];
            }
        }
    }

    match key.code {
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Phoenix protocol tests (ws_url, encode_msg) are in ui::factory::phoenix::tests

    #[test]
    fn test_key_to_bytes_ctrl_d() {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState};
        let key = KeyEvent {
            code: KeyCode::Char('d'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };
        assert_eq!(key_to_bytes(&key), vec![0x04]);
    }

    #[test]
    fn test_key_to_bytes_arrow_keys() {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState};
        let key = KeyEvent {
            code: KeyCode::Up,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };
        assert_eq!(key_to_bytes(&key), vec![0x1b, b'[', b'A']);
    }
}
