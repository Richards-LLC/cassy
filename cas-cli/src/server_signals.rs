//! Signal disposition for long-lived `cas` servers (cas-621ec, cas-5918).
//!
//! `main` resets SIGPIPE to `SIG_DFL` for every `cas` process, so that a
//! short-lived command piped into `head` exits quietly instead of panicking
//! on "Broken pipe". That disposition is fatal to a process that serves other
//! processes. The kernel raises SIGPIPE on any write to a socket or pipe whose
//! reader has gone, unless the write passes `MSG_NOSIGNAL`. Rust's std adds
//! that flag only on some paths: `TcpStream::write_vectored` and plain
//! `write(2)` calls do not. The factory daemon, the MCP server and the bridge
//! write to Unix sockets, and the hub writes HTTP through `writev`. A peer
//! that vanishes mid-write therefore killed the whole server, which also took
//! down every other client it was serving.
//!
//! Each long-lived server entry point calls [`ignore_sigpipe_for_server`]
//! before it serves. The failing write then returns `EPIPE`, the server drops
//! that one peer, and it keeps running. Short-lived commands never call it and
//! keep `main`'s `SIG_DFL`. An ignored signal survives `exec`, but children
//! spawned through `std::process::Command` (and so tokio's `Command` and
//! portable-pty's worker PTYs) still start with SIGPIPE at `SIG_DFL`: std
//! resets it in the child before exec (`POSIX_SPAWN_SETSIGDEF` on the
//! posix_spawn path). Worker shells and their pipelines are unaffected.
//!
//! Entry points (asserted by `every_long_lived_server_entry_point_ignores_sigpipe`):
//! - `cas hub serve`: `cli::hub::serve_foreground`
//! - `cas serve` (MCP): `cli::serve_execute`
//! - `cas bridge serve`: `bridge::server::serve`, after its startup banner
//! - the factory daemon: every process entry (`run_daemon`,
//!   `run_daemon_with_boot_progress`, `run_daemon_after_fork`, the forked
//!   children of `fork_into_daemon` and `fork_first_daemon`) and, as a
//!   backstop, `FactoryDaemon::run`.

