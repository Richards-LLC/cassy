//! Artifact collection for test failures
//!
//! Captures and persists debugging artifacts when tests fail:
//! - Screen snapshots (YAML)
//! - Frame history
//! - Raw PTY logs
//! - Rendered terminal output (ASCII)

use crate::screen::{Frame, FrameHistory, ScreenBuffer, Snapshot, SnapshotMetadata};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

/// Configuration for artifact collection
#[derive(Clone, Debug)]
pub struct ArtifactConfig {
    /// Base directory for artifacts (default: "test-artifacts")
    pub base_dir: PathBuf,
    /// Whether to capture raw PTY output
    pub capture_pty_log: bool,
    /// Whether to render ASCII screenshots
    pub render_screenshots: bool,
    /// Whether to save frame history
    pub save_frame_history: bool,
    /// Maximum PTY log size in bytes (default: 1MB)
    pub max_pty_log_size: usize,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("test-artifacts"),
            capture_pty_log: true,
            render_screenshots: true,
            save_frame_history: true,
            max_pty_log_size: 1024 * 1024, // 1MB
        }
    }
}

impl ArtifactConfig {
    /// Create config with custom base directory
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: dir.into(),
            ..Default::default()
        }
    }

    /// Set base directory from environment variable if present
    pub fn from_env() -> Self {
        let base_dir = std::env::var("TUI_TEST_ARTIFACT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("test-artifacts"));
        Self {
            base_dir,
            ..Default::default()
        }
    }
}

/// Collects and persists artifacts for test debugging
#[derive(Debug)]
pub struct ArtifactCollector {
    config: ArtifactConfig,
    /// Raw PTY output buffer
    pty_log: Vec<u8>,
    /// Frame history
    frames: FrameHistory,
    /// Test name for directory organization
    test_name: String,
}

