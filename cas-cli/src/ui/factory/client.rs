//! Factory TUI client - attaches to factory daemon via Unix socket
//!
//! Connects to the daemon's Unix socket and forwards terminal I/O.
//! The daemon renders the full TUI and broadcasts raw terminal output.
//! This client just forwards bytes between the terminal and socket.

use crate::ui::factory::session::{SessionInfo, SessionManager};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cas_factory::SessionSummary;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use ghostty_vt::KeyModifiers as GhosttyKeyModifiers;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

/// Attach to an existing factory session via Unix socket
///
/// Connects to the daemon's Unix socket for the given session.
pub fn attach(session_name: Option<String>) -> anyhow::Result<()> {
    let session_manager = SessionManager::new();

    // Find session to attach to
    let session = session_manager
        .find_session(session_name.as_deref())?
        .ok_or_else(|| {
            if let Some(name) = &session_name {
                anyhow::anyhow!("Session '{name}' not found or not running")
            } else {
                anyhow::anyhow!("No running factory sessions found. Start one with 'cas'")
            }
        })?;

    if !session.can_attach() {
        anyhow::bail!(
            "Cannot attach to session '{}': daemon not running",
            session.name
        );
    }

    attach_unix(&session)
}

// Control sequence constants (must match daemon.rs)
const CONTROL_PREFIX: &[u8] = b"\x1b]777;";
const CONTROL_SUFFIX: u8 = 0x07; // BEL

// A signal handler cannot use crossterm's locks or buffered terminal I/O.
// Ask the client loop to restore, with an independent bound if I/O is stuck.
static CLIENT_EXIT_SIGNAL: AtomicI32 = AtomicI32::new(0);
const CLIENT_SIGNAL_GRACE: Duration = Duration::from_secs(1);

extern "C" fn record_client_exit_signal(signal: libc::c_int) {
    if let Err(first) =
        CLIENT_EXIT_SIGNAL.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed)
    {
        // Re-raise the first signal with its default disposition: a second
        // signal means terminal cleanup is stuck (for example in write_all).
        reraise_default_signal(first);
    }
}

fn reraise_default_signal(signal: libc::c_int) {
    // These POSIX calls are async-signal-safe. Unblock before raising because
    // this can also run inside the handler for the signal being re-raised.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        libc::sigaddset(&mut mask, signal);
        libc::sigprocmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut());
        libc::raise(signal);
    }
}

