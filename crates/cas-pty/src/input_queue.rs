//! Non-blocking PTY input (factory loop v2 C4a, spec §3.1).
//!
//! A PTY master write is a blocking `write_all`: when the child stops reading
//! and the kernel buffer fills, the writer waits. Before C4a that wait ran on
//! the daemon loop, so one pane that stopped reading stalled input, echo and
//! delivery for every pane.
//!
//! Each [`crate::Pty`] now owns a [`PtyInput`]: writers append to a per-pane
//! buffer and return at once, and a writer thread of the pane's own does the
//! blocking `write_all`. A pane whose buffer holds [`PTY_INPUT_BUFFER_CAP`]
//! bytes refuses more input with [`crate::Error::InputFull`] until its child
//! reads; other panes are unaffected. Bytes keep their order: every write for
//! the pane goes through this one queue.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// Bytes a pane may hold unwritten before it stops taking input.
pub const PTY_INPUT_BUFFER_CAP: usize = 256 * 1024;

/// The largest write [`PtyInput::write_control_then`] takes.
pub const CONTROL_WRITE_MAX: usize = 64;

/// Called on the writer thread once a chunk is written, or failed.
pub type WriteDone = Box<dyn FnOnce(std::io::Result<()>) + Send + 'static>;

type SharedWriter = Arc<tokio::sync::Mutex<Box<dyn Write + Send>>>;

struct Chunk {
    bytes: Vec<u8>,
    done: Option<WriteDone>,
}

#[derive(Default)]
struct State {
    chunks: VecDeque<Chunk>,
    /// Bytes queued or being written.
    buffered: usize,
    closed: bool,
    /// The first write error; the pane takes no more input after it.
    failed: Option<String>,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when a chunk is queued or the queue closes.
    queued: Condvar,
    /// Signalled when a chunk finishes.
    written: Condvar,
    cap: usize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A pane's input queue. Cloning shares the queue.
#[derive(Clone)]
pub struct PtyInput {
    shared: Arc<Shared>,
}

/// Closes the queue when the owning [`crate::Pty`] drops, so the writer
/// thread exits once it has written what is queued.
pub(crate) struct PtyInputOwner(PtyInput);

impl Drop for PtyInputOwner {
    fn drop(&mut self) {
        let mut state = self.0.shared.lock();
        state.closed = true;
        drop(state);
        self.0.shared.queued.notify_all();
    }
}

impl PtyInputOwner {
    pub(crate) fn input(&self) -> &PtyInput {
        &self.0
    }
}

impl PtyInput {
    /// Start the writer thread for `writer`.
    pub(crate) fn start(id: &str, writer: SharedWriter, cap: usize) -> Result<PtyInputOwner> {
        let input = Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                queued: Condvar::new(),
                written: Condvar::new(),
                cap,
            }),
        };
        let shared = Arc::clone(&input.shared);
        std::thread::Builder::new()
            .name(format!("pty-input-{id}"))
            .spawn(move || writer_loop(&shared, &writer))
            .map_err(|error| Error::pty(format!("Failed to start PTY input writer: {error}")))?;
        Ok(PtyInputOwner(input))
    }

    /// Queue `data`. Never blocks on the PTY.
    pub fn write(&self, data: &[u8]) -> Result<()> {
        self.enqueue(data, None, false)
    }

    /// Queue `data` and call `done` on the writer thread once it is written
    /// (or failed). `done` is not called when this returns an error.
    pub fn write_then(&self, data: &[u8], done: WriteDone) -> Result<()> {
        self.enqueue(data, Some(done), false)
    }

    /// Queue a few control bytes past the cap: a prompt's submit CR, or a
    /// terminal reply (CPR). Refusing them would leave a queued payload
    /// unsubmitted, or a child waiting on its reply, for good (cas-786a6
    /// review H1, L2). Still refused once the pane failed or closed.
    pub fn write_control_then(&self, data: &[u8], done: Option<WriteDone>) -> Result<()> {
        debug_assert!(data.len() <= CONTROL_WRITE_MAX, "control writes are a few bytes");
        self.enqueue(data, done, true)
    }

    fn enqueue(&self, data: &[u8], done: Option<WriteDone>, control: bool) -> Result<()> {
        let mut state = self.shared.lock();
        if let Some(error) = &state.failed {
            return Err(Error::pty(format!("Write failed: {error}")));
        }
        if state.closed {
            return Err(Error::pty("PTY input is closed"));
        }
        // An empty buffer takes one chunk of any size (a large paste), so a
        // pane that is reading is never refused for good.
        if !control && state.buffered > 0 && state.buffered + data.len() > self.shared.cap {
            return Err(Error::InputFull {
                buffered: state.buffered,
                cap: self.shared.cap,
            });
        }
        state.buffered += data.len();
        state.chunks.push_back(Chunk {
            bytes: data.to_vec(),
            done,
        });
        drop(state);
        self.shared.queued.notify_one();
        Ok(())
    }

    /// Bytes queued or being written.
    pub fn buffered(&self) -> usize {
        self.shared.lock().buffered
    }

    /// Whether the pane's buffer is at its cap and refuses more input.
    pub fn is_full(&self) -> bool {
        self.shared.lock().buffered >= self.shared.cap
    }

    /// Block until everything queued is written, up to `timeout`. Returns
    /// whether the queue drained. For tests and shutdown; never the loop.
    pub fn wait_drained(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        while state.buffered > 0 && state.failed.is_none() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            state = self
                .shared
                .written
                .wait_timeout(state, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        state.buffered == 0
    }
}

