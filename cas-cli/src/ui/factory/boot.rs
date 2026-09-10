//! Factory boot screen - animated startup sequence for daemon initialization.

use std::thread;
use std::time::Duration;

/// Boot screen configuration
pub struct BootConfig {
    /// Supervisor name
    pub supervisor_name: String,
    /// Worker names
    pub worker_names: Vec<String>,
    /// Working directory
    pub cwd: String,
    /// Session name
    pub session_name: String,
    /// Startup profile summary (workers, mode, harness)
    pub profile: String,
    /// Skip animations (for testing)
    pub skip_animation: bool,
    /// Use minions theme
    pub minions_theme: bool,
}

mod screen;

use crate::ui::factory::boot::screen::BootScreen;

/// Boot screen client mode - receives progress from daemon socket
///
/// This is used in the fork-first architecture where the daemon does
/// all initialization and sends progress updates via socket.
pub fn run_boot_screen_client(
    boot_config: &BootConfig,
    sock_path: &std::path::Path,
    _daemon_pid: u32,
) -> anyhow::Result<()> {
    use crate::ui::factory::protocol::{self, DaemonMessage, FRAME_HEADER_SIZE, MAX_MESSAGE_SIZE};
    use std::collections::HashMap as AgentMap;
    use std::io::Read;

    let mut screen = BootScreen::new_themed(boot_config.skip_animation, boot_config.minions_theme)?;

    // Draw logo and get starting row
    let box_start = screen.draw_logo()?;
    screen.steps_row = box_start;

    // Draw the box
    screen.draw_box(
        &boot_config.session_name,
        &boot_config.cwd,
        &boot_config.profile,
        boot_config.worker_names.len(),
    )?;

    // Calculate row positions with better spacing
    let step_base = screen.steps_row + 7;
    let separator_row = step_base + 6; // 5 steps in fork-first mode with spacing
    screen.agent_row = separator_row + 2;

    // Connect to daemon socket with retry
    let stream = connect_with_retry(sock_path, Duration::from_secs(30))?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;

    // Track state for rendering
    let mut current_step: u16 = 0;
    let mut agent_rows: AgentMap<String, u16> = AgentMap::new();
    let mut next_agent_row = screen.agent_row;

    // Process messages until InitComplete
    let mut read_buf = Vec::new();
    let mut temp_buf = [0u8; 4096];

    loop {
        // Spin animation while waiting
        if !boot_config.skip_animation && current_step < 6 {
            screen.spin_step(step_base + current_step, 1)?;
        }

        // Try to read data
        match (&stream).read(&mut temp_buf) {
            Ok(0) => {
                // Connection closed
                anyhow::bail!("Daemon closed connection during initialization");
            }
            Ok(n) => {
                read_buf.extend_from_slice(&temp_buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data available, continue spinning
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                // Timeout, continue spinning
                continue;
            }
            Err(e) => {
                anyhow::bail!("Error reading from daemon: {e}");
            }
        }

        // Process all complete messages in buffer
        while read_buf.len() >= FRAME_HEADER_SIZE {
            let len = protocol::decode_length(read_buf[..FRAME_HEADER_SIZE].try_into().unwrap());
            if len > MAX_MESSAGE_SIZE {
                anyhow::bail!("Message too large: {len}");
            }

            if read_buf.len() < FRAME_HEADER_SIZE + len {
                // Need more data
                break;
            }

            // Extract and parse message
            let msg_data = &read_buf[FRAME_HEADER_SIZE..FRAME_HEADER_SIZE + len];
            let msg: DaemonMessage = serde_json::from_slice(msg_data)?;

            // Remove processed message from buffer
            read_buf = read_buf[FRAME_HEADER_SIZE + len..].to_vec();

            // Handle message
            match msg {
                DaemonMessage::InitProgress {
                    step,
                    step_num,
                    total_steps: _,
                    completed,
                } => {
                    current_step = (step_num - 1) as u16;
                    if completed {
                        screen.complete_step(step_base + current_step, &step)?;
                    } else {
                        screen.start_step(step_base + current_step, &step)?;
                    }
                }
                DaemonMessage::AgentProgress {
                    name,
                    is_supervisor,
                    progress,
                    ready,
                } => {
                    // Draw separator with AGENTS label before first agent
                    if agent_rows.is_empty() {
                        screen.draw_section_divider(separator_row, "AGENTS")?;
                    }

                    // Get or allocate row for this agent
                    let row = *agent_rows.entry(name.clone()).or_insert_with(|| {
                        let r = next_agent_row;
                        next_agent_row += 1;
                        r
                    });

                    if progress == 0.0 && !ready {
                        screen.start_agent(row, &name, is_supervisor)?;
                    } else if ready {
                        screen.complete_agent(row)?;
                    } else {
                        screen.update_agent_progress(row, progress)?;
                    }
                }
                DaemonMessage::InitComplete => {
                    // Show ready
                    let final_row = next_agent_row + 1;
                    screen.show_ready(final_row)?;

                    // Cleanup screen
                    screen.cleanup()?;
                    return Ok(());
                }
                DaemonMessage::Error { message, .. } => {
                    // Show error
                    screen.fail_step(step_base + current_step, "Error", &message)?;
                    screen.cleanup()?;
                    anyhow::bail!("Daemon initialization failed: {message}");
                }
                _ => {
                    // Ignore other messages during init
                }
            }
        }
    }
}

/// Connect to daemon socket with retry
fn connect_with_retry(
    sock_path: &std::path::Path,
    timeout: Duration,
) -> anyhow::Result<std::os::unix::net::UnixStream> {
    use std::os::unix::net::UnixStream;

    let start = std::time::Instant::now();
    let mut last_err = None;

    while start.elapsed() < timeout {
        match UnixStream::connect(sock_path) {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                last_err = Some(e);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    anyhow::bail!(
        "Failed to connect to daemon at {:?}: {}",
        sock_path,
        last_err.map(|e| e.to_string()).unwrap_or_default()
    )
}