impl ArtifactCollector {
    /// Create a new artifact collector
    pub fn new(test_name: impl Into<String>) -> Self {
        Self::with_config(test_name, ArtifactConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(test_name: impl Into<String>, config: ArtifactConfig) -> Self {
        Self {
            config,
            pty_log: Vec::new(),
            frames: FrameHistory::new(20),
            test_name: test_name.into(),
        }
    }

    /// Record PTY output
    pub fn record_pty_output(&mut self, data: &[u8]) {
        if self.config.capture_pty_log {
            let remaining = self
                .config
                .max_pty_log_size
                .saturating_sub(self.pty_log.len());
            let to_add = data.len().min(remaining);
            self.pty_log.extend_from_slice(&data[..to_add]);
        }
    }

    /// Record a frame
    pub fn record_frame(&mut self, buffer: ScreenBuffer, raw_output: Vec<u8>) {
        if self.config.save_frame_history {
            self.frames.push(Frame::new(buffer, raw_output));
        }
    }

    /// Get the artifact directory path for this test
    pub fn artifact_dir(&self) -> PathBuf {
        self.config.base_dir.join(&self.test_name)
    }

    /// Persist all collected artifacts to disk
    ///
    /// Creates a directory structure like:
    /// ```text
    /// test-artifacts/
    ///   my_test_name/
    ///     snapshot.yaml       # Final screen state
    ///     pty.log            # Raw PTY output
    ///     screenshot.txt     # Rendered ASCII
    ///     frames/
    ///       frame_000.yaml   # Frame history
    ///       frame_001.yaml
    ///       ...
    /// ```
    pub fn persist(&self, final_buffer: &ScreenBuffer) -> io::Result<ArtifactPaths> {
        let dir = self.artifact_dir();
        fs::create_dir_all(&dir)?;

        let mut paths = ArtifactPaths {
            base_dir: dir.clone(),
            snapshot: None,
            pty_log: None,
            screenshot: None,
            frames: Vec::new(),
        };

        // Save final snapshot
        let snapshot_path = dir.join("snapshot.yaml");
        let metadata = SnapshotMetadata {
            created_at: Some(iso8601_now()),
            framework_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        };
        let snapshot = Snapshot::from_buffer_with_metadata(&self.test_name, final_buffer, metadata);
        snapshot.save(&snapshot_path)?;
        paths.snapshot = Some(snapshot_path);

        // Save PTY log
        if self.config.capture_pty_log && !self.pty_log.is_empty() {
            let log_path = dir.join("pty.log");
            fs::write(&log_path, &self.pty_log)?;
            paths.pty_log = Some(log_path);
        }

        // Render and save ASCII screenshot
        if self.config.render_screenshots {
            let screenshot_path = dir.join("screenshot.txt");
            let screenshot = render_buffer_ascii(final_buffer);
            fs::write(&screenshot_path, screenshot)?;
            paths.screenshot = Some(screenshot_path);
        }

        // Save frame history
        if self.config.save_frame_history && !self.frames.is_empty() {
            let frames_dir = dir.join("frames");
            fs::create_dir_all(&frames_dir)?;

            for (i, frame) in self.frames.iter().enumerate() {
                let frame_path = frames_dir.join(format!("frame_{i:03}.yaml"));
                let snapshot = frame.to_snapshot(format!("frame_{i}"));
                snapshot.save(&frame_path)?;
                paths.frames.push(frame_path);
            }
        }

        Ok(paths)
    }

    /// Persist artifacts and return a formatted failure message
    pub fn persist_with_message(&self, final_buffer: &ScreenBuffer) -> String {
        match self.persist(final_buffer) {
            Ok(paths) => {
                let mut msg = String::new();
                msg.push_str("Test artifacts saved:\n");
                msg.push_str(&format!("  Directory: {}\n", paths.base_dir.display()));
                if let Some(ref p) = paths.snapshot {
                    msg.push_str(&format!("  Snapshot: {}\n", p.display()));
                }
                if let Some(ref p) = paths.pty_log {
                    msg.push_str(&format!("  PTY Log: {}\n", p.display()));
                }
                if let Some(ref p) = paths.screenshot {
                    msg.push_str(&format!("  Screenshot: {}\n", p.display()));
                }
                if !paths.frames.is_empty() {
                    msg.push_str(&format!("  Frames: {} saved\n", paths.frames.len()));
                }
                msg
            }
            Err(e) => format!("Failed to save artifacts: {e}\n"),
        }
    }

    /// Get the frame history
    pub fn frames(&self) -> &FrameHistory {
        &self.frames
    }

    /// Get the PTY log
    pub fn pty_log(&self) -> &[u8] {
        &self.pty_log
    }

    /// Clear all collected artifacts
    pub fn clear(&mut self) {
        self.pty_log.clear();
        self.frames.clear();
    }
}

/// Paths to persisted artifacts
#[derive(Clone, Debug)]
pub struct ArtifactPaths {
    /// Base directory
    pub base_dir: PathBuf,
    /// Path to snapshot YAML
    pub snapshot: Option<PathBuf>,
    /// Path to PTY log
    pub pty_log: Option<PathBuf>,
    /// Path to ASCII screenshot
    pub screenshot: Option<PathBuf>,
    /// Paths to frame snapshots
    pub frames: Vec<PathBuf>,
}

/// Render a screen buffer to ASCII text with box drawing
pub fn render_buffer_ascii(buffer: &ScreenBuffer) -> String {
    let size = buffer.size();
    let mut output = String::new();

    // Top border
    output.push('┌');
    for _ in 0..size.cols {
        output.push('─');
    }
    output.push_str("┐\n");

    // Content rows
    for row in 0..size.rows {
        output.push('│');
        let line = buffer.row_text(row);
        output.push_str(&line);
        // Pad to full width
        for _ in line.chars().count()..(size.cols as usize) {
            output.push(' ');
        }
        output.push_str("│\n");
    }

    // Bottom border
    output.push('└');
    for _ in 0..size.cols {
        output.push('─');
    }
    output.push_str("┘\n");

    // Cursor info
    let cursor = buffer.cursor();
    output.push_str(&format!(
        "Cursor: ({}, {}) {}\n",
        cursor.row,
        cursor.col,
        if buffer.cursor_visible() {
            "visible"
        } else {
            "hidden"
        }
    ));

    output
}

/// Get current time as ISO8601 string
fn iso8601_now() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Simple ISO8601 formatting without external deps
    let secs_per_day = 86400u64;
    let secs_per_hour = 3600u64;
    let secs_per_min = 60u64;

    let days = now / secs_per_day;
    let remaining = now % secs_per_day;
    let hours = remaining / secs_per_hour;
    let remaining = remaining % secs_per_hour;
    let minutes = remaining / secs_per_min;
    let seconds = remaining % secs_per_min;

    // Calculate year/month/day from days since epoch (1970-01-01)
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since epoch to year/month/day
fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    let mut remaining = days as i64;
    let mut year = 1970i32;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let days_in_months: [i64; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u32;
    for days_in_month in days_in_months {
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        month += 1;
    }

    (year as u32, month, remaining as u32 + 1)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use crate::artifact::collector::*;
    use tempfile::TempDir;

    fn make_buffer_with_text(text: &str) -> ScreenBuffer {
        let mut buffer = ScreenBuffer::new(40, 10);
        buffer.put_str(text);
        buffer
    }

    #[test]
    fn test_artifact_collector_new() {
        let collector = ArtifactCollector::new("my_test");
        assert!(collector.pty_log().is_empty());
        assert!(collector.frames().is_empty());
    }

    #[test]
    fn test_record_pty_output() {
        let mut collector = ArtifactCollector::new("test");
        collector.record_pty_output(b"Hello ");
        collector.record_pty_output(b"World");
        assert_eq!(collector.pty_log(), b"Hello World");
    }

    #[test]
    fn test_record_frame() {
        let mut collector = ArtifactCollector::new("test");
        let buffer = make_buffer_with_text("Frame 1");
        collector.record_frame(buffer, vec![0x1b, 0x5b]);
        assert_eq!(collector.frames().len(), 1);
    }

    #[test]
    fn test_persist_artifacts() {
        let temp_dir = TempDir::new().unwrap();
        let config = ArtifactConfig::with_dir(temp_dir.path());
        let mut collector = ArtifactCollector::with_config("test_persist", config);

        collector.record_pty_output(b"test output");
        let buffer = make_buffer_with_text("Final state");
        collector.record_frame(buffer.clone(), vec![]);

        let paths = collector.persist(&buffer).unwrap();

        assert!(paths.snapshot.unwrap().exists());
        assert!(paths.pty_log.unwrap().exists());
        assert!(paths.screenshot.unwrap().exists());
        assert_eq!(paths.frames.len(), 1);
        assert!(paths.frames[0].exists());
    }

    #[test]
    fn test_render_buffer_ascii() {
        let mut buffer = ScreenBuffer::new(10, 3);
        buffer.put_str("Hello");
        buffer.move_cursor_to(1, 0);
        buffer.put_str("World");

        let rendered = render_buffer_ascii(&buffer);
        assert!(rendered.contains("┌"));
        assert!(rendered.contains("Hello"));
        assert!(rendered.contains("World"));

    }

    #[test]
    fn test_iso8601_now() {
        let ts = iso8601_now();
        // Should match pattern like "2026-01-31T10:30:00Z"
        assert!(ts.contains("T"));
        assert!(ts.ends_with("Z"));
        assert_eq!(ts.len(), 20);
    }

    #[test]
    fn test_max_pty_log_size() {
        let config = ArtifactConfig {
            max_pty_log_size: 10,
            ..Default::default()
        };
        let mut collector = ArtifactCollector::with_config("test", config);

        collector.record_pty_output(b"12345678901234567890"); // 20 bytes
        assert_eq!(collector.pty_log().len(), 10); // Capped at 10
    }
}