fn install_client_exit_handler(signal: libc::c_int) -> io::Result<Option<libc::sigaction>> {
    // nohup and callers may deliberately ignore a signal. Leave that
    // disposition alone instead of turning it into an attach-time exit.
    let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
    if unsafe { libc::sigaction(signal, std::ptr::null(), &mut current) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if current.sa_sigaction == libc::SIG_IGN {
        return Ok(None);
    }

    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = record_client_exit_signal as *const () as libc::sighandler_t;
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if previous.sa_sigaction == libc::SIG_IGN {
        // Another thread changed the disposition after our query. Restore it
        // immediately, and do not register it for Drop's death-by-signal path.
        if unsafe { libc::sigaction(signal, &previous, std::ptr::null_mut()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(None);
    }
    Ok(Some(previous))
}

/// Restore on every return and panic unwind, including partial setup failures.
/// Signal dispositions belong only to this attach, not later CLI operations.
struct ClientTerminalGuard {
    previous_handlers: Vec<(libc::c_int, libc::sigaction)>,
    raw_enabled: bool,
    stop_watchdog: Arc<AtomicBool>,
    watchdog: Option<std::thread::JoinHandle<()>>,
}

impl ClientTerminalGuard {
    fn enter() -> io::Result<Self> {
        let mut guard = Self {
            previous_handlers: Vec::new(),
            raw_enabled: false,
            stop_watchdog: Arc::new(AtomicBool::new(false)),
            watchdog: None,
        };
        CLIENT_EXIT_SIGNAL.store(0, Ordering::Relaxed);
        for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
            // Install before changing the terminal; Drop rolls back any partial
            // registration. The handler never touches terminal locks or I/O.
            if let Some(previous) = install_client_exit_handler(signal)? {
                guard.previous_handlers.push((signal, previous));
            }
        }
        let stop = Arc::clone(&guard.stop_watchdog);
        guard.watchdog = Some(
            std::thread::Builder::new()
                .name("factory-client-signal".into())
                .spawn(move || {
                    let mut signalled_at = None;
                    while !stop.load(Ordering::Acquire) {
                        let signal = CLIENT_EXIT_SIGNAL.load(Ordering::Relaxed);
                        if signal != 0 {
                            let since = signalled_at.get_or_insert_with(std::time::Instant::now);
                            if since.elapsed() >= CLIENT_SIGNAL_GRACE {
                                reraise_default_signal(signal);
                            }
                        }
                        std::thread::park_timeout(Duration::from_millis(100));
                    }
                })?,
        );
        enable_raw_mode()?;
        guard.raw_enabled = true;
        execute!(
            io::stdout(),
            crossterm::event::EnableBracketedPaste,
            crossterm::event::EnableMouseCapture
        )?;
        Ok(guard)
    }

    fn exit_signal(&self) -> i32 {
        CLIENT_EXIT_SIGNAL.load(Ordering::Relaxed)
    }
}

impl Drop for ClientTerminalGuard {
    fn drop(&mut self) {
        if self.raw_enabled {
            // Attempt every reset even if restoring termios or output fails.
            let _ = execute!(
                io::stdout(),
                crossterm::event::DisableMouseCapture,
                crossterm::event::DisableBracketedPaste,
                terminal::LeaveAlternateScreen,
                crossterm::cursor::Show
            );
            let _ = disable_raw_mode();
        }
        self.stop_watchdog.store(true, Ordering::Release);
        if let Some(watchdog) = self.watchdog.take() {
            watchdog.thread().unpark();
            let _ = watchdog.join();
        }
        // Keep signal interception until the terminal is safe again.
        for (signal, previous) in self.previous_handlers.iter().rev() {
            unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
        }
        let signal = self.exit_signal();
        if signal != 0 {
            // Preserve death-by-signal even when an error unwinds this guard.
            unsafe { libc::raise(signal) };
            // A previously handled or masked signal must still terminate.
            reraise_default_signal(signal);
        }
    }
}

/// Attach via Unix socket (local daemon mode)
///
/// The daemon renders the full TUI and broadcasts raw terminal output.
/// This client just forwards bytes between the terminal and socket.
fn attach_unix(session: &SessionInfo) -> anyhow::Result<()> {
    let sock_path = PathBuf::from(&session.metadata.socket_path);

    // Connect to daemon socket
    let mut stream = UnixStream::connect(&sock_path)
        .map_err(|e| anyhow::anyhow!("Failed to connect to daemon socket: {e}"))?;

    // Set socket to non-blocking for the read loop
    stream.set_nonblocking(true)?;

    // Check if stdin is a TTY
    let stdin_fd = std::os::unix::io::AsRawFd::as_raw_fd(&io::stdin());
    let is_tty = unsafe { libc::isatty(stdin_fd) } == 1;
    if !is_tty {
        anyhow::bail!(
            "Cannot attach: stdin is not a terminal.\n\
            Factory mode requires an interactive terminal."
        );
    }

    // Enable raw mode + mouse capture for scroll events.
    // Native text selection still works via Shift+click/drag in most terminals.
    let terminal_guard = ClientTerminalGuard::enter()?;

    // Send initial terminal size as control sequence
    if let Ok((cols, rows)) = terminal::size() {
        let resize_cmd = format!("resize;{cols};{rows}");
        let mut msg = Vec::new();
        msg.extend_from_slice(CONTROL_PREFIX);
        msg.extend_from_slice(resize_cmd.as_bytes());
        msg.push(CONTROL_SUFFIX);
        let _ = stream.write_all(&msg);

        // Auto-request compact mode for narrow terminals (phone SSH)
        if cols < 80 {
            let mut mode_msg = Vec::new();
            mode_msg.extend_from_slice(CONTROL_PREFIX);
            mode_msg.extend_from_slice(b"mode;compact");
            mode_msg.push(CONTROL_SUFFIX);
            let _ = stream.write_all(&mode_msg);
        }
    }

    let mut stdout = io::stdout();
    let mut read_buf = [0u8; 4096];

    // Client-side resize debounce: coalesce rapid resize events before sending
    let mut pending_resize: Option<(u16, u16)> = None;
    let mut pending_resize_at = std::time::Instant::now();

    // Mouse capture toggle (F10). When off, native terminal drag-selection works.
    let mut mouse_captured = true;
    const CLIENT_RESIZE_DEBOUNCE_MS: u64 = 50;
    // Main loop
    while terminal_guard.exit_signal() == 0 {
        // Read from socket (non-blocking)
        match stream.read(&mut read_buf) {
            Ok(0) => {
                // Connection closed
                break;
            }
            Ok(n) => {
                let data = &read_buf[..n];
                let _ = stdout.write_all(data);
                let _ = stdout.flush();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data available, continue
            }
            Err(_) => {
                // Error, exit
                break;
            }
        }

        // Poll for terminal events
        let polled = event::poll(Duration::from_millis(10));
        if terminal_guard.exit_signal() != 0 {
            break;
        }
        if polled? {
            let event = event::read();
            if terminal_guard.exit_signal() != 0 {
                break;
            }
            match event? {
                Event::Key(key) => {
                    // Ctrl+Q = detach this client (do not shut down daemon)
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('q'))
                    {
                        // Send detach control sequence
                        let mut msg = Vec::new();
                        msg.extend_from_slice(CONTROL_PREFIX);
                        msg.extend_from_slice(b"detach");
                        msg.push(CONTROL_SUFFIX);
                        let _ = stream.write_all(&msg);
                        break;
                    }

                    // F10 toggles mouse capture so native terminal selection works.
                    if matches!(key.code, KeyCode::F(10)) {
                        mouse_captured = !mouse_captured;
                        let mode_cmd: &[u8] = if mouse_captured {
                            let _ = execute!(stdout, crossterm::event::EnableMouseCapture);
                            b"mode;select_off"
                        } else {
                            let _ = execute!(stdout, crossterm::event::DisableMouseCapture);
                            b"mode;select_on"
                        };
                        let mut msg = Vec::new();
                        msg.extend_from_slice(CONTROL_PREFIX);
                        msg.extend_from_slice(mode_cmd);
                        msg.push(CONTROL_SUFFIX);
                        let _ = stream.write_all(&msg);
                        continue;
                    }

                    // Convert key to bytes and send
                    let data = key_to_bytes(&key);
                    if !data.is_empty() {
                        let _ = stream.write_all(&data);
                    }
                }
                Event::Resize(cols, rows) => {
                    // Debounce resize: store latest dims, send after quiet period
                    pending_resize = Some((cols, rows));
                    pending_resize_at = std::time::Instant::now();
                }
                Event::Mouse(mouse) => {
                    // Handle scroll and click events. Drag is ignored so native
                    // terminal selection (Shift+click/drag) still works.
                    let mouse_cmd = match mouse.kind {
                        crossterm::event::MouseEventKind::ScrollUp => Some("scroll_up"),
                        crossterm::event::MouseEventKind::ScrollDown => Some("scroll_down"),
                        crossterm::event::MouseEventKind::Down(
                            crossterm::event::MouseButton::Left,
                        ) => Some("click"),
                        _ => None,
                    };
                    if let Some(kind) = mouse_cmd {
                        let cmd = format!("mouse;{kind};{};{}", mouse.column, mouse.row);
                        let mut msg = Vec::new();
                        msg.extend_from_slice(CONTROL_PREFIX);
                        msg.extend_from_slice(cmd.as_bytes());
                        msg.push(CONTROL_SUFFIX);
                        let _ = stream.write_all(&msg);
                    }
                }
                Event::Paste(text) => {
                    if !contains_dropped_image_path(&text) {
                        // Forward the paste as a framed control event rather than
                        // raw bytes. crossterm already stripped the terminal's
                        // \x1b[200~/\x1b[201~ markers, so raw passthrough lets the
                        // daemon's byte parser treat embedded newlines as Enter and
                        // submit the prompt mid-paste. The daemon re-wraps this
                        // payload as a bracketed paste before injecting it into the
                        // focused pane (cas-5702).
                        let _ = stream.write_all(&encode_paste_control(&text));
                    } else {
                        // Preserve the original drop payload and route it as a drop event.
                        let encoded_payload = URL_SAFE_NO_PAD.encode(text.as_bytes());
                        let cmd = format!("drop_image;{};{};{encoded_payload}", u16::MAX, u16::MAX);
                        let mut msg = Vec::new();
                        msg.extend_from_slice(CONTROL_PREFIX);
                        msg.extend_from_slice(cmd.as_bytes());
                        msg.push(CONTROL_SUFFIX);
                        let _ = stream.write_all(&msg);
                    }
                }
                _ => {}
            }
        }

        // Send debounced resize if quiet period elapsed
        if let Some((cols, rows)) = pending_resize {
            if pending_resize_at.elapsed() >= Duration::from_millis(CLIENT_RESIZE_DEBOUNCE_MS) {
                let resize_cmd = format!("resize;{cols};{rows}");
                let mut msg = Vec::new();
                msg.extend_from_slice(CONTROL_PREFIX);
                msg.extend_from_slice(resize_cmd.as_bytes());
                msg.push(CONTROL_SUFFIX);
                let _ = stream.write_all(&msg);
                pending_resize = None;
            }
        }
    }

    drop(terminal_guard);

    Ok(())
}

/// Frame a paste payload as a `paste;<base64>` control event for the daemon.
/// base64 keeps newlines, `;`, and the BEL control suffix from corrupting the
/// CONTROL_PREFIX..CONTROL_SUFFIX framing; the daemon decodes it and re-wraps it
/// as a bracketed paste before injecting it into the focused pane (cas-5702).
fn encode_paste_control(text: &str) -> Vec<u8> {
    let encoded_payload = URL_SAFE_NO_PAD.encode(text.as_bytes());
    let cmd = format!("paste;{encoded_payload}");
    let mut msg = Vec::new();
    msg.extend_from_slice(CONTROL_PREFIX);
    msg.extend_from_slice(cmd.as_bytes());
    msg.push(CONTROL_SUFFIX);
    msg
}

fn contains_dropped_image_path(text: &str) -> bool {
    text.lines().any(|candidate| {
        normalize_image_path_candidate(candidate)
            .map(|path| is_image_path(&path))
            .unwrap_or(false)
    })
}

fn normalize_image_path_candidate(candidate: &str) -> Option<String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return None;
    }

    let unquoted = trimmed.trim_matches(|c| c == '"' || c == '\'');
    let path = if let Some(rest) = unquoted.strip_prefix("file://") {
        decode_file_uri_path(rest)
    } else {
        unquoted.to_string()
    };

    if is_image_path(&path) {
        Some(path)
    } else {
        None
    }
}

