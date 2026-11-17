//! Unix datagram notification socket for instant prompt queue wakeup.
//!
//! The factory daemon binds a `DaemonNotifier` that listens for single-byte
//! datagrams. Workers call `notify_daemon()` after enqueuing a prompt to wake
//! the daemon immediately instead of waiting for the next poll interval.

use std::os::unix::net::UnixDatagram as StdUnixDatagram;
use std::path::{Path, PathBuf};
use tokio::net::UnixDatagram;

/// Returns the canonical path for the notification socket.
pub fn notify_socket_path(cas_dir: &Path) -> PathBuf {
    cas_dir.join("notify.sock")
}

/// Daemon-side notification receiver.
///
/// Binds a Unix datagram socket and waits for notification bytes from workers.
/// Used in a `tokio::select!` branch to wake the event loop instantly when
/// new prompts are enqueued.
pub struct DaemonNotifier {
    /// Bound std socket — converted to tokio lazily on first async use so that
    /// `bind()` can be called before a Tokio runtime exists.
    std_socket: Option<StdUnixDatagram>,
    socket: Option<UnixDatagram>,
    path: PathBuf,
}

impl DaemonNotifier {
    /// Bind the notification socket at `{cas_dir}/notify.sock`.
    ///
    /// Removes a stale socket file from a previous run if one exists.
    /// Safe to call before a Tokio runtime is active.
    pub fn bind(cas_dir: &Path) -> std::io::Result<Self> {
        let path = notify_socket_path(cas_dir);

        // Remove stale socket from a previous daemon run
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let std_socket = StdUnixDatagram::bind(&path)?;
        std_socket.set_nonblocking(true)?;
        Ok(Self {
            std_socket: Some(std_socket),
            socket: None,
            path,
        })
    }

    /// Convert the std socket to a tokio socket. Must be called from within a
    /// Tokio runtime. Idempotent — safe to call multiple times.
    fn tokio_socket(&mut self) -> std::io::Result<&UnixDatagram> {
        if self.socket.is_none() {
            let std_sock = self
                .std_socket
                .take()
                .expect("std_socket already consumed");
            self.socket = Some(UnixDatagram::from_std(std_sock)?);
        }
        Ok(self.socket.as_ref().unwrap())
    }

    /// Async wait for a notification byte. Cancellation-safe (tokio
    /// `UnixDatagram::recv` is cancellation-safe).
    pub async fn recv(&mut self) -> std::io::Result<()> {
        let mut buf = [0u8; 64];
        self.tokio_socket()?.recv(&mut buf).await?;
        Ok(())
    }

    /// Non-blocking drain of all pending datagrams to coalesce multiple
    /// notifications into a single wakeup.
    pub fn drain(&mut self) {
        let mut buf = [0u8; 64];
        if let Ok(sock) = self.tokio_socket() {
            while sock.try_recv(&mut buf).is_ok() {}
        }
    }

    /// Remove the socket file (called on shutdown).
    pub fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for DaemonNotifier {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Fire-and-forget notification to the daemon.
///
/// Sends a single byte to `{cas_dir}/notify.sock`. This is synchronous and
/// non-blocking: if the socket buffer is full or the daemon is not listening,
/// it silently returns `Ok(())`.
pub fn notify_daemon(cas_dir: &Path) -> std::io::Result<()> {
    let path = notify_socket_path(cas_dir);
    let sock = StdUnixDatagram::unbound()?;
    sock.set_nonblocking(true)?;
    match sock.send_to(&[1u8], &path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn bind_creates_socket_file() {
        let dir = TempDir::new().unwrap();
        let notifier = DaemonNotifier::bind(dir.path()).unwrap();
        assert!(notify_socket_path(dir.path()).exists());
        drop(notifier);
    }

    #[tokio::test]
    async fn notify_and_recv_round_trip() {
        let dir = TempDir::new().unwrap();
        let mut notifier = DaemonNotifier::bind(dir.path()).unwrap();

        // Send a notification from the "worker" side
        notify_daemon(dir.path()).unwrap();

        // Receive it on the daemon side (should complete immediately)
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(10), notifier.recv()).await;
        assert!(result.is_ok(), "recv should complete within timeout");
        assert!(result.unwrap().is_ok());
    }

    #[tokio::test]
    async fn notify_when_no_listener_is_noop() {
        let dir = TempDir::new().unwrap();
        // No DaemonNotifier bound — notify_daemon should not error
        let result = notify_daemon(dir.path());
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn drain_clears_pending_notifications() {
        let dir = TempDir::new().unwrap();
        let mut notifier = DaemonNotifier::bind(dir.path()).unwrap();

        // First recv registers the tokio socket with the reactor — without
        // this, try_recv inside drain() will never see pending datagrams.
        notify_daemon(dir.path()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), notifier.recv()).await.unwrap().unwrap();

        // Now send several more notifications
        for _ in 0..5 {
            notify_daemon(dir.path()).unwrap();
        }
        notifier.tokio_socket().unwrap().readable().await.unwrap();

        // Drain should clear all pending
        notifier.drain();

        // After drain, recv should block (no data pending)
        let mut buf = [0u8; 64];
        let error = notifier.tokio_socket().unwrap().try_recv(&mut buf).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[tokio::test]
    async fn cleanup_removes_socket_file() {
        let dir = TempDir::new().unwrap();
        let notifier = DaemonNotifier::bind(dir.path()).unwrap();
        let path = notify_socket_path(dir.path());
        assert!(path.exists());
        notifier.cleanup();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn stale_socket_is_removed_on_bind() {
        let dir = TempDir::new().unwrap();
        let path = notify_socket_path(dir.path());

        // Create a stale socket file (simulate previous crash)
        let _first = DaemonNotifier::bind(dir.path()).unwrap();
        assert!(path.exists());
        // Leak the first notifier (don't drop/cleanup) by forgetting it
        std::mem::forget(_first);
        assert!(path.exists());

        // Second bind should succeed by removing the stale socket
        let _second = DaemonNotifier::bind(dir.path()).unwrap();
        assert!(path.exists());
    }
}