/// Ignore SIGPIPE for the rest of this process's life, so a write to a peer
/// that has gone away fails with `EPIPE` instead of killing the server.
///
/// Call it only from a long-lived server entry point. It is idempotent.
pub(crate) fn ignore_sigpipe_for_server() {
    #[cfg(unix)]
    // SAFETY: installing SIG_IGN for SIGPIPE is async-signal-safe and does not
    // interact with any Rust-managed state.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How the forked child wrote to its gone peer.
    #[cfg(unix)]
    #[derive(Clone, Copy)]
    enum WriteKind {
        /// `write(2)`, what the factory daemon and the MCP socket use.
        Scalar,
        /// `writev(2)`, what hyper uses for the hub's HTTP/1 responses.
        Vectored,
    }

    /// Fork, reproduce `main`'s SIGPIPE reset, optionally apply the server
    /// fix, then write to a Unix socket whose reader has been dropped. Returns
    /// the raw wait status: exit 0 means the write failed with `EPIPE`, exit 3
    /// means it succeeded or failed some other way. The child only makes
    /// async-signal-safe calls (`signal`, `write`/`writev`, `_exit`), so forking
    /// from the multithreaded test harness is sound.
    #[cfg(unix)]
    fn write_to_closed_peer_in_child(ignore_sigpipe: bool, kind: WriteKind) -> libc::c_int {
        use std::io::{IoSlice, Write};
        use std::os::unix::net::UnixStream;

        let (writer, reader) = UnixStream::pair().unwrap();
        drop(reader);
        let payload = [b'x'; 64];
        // SAFETY: see the function comment; the child never returns.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            // SAFETY: async-signal-safe; this is exactly what `main` does.
            unsafe {
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            }
            if ignore_sigpipe {
                ignore_sigpipe_for_server();
            }
            let result = match kind {
                WriteKind::Scalar => (&writer).write(&payload),
                WriteKind::Vectored => {
                    (&writer).write_vectored(&[IoSlice::new(&payload), IoSlice::new(&payload)])
                }
            };
            let code = match result {
                Err(error) if error.raw_os_error() == Some(libc::EPIPE) => 0,
                _ => 3,
            };
            // SAFETY: terminate the forked child without running harness code.
            unsafe { libc::_exit(code) }
        }
        drop(writer);
        let mut status = 0;
        // SAFETY: waiting on our own child.
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(waited, pid, "waitpid failed");
        status
    }

    #[cfg(unix)]
    #[test]
    fn vectored_write_to_a_gone_peer_kills_a_process_with_mains_sigpipe_disposition() {
        // cas-621ec root cause: std's vectored socket write is a bare writev
        // with no MSG_NOSIGNAL, so under `main`'s SIG_DFL it is fatal.
        let status = write_to_closed_peer_in_child(false, WriteKind::Vectored);
        assert!(libc::WIFSIGNALED(status), "wait status {status:#x}");
        assert_eq!(libc::WTERMSIG(status), libc::SIGPIPE);
    }

    #[cfg(unix)]
    #[test]
    fn a_server_that_ignores_sigpipe_sees_epipe_for_every_write_kind() {
        for (kind, name) in [
            (WriteKind::Scalar, "write"),
            (WriteKind::Vectored, "writev"),
        ] {
            let status = write_to_closed_peer_in_child(true, kind);
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "{name}: the server must survive and observe EPIPE; wait status {status:#x}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn ignoring_sigpipe_twice_is_harmless() {
        // SAFETY: the child only calls async-signal-safe functions (`signal`,
        // `_exit`) before it exits.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            ignore_sigpipe_for_server();
            ignore_sigpipe_for_server();
            // SAFETY: querying the current disposition without changing it.
            let current = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
            // SAFETY: terminate the forked child without running harness code.
            unsafe { libc::_exit(if current == libc::SIG_IGN { 0 } else { 3 }) }
        }
        let mut status = 0;
        // SAFETY: waiting on our own child.
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(waited, pid, "waitpid failed");
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "wait status {status:#x}"
        );
    }

    /// The body of `fn <name>` in `source`, from its signature to the next
    /// top-level `fn` or `impl` (enough to find a call near its start).
    fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
        let start = source
            .find(signature)
            .unwrap_or_else(|| panic!("`{signature}` not found"));
        let rest = &source[start + signature.len()..];
        let end = [
            "\npub fn ",
            "\npub async fn ",
            "\nfn ",
            "\nasync fn ",
            "\n    pub fn ",
            "\n    pub async fn ",
            "\n    pub(crate) fn ",
            "\n    fn ",
            "\n    async fn ",
        ]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .unwrap_or(rest.len());
        &rest[..end]
    }

    #[test]
    fn every_long_lived_server_entry_point_ignores_sigpipe() {
        const CALL: &str = "ignore_sigpipe_for_server()";
        let cases: [(&str, &str, &str); 7] = [
            (
                "cli/hub.rs",
                include_str!("cli/hub.rs"),
                "fn serve_foreground(",
            ),
            (
                "cli/mod.rs",
                include_str!("cli/mod.rs"),
                "fn serve_execute(",
            ),
            (
                "bridge/server/mod.rs",
                include_str!("bridge/server/mod.rs"),
                "pub fn serve(",
            ),
            (
                "ui/factory/daemon/process.rs",
                include_str!("ui/factory/daemon/process.rs"),
                "pub async fn run_daemon(",
            ),
            (
                "ui/factory/daemon/process.rs",
                include_str!("ui/factory/daemon/process.rs"),
                "pub async fn run_daemon_with_boot_progress(",
            ),
            (
                "ui/factory/daemon/process.rs",
                include_str!("ui/factory/daemon/process.rs"),
                "pub async fn run_daemon_after_fork(",
            ),
            (
                "ui/factory/daemon/runtime/lifecycle.rs",
                include_str!("ui/factory/daemon/runtime/lifecycle.rs"),
                "pub async fn run(&mut self)",
            ),
        ];
        for (file, source, signature) in cases {
            assert!(
                function_body(source, signature).contains(CALL),
                "{file}: `{signature}` must call {CALL} before serving"
            );
        }
        // The two forked daemon children set it right after setsid, before
        // they talk to the parent's boot client.
        for (file, source) in [
            (
                "ui/factory/daemon/process.rs",
                include_str!("ui/factory/daemon/process.rs"),
            ),
            (
                "ui/factory/daemon/fork_first.rs",
                include_str!("ui/factory/daemon/fork_first.rs"),
            ),
        ] {
            let child = source
                .split("Ok(NixForkResult::Child) => {")
                .nth(1)
                .unwrap_or_else(|| panic!("{file}: no forked child branch"));
            let setsid = child
                .find("setsid()")
                .unwrap_or_else(|| panic!("{file}: child has no setsid"));
            let call = child
                .find(CALL)
                .unwrap_or_else(|| panic!("{file}: forked child must call {CALL}"));
            assert!(
                call > setsid && call - setsid < 400,
                "{file}: {CALL} belongs right after setsid"
            );
        }
    }

    #[test]
    fn short_lived_commands_keep_mains_sigpipe_default() {
        let main = include_str!("main.rs");
        assert!(
            main.contains("libc::signal(libc::SIGPIPE, libc::SIG_DFL)"),
            "main.rs must keep SIG_DFL for CLI commands"
        );
        assert!(
            !main.contains("ignore_sigpipe_for_server"),
            "main.rs must not ignore SIGPIPE for every command"
        );
    }
}
