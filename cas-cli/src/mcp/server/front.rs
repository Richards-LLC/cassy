//! Stable client transport; only the private backend is replaced on an update.
use anyhow::Result;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

type Reader = Box<dyn AsyncRead + Unpin + Send>;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

struct Backend {
    input: Writer,
    output: Reader,
    control_input: Writer,
    control_output: Reader,
    child: Option<tokio::process::Child>,
}

#[derive(Debug)]
enum Event {
    Serving(String),
    Prepared,
    Deferred(String),
}

async fn relay<R, W>(
    _input: R,
    _output: W,
    _backend: Backend,
    _updates: mpsc::Receiver<Backend>,
    _events: mpsc::Sender<Event>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    anyhow::bail!("stable MCP front is not implemented")
}

#[cfg(test)]
#[path = "front/tests.rs"]
mod tests;