fn decode_file_uri_path(uri_path: &str) -> String {
    let bytes = uri_path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = bytes[i + 1] as char;
            let lo = bytes[i + 2] as char;
            if let (Some(hi), Some(lo)) = (hi.to_digit(16), lo.to_digit(16)) {
                decoded.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&decoded).to_string()
}

fn is_image_path(path: &str) -> bool {
    let stripped = path.split('?').next().unwrap_or(path);
    let stripped = stripped.split('#').next().unwrap_or(stripped);
    std::path::Path::new(stripped)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "tif"
                    | "tiff"
                    | "heic"
                    | "heif"
                    | "avif"
                    | "svg"
            )
        })
}

/// List all factory sessions
pub fn list_sessions() -> anyhow::Result<Vec<SessionInfo>> {
    let manager = SessionManager::new();
    manager.list_sessions().map_err(|e| e.into())
}

/// List factory sessions for a specific project directory
pub fn list_sessions_for_project(project_dir: &str) -> anyhow::Result<Vec<SessionInfo>> {
    let manager = SessionManager::new();
    let sessions = manager.list_sessions()?;
    Ok(sessions
        .into_iter()
        .filter(|s| {
            s.metadata
                .project_dir
                .as_ref()
                .is_some_and(|p| p == project_dir)
        })
        .collect())
}