fn writer_loop(shared: &Shared, writer: &SharedWriter) {
    loop {
        let chunk = {
            let mut state = shared.lock();
            loop {
                if let Some(chunk) = state.chunks.pop_front() {
                    break chunk;
                }
                if state.closed {
                    return;
                }
                state = shared
                    .queued
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let result = {
            // A fresh OS thread, never a Tokio worker: blocking_lock is safe.
            let mut writer = writer.blocking_lock();
            writer
                .write_all(&chunk.bytes)
                .and_then(|()| writer.flush())
        };
        let mut failed_chunks = Vec::new();
        {
            let mut state = shared.lock();
            state.buffered = state.buffered.saturating_sub(chunk.bytes.len());
            if let Err(error) = &result
                && state.failed.is_none()
            {
                // The pane takes no more input; what was queued behind the
                // failed chunk is failed too, in order.
                state.failed = Some(error.to_string());
                failed_chunks = state.chunks.drain(..).collect();
                state.buffered = 0;
            }
        }
        shared.written.notify_all();
        let error_text = result.as_ref().err().map(ToString::to_string);
        if let Some(done) = chunk.done {
            done(result);
        }
        for chunk in failed_chunks {
            if let Some(done) = chunk.done {
                done(Err(std::io::Error::other(
                    error_text.clone().unwrap_or_default(),
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that blocks until released, like a PTY whose child stopped
    /// reading.
    struct Gate(Arc<(Mutex<bool>, Condvar)>, Arc<Mutex<Vec<u8>>>);

    impl Write for Gate {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            let (open, cv) = &*self.0;
            let mut open = open.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
            self.1.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_blocked_writer_fills_the_buffer_and_then_refuses_without_blocking() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let out = Arc::new(Mutex::new(Vec::new()));
        let writer: SharedWriter = Arc::new(tokio::sync::Mutex::new(Box::new(Gate(
            Arc::clone(&gate),
            Arc::clone(&out),
        ))));
        let owner = PtyInput::start("gate", writer, 16).unwrap();
        let input = owner.input().clone();
        let writer_input = input.clone();
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let writes = std::thread::spawn(move || {
            let first = writer_input.write(b"0123456789");
            let second = writer_input.write(b"abcdef");
            let refused = writer_input.write(b"x");
            let full = writer_input.is_full();
            done_tx.send((first, second, refused, full)).unwrap();
        });

        // The child writer is still held at the gate. Completion is the
        // contract; the timeout only prevents a regressed blocking write from
        // hanging this test forever.
        let completed = done_rx.recv_timeout(Duration::from_secs(10));

        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        if completed.is_err() {
            writes.join().unwrap();
            panic!("input writes did not return while the PTY writer was held");
        }
        let (first, second, refused, full) = completed.unwrap();
        writes.join().unwrap();
        first.unwrap();
        second.unwrap();
        assert!(matches!(refused, Err(Error::InputFull { .. })));
        assert!(full);
        assert!(input.wait_drained(Duration::from_secs(5)));
        assert_eq!(&*out.lock().unwrap(), b"0123456789abcdef", "bytes keep their order");
        input.write(b"!").unwrap();
        assert!(input.wait_drained(Duration::from_secs(5)));
    }

    /// cas-786a6 review H1: a submit CR queued while the buffer is full is
    /// still taken, so the payload ahead of it is submitted once the child
    /// reads; ordinary input stays refused.
    #[test]
    fn a_submit_cr_is_taken_past_the_cap_and_written_after_its_payload() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let out = Arc::new(Mutex::new(Vec::new()));
        let writer: SharedWriter = Arc::new(tokio::sync::Mutex::new(Box::new(Gate(
            Arc::clone(&gate),
            Arc::clone(&out),
        ))));
        let owner = PtyInput::start("cr", writer, 8).unwrap();
        let input = owner.input().clone();
        input.write(b"payload!").unwrap();
        assert!(matches!(input.write(b"x"), Err(Error::InputFull { .. })));
        let (tx, rx) = std::sync::mpsc::channel();
        input
            .write_control_then(b"\r", Some(Box::new(move |result| tx.send(result.is_ok()).unwrap())))
            .expect("the submit CR is never refused for a full buffer");
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true));
        assert_eq!(&*out.lock().unwrap(), b"payload!\r");
    }

    #[test]
    fn a_failed_write_fails_its_followers_and_the_pane_takes_no_more_input() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("EIO"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let writer: SharedWriter = Arc::new(tokio::sync::Mutex::new(Box::new(Broken)));
        let owner = PtyInput::start("broken", writer, 1024).unwrap();
        let input = owner.input().clone();
        let (tx, rx) = std::sync::mpsc::channel();
        input
            .write_then(b"a", Box::new(move |result| tx.send(result.is_ok()).unwrap()))
            .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(false));
        assert!(input.write(b"b").is_err());
    }
}
