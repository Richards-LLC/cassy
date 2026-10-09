//! Answer client requests that arrive before `initialize` (cas-2a49, GH #1143).
//!
//! Claude Code probes a stdio server with a `server/discover` request
//! (id `server-discover-probe-1`) before it sends `initialize`. rmcp's server
//! handshake requires `initialize` as the first message. It logged
//! `expect initialized request, but received ... server/discover` and the
//! process exited. Claude then restarted `cas serve` once without the probe.
//! The exited first server had already registered itself as the worker's
//! `pid`. Until the replacement re-registered, the factory boot verifier saw
//! a dead PID. On macOS, which has no `/proc` fallback, it then killed a
//! healthy worker. The restart also added 12–28 s to every Claude MCP
//! connect.
//!
//! This guard sits between stdin and rmcp. Until `initialize` arrives:
//! - a request is answered with JSON-RPC `-32601 Method not found`, so the
//!   client falls back to the classic handshake;
//! - a notification or stray response is dropped.
//!
//! `initialize` itself, and anything the guard cannot classify, is forwarded,
//! and from then on every byte passes through unchanged. Unclassified input
//! keeps rmcp's own handling.

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream};

/// In-memory pipe capacity between the guard and rmcp's reader.
const FORWARD_BUFFER_BYTES: usize = 64 * 1024;

/// JSON-RPC "Method not found".
const METHOD_NOT_FOUND: i64 = -32601;

/// What the guard does with one newline-delimited message seen before
/// `initialize`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PreInitialize {
    /// `initialize` (or input the guard does not understand): forward it and
    /// switch to passthrough for the rest of the stream.
    Passthrough,
    /// A request other than `initialize`: write this reply, forward nothing.
    Reply(Vec<u8>),
    /// A notification, response or blank line: forward nothing.
    Drop,
}

/// Classify one raw line read before the handshake completed.
pub(crate) fn classify_pre_initialize(line: &[u8]) -> PreInitialize {
    if line.iter().all(u8::is_ascii_whitespace) {
        return PreInitialize::Drop;
    }
    let Ok(serde_json::Value::Object(message)) = serde_json::from_slice::<serde_json::Value>(line)
    else {
        return PreInitialize::Passthrough;
    };
    let Some(method) = message.get("method").and_then(serde_json::Value::as_str) else {
        // A response to nothing we sent.
        return PreInitialize::Drop;
    };
    if method == "initialize" {
        return PreInitialize::Passthrough;
    }
    let Some(id) = message.get("id").filter(|id| !id.is_null()) else {
        return PreInitialize::Drop;
    };
    let reply = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": METHOD_NOT_FOUND,
            "message": format!(
                "Method not found: {method} (this server answers requests only after initialize)"
            ),
        },
    });
    let mut bytes = serde_json::to_vec(&reply).unwrap_or_default();
    bytes.push(b'\n');
    PreInitialize::Reply(bytes)
}