/// Find a running session for a specific project directory
pub fn find_session_for_project(
    project_dir: &str,
    name: Option<&str>,
) -> anyhow::Result<Option<SessionInfo>> {
    let manager = SessionManager::new();
    manager
        .find_session_for_project(name, project_dir)
        .map_err(|e| e.into())
}

/// List all factory sessions as unified SessionSummary.
///
/// This provides rich session metadata using the unified session model
/// from `cas_factory::UnifiedSessionManager`.
pub fn list_session_summaries() -> anyhow::Result<Vec<SessionSummary>> {
    let sessions = list_sessions()?;
    Ok(sessions.iter().map(|s| s.to_session_summary()).collect())
}

/// List factory sessions for a project as unified SessionSummary.
pub fn list_session_summaries_for_project(
    project_dir: &str,
) -> anyhow::Result<Vec<SessionSummary>> {
    let sessions = list_sessions_for_project(project_dir)?;
    Ok(sessions.iter().map(|s| s.to_session_summary()).collect())
}

/// Convert a key event to terminal bytes using ghostty_vt's key encoding
fn key_to_bytes(key: &KeyEvent) -> Vec<u8> {
    // Convert crossterm modifiers to ghostty_vt modifiers
    let mods = GhosttyKeyModifiers {
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        super_key: key.modifiers.contains(KeyModifiers::SUPER),
    };

    // Handle Ctrl+char specially.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            if let Some(ctrl) = ctrl_char_to_byte(c) {
                return vec![ctrl];
            }
        }
    }

    // Try ghostty_vt's encode_key for named keys (handles modifiers properly)
    let key_name = match key.code {
        KeyCode::Up => Some("up"),
        KeyCode::Down => Some("down"),
        KeyCode::Left => Some("left"),
        KeyCode::Right => Some("right"),
        KeyCode::Home => Some("home"),
        KeyCode::End => Some("end"),
        KeyCode::PageUp => Some("page_up"),
        KeyCode::PageDown => Some("page_down"),
        KeyCode::Delete => Some("delete"),
        KeyCode::Insert => Some("insert"),
        KeyCode::F(1) => Some("f1"),
        KeyCode::F(2) => Some("f2"),
        KeyCode::F(3) => Some("f3"),
        KeyCode::F(4) => Some("f4"),
        KeyCode::F(5) => Some("f5"),
        KeyCode::F(6) => Some("f6"),
        KeyCode::F(7) => Some("f7"),
        KeyCode::F(8) => Some("f8"),
        KeyCode::F(9) => Some("f9"),
        KeyCode::F(10) => Some("f10"),
        KeyCode::F(11) => Some("f11"),
        KeyCode::F(12) => Some("f12"),
        _ => None,
    };

    if let Some(name) = key_name {
        if let Some(encoded) = ghostty_vt::encode_key(name, mods) {
            return encoded;
        }
    }

    // Handle remaining keys manually
    match key.code {
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        _ => Vec::new(),
    }
}