/// Start the guard over `input`. Pre-initialize replies are written to
/// `replies`; the returned stream is what rmcp reads in place of stdin.
///
/// The guard writes `replies` only before it forwards `initialize`, and rmcp
/// writes nothing until it has read `initialize`. A shared stdout therefore
/// never interleaves. Every reply is flushed before `initialize` is
/// forwarded.
pub(crate) fn spawn<R, W>(input: R, replies: W) -> DuplexStream
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (mut forward, server_side) = tokio::io::duplex(FORWARD_BUFFER_BYTES);
    tokio::spawn(async move {
        let mut reader = BufReader::new(input);
        let mut replies = replies;
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line).await {
                // EOF or a read error before initialize: dropping `forward`
                // hands rmcp the EOF it would have seen on stdin.
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            match classify_pre_initialize(&line) {
                PreInitialize::Passthrough => {
                    if forward.write_all(&line).await.is_err() {
                        return;
                    }
                    break;
                }
                PreInitialize::Reply(reply) => {
                    eprintln!(
                        "[Cassy] Answered a pre-initialize request with Method not found (cas-2a49)"
                    );
                    if replies.write_all(&reply).await.is_err() || replies.flush().await.is_err() {
                        // The client is gone; let rmcp observe EOF.
                        return;
                    }
                }
                PreInitialize::Drop => {}
            }
        }
        drop(replies);
        let _ = tokio::io::copy_buf(&mut reader, &mut forward).await;
    });
    server_side
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    const PROBE: &str = r#"{"jsonrpc":"2.0","id":"server-discover-probe-1","method":"server/discover","params":{}}"#;
    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.1.259"}}}"#;

    #[test]
    fn probe_request_gets_method_not_found_with_its_id() {
        let PreInitialize::Reply(reply) = classify_pre_initialize(PROBE.as_bytes()) else {
            panic!("a pre-initialize request must be answered");
        };
        assert_eq!(reply.last(), Some(&b'\n'), "replies are newline-delimited");
        let value: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], "server-discover-probe-1");
        assert_eq!(value["error"]["code"], METHOD_NOT_FOUND);
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("server/discover")
        );
    }

    #[test]
    fn initialize_and_unparseable_input_pass_through() {
        assert_eq!(
            classify_pre_initialize(format!("{INITIALIZE}\n").as_bytes()),
            PreInitialize::Passthrough
        );
        assert_eq!(
            classify_pre_initialize(b"not json\n"),
            PreInitialize::Passthrough
        );
        assert_eq!(
            classify_pre_initialize(b"[1,2]\n"),
            PreInitialize::Passthrough
        );
    }

    #[test]
    fn notifications_responses_and_blank_lines_are_dropped() {
        assert_eq!(
            classify_pre_initialize(br#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#),
            PreInitialize::Drop
        );
        assert_eq!(
            classify_pre_initialize(br#"{"jsonrpc":"2.0","id":null,"method":"x"}"#),
            PreInitialize::Drop
        );
        assert_eq!(
            classify_pre_initialize(br#"{"jsonrpc":"2.0","id":3,"result":{}}"#),
            PreInitialize::Drop
        );
        assert_eq!(classify_pre_initialize(b"  \r\n"), PreInitialize::Drop);
    }

    /// The GH #1143 sequence: a probe, then initialize and post-handshake
    /// traffic. rmcp must see the stream starting at `initialize`, byte for
    /// byte, and the probe must be answered on the reply channel.
    #[tokio::test]
    async fn probe_is_answered_and_stream_resumes_at_initialize() {
        let after = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let tools = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#;
        let input = format!("{PROBE}\n{INITIALIZE}\n{after}\n{tools}\n");
        let (mut replies_reader, replies_writer) = tokio::io::duplex(4096);

        let mut server_side = spawn(std::io::Cursor::new(input.into_bytes()), replies_writer);
        let mut forwarded = String::new();
        server_side.read_to_string(&mut forwarded).await.unwrap();
        assert_eq!(
            forwarded,
            format!("{INITIALIZE}\n{after}\n{tools}\n"),
            "after initialize the guard is a byte-exact passthrough, even for a method it would have answered earlier"
        );

        let mut replies = String::new();
        replies_reader.read_to_string(&mut replies).await.unwrap();
        let lines: Vec<&str> = replies.lines().collect();
        assert_eq!(lines.len(), 1, "exactly the probe is answered: {replies}");
        let value: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(value["id"], "server-discover-probe-1");
    }

    #[tokio::test]
    async fn eof_before_initialize_reaches_rmcp_as_eof() {
        let (_replies_reader, replies_writer) = tokio::io::duplex(4096);
        let mut server_side = spawn(
            std::io::Cursor::new(format!("{PROBE}\n").into_bytes()),
            replies_writer,
        );
        let mut forwarded = Vec::new();
        server_side.read_to_end(&mut forwarded).await.unwrap();
        assert!(forwarded.is_empty());
    }

    /// Messages larger than the in-memory pipe must still stream through.
    #[tokio::test]
    async fn large_post_initialize_messages_stream_through() {
        let big = format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"blob":"{}"}}}}"#,
            "x".repeat(FORWARD_BUFFER_BYTES * 3)
        );
        let input = format!("{INITIALIZE}\n{big}\n");
        let (_replies_reader, replies_writer) = tokio::io::duplex(4096);
        let mut server_side = spawn(
            std::io::Cursor::new(input.clone().into_bytes()),
            replies_writer,
        );
        let mut forwarded = String::new();
        server_side.read_to_string(&mut forwarded).await.unwrap();
        assert_eq!(forwarded, input);
    }
}