fn ctrl_char_to_byte(c: char) -> Option<u8> {
    if c.is_ascii_alphabetic() {
        return Some((c.to_ascii_lowercase() as u8) - b'a' + 1);
    }

    // ASCII control punctuation mappings.
    // Include both punctuation and crossterm's canonical digit forms:
    // Ctrl+3/[/ESC, Ctrl+4/\\, Ctrl+5/], Ctrl+6/^, Ctrl+7/_.
    match c {
        '@' | '`' | ' ' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;
    use crossterm::event::KeyEventState;

    #[test]
    fn ignored_client_signal_disposition_survives_handler_setup() {
        const CHILD: &str = "CAS_IGNORED_CLIENT_SIGNAL_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let mut ignored: libc::sigaction = unsafe { std::mem::zeroed() };
            ignored.sa_sigaction = libc::SIG_IGN;
            unsafe { libc::sigemptyset(&mut ignored.sa_mask) };
            assert_eq!(
                unsafe { libc::sigaction(libc::SIGHUP, &ignored, std::ptr::null_mut()) },
                0
            );

            assert!(install_client_exit_handler(libc::SIGHUP).unwrap().is_none());
            let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut current) },
                0
            );
            assert_eq!(current.sa_sigaction, libc::SIG_IGN);
            assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
            assert_eq!(CLIENT_EXIT_SIGNAL.load(Ordering::Relaxed), 0);
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("ignored_client_signal_disposition_survives_handler_setup")
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stdout).to_string()
                + &String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn detects_dropped_images_from_plain_path_and_file_uri() {
        assert!(contains_dropped_image_path(
            "/tmp/screenshot.png\nfile:///Users/test/Pictures/image%201.jpeg\n/tmp/not-image.txt"
        ));
    }

    #[test]
    fn ignores_non_image_paste_content() {
        assert!(!contains_dropped_image_path("hello world\n/tmp/readme.md"));
    }

    #[test]
    fn key_to_bytes_encodes_ctrl_bracket_as_group_separator() {
        let key = KeyEvent {
            code: KeyCode::Char(']'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };
        assert_eq!(key_to_bytes(&key), vec![0x1d]);
    }

    #[test]
    fn key_to_bytes_encodes_ctrl_digit_five_as_group_separator() {
        let key = KeyEvent {
            code: KeyCode::Char('5'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };
        assert_eq!(key_to_bytes(&key), vec![0x1d]);
    }

    #[test]
    fn key_to_bytes_encodes_ctrl_alpha() {
        let key = KeyEvent {
            code: KeyCode::Char('d'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };
        assert_eq!(key_to_bytes(&key), vec![0x04]);
    }

    // cas-5702: a multi-line paste must be carried as a base64 control event so
    // the daemon can re-wrap it as a single bracketed paste. The wire bytes must
    // NOT contain a bare newline/carriage-return (which the inner CLI would treat
    // as Enter and submit the prompt mid-paste), and must round-trip exactly.
    #[test]
    fn encode_paste_control_frames_multiline_without_bare_newlines() {
        let text = "\nfirst line\nsecond line\r\nthird; with semicolon";
        let frame = encode_paste_control(text);

        assert!(frame.starts_with(CONTROL_PREFIX));
        assert_eq!(*frame.last().unwrap(), CONTROL_SUFFIX);
        // No bare submit-triggering bytes survive on the wire.
        assert!(!frame.contains(&b'\n'));
        assert!(!frame.contains(&b'\r'));

        // The payload between the framing decodes back to the exact original.
        let body = &frame[CONTROL_PREFIX.len()..frame.len() - 1];
        let cmd = std::str::from_utf8(body).unwrap();
        let b64 = cmd.strip_prefix("paste;").expect("paste; prefix");
        let decoded = URL_SAFE_NO_PAD.decode(b64).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), text);
    }
}
