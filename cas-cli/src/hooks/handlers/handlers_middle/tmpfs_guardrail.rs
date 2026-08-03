use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::config::{DEFAULT_TMPFS_WARNING_THRESHOLD_BYTES, StagingConfig};
use crate::hooks::types::{HookInput, HookOutput};

const STATE_DIR: &str = "tmpfs_guardrail";
const STATE_LOCK_FILE: &str = "tmpfs_guardrail.lock";
const STATE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MEMORY_FS_TYPES: &[&str] = &["tmpfs", "ramfs"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MountInfo {
    pub mount_point: PathBuf,
    pub fs_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MountUsage {
    pub used_bytes: u64,
}

pub(crate) trait UsageReader {
    fn usage(&self, path: &Path) -> Option<MountUsage>;
}

#[derive(Debug, Default)]
struct SystemUsageReader;

impl UsageReader for SystemUsageReader {
    fn usage(&self, path: &Path) -> Option<MountUsage> {
        fs_usage(path)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GuardrailState {
    mounts: Vec<MountState>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MountState {
    mount_point: String,
    #[serde(default, alias = "cumulative_bytes")]
    written_bytes: u64,
    #[serde(default)]
    usage_growth_bytes: u64,
    baseline_used_bytes: Option<u64>,
    #[serde(default, alias = "last_warned_multiple")]
    last_warned_written_multiple: u64,
    #[serde(default)]
    last_warned_usage_multiple: u64,
    #[serde(default)]
    usage_growth_pending_confirmation: bool,
}

pub(crate) fn maybe_tmpfs_guardrail_warning(
    cas_root: &Path,
    input: &HookInput,
    config: &crate::config::Config,
) -> Option<HookOutput> {
    let staging = config.staging.clone().unwrap_or_default();
    let threshold = effective_threshold(&staging);
    let mounts = read_system_mounts()?;
    let usage = SystemUsageReader;
    maybe_tmpfs_guardrail_warning_with(
        cas_root,
        input,
        &mounts,
        &usage,
        threshold,
        staging.staging_dir.as_deref(),
    )
    .map(HookOutput::with_post_tool_context)
}

pub(crate) fn maybe_tmpfs_guardrail_warning_with(
    cas_root: &Path,
    input: &HookInput,
    mounts: &[MountInfo],
    usage_reader: &dyn UsageReader,
    threshold_bytes: u64,
    staging_dir: Option<&str>,
) -> Option<String> {
    let threshold_bytes = threshold_bytes.max(1);
    let tool_name = input.tool_name.as_deref()?;
    if !matches!(tool_name, "Write" | "Edit" | "Bash") {
        return None;
    }

    with_state_lock(cas_root, || {
        let mut state = load_state(cas_root, &input.session_id);
        let warning = match tool_name {
            "Write" | "Edit" => {
                let path = input
                    .tool_input
                    .as_ref()
                    .and_then(|tool_input| tool_input.get("file_path"))
                    .and_then(|file_path| file_path.as_str())?;
                let path = resolve_probe_path(path, &input.cwd);
                let mount = find_mount_for_path(&path, mounts)?;
                if !is_memory_fs(&mount.fs_type) {
                    return None;
                }

                let bytes = tool_write_size(input, &path)?;
                apply_delta(
                    &mut state,
                    &mount.mount_point,
                    bytes,
                    threshold_bytes,
                    staging_dir,
                )
            }
            "Bash" => apply_bash_usage_samples(
                &mut state,
                mounts,
                usage_reader,
                threshold_bytes,
                staging_dir,
            ),
            _ => None,
        };

        save_state(cas_root, &input.session_id, &state);
        warning
    })
}

fn effective_threshold(staging: &StagingConfig) -> u64 {
    if staging.tmpfs_warning_threshold_bytes == 0 {
        DEFAULT_TMPFS_WARNING_THRESHOLD_BYTES
    } else {
        staging.tmpfs_warning_threshold_bytes
    }
}

fn apply_delta(
    state: &mut GuardrailState,
    mount_point: &Path,
    delta_bytes: u64,
    threshold_bytes: u64,
    staging_dir: Option<&str>,
) -> Option<String> {
    if delta_bytes == 0 {
        return None;
    }

    let entry = state.entry_mut(mount_point);
    entry.written_bytes = entry.written_bytes.saturating_add(delta_bytes);
    let written_bytes = entry.written_bytes;
    warning_for_counter(
        &mut entry.last_warned_written_multiple,
        mount_point,
        written_bytes,
        threshold_bytes,
        staging_dir,
    )
}

fn apply_bash_usage_samples(
    state: &mut GuardrailState,
    mounts: &[MountInfo],
    usage_reader: &dyn UsageReader,
    threshold_bytes: u64,
    staging_dir: Option<&str>,
) -> Option<String> {
    let mut warning = None;
    for mount in mounts.iter().filter(|mount| is_memory_fs(&mount.fs_type)) {
        let usage = match usage_reader.usage(&mount.mount_point) {
            Some(usage) => usage,
            None => continue,
        };
        if let Some(candidate) = apply_usage_sample(
            state,
            &mount.mount_point,
            usage.used_bytes,
            threshold_bytes,
            staging_dir,
        ) {
            warning.get_or_insert(candidate);
        }
    }
    warning
}

fn apply_usage_sample(
    state: &mut GuardrailState,
    mount_point: &Path,
    used_bytes: u64,
    threshold_bytes: u64,
    staging_dir: Option<&str>,
) -> Option<String> {
    let entry = state.entry_mut(mount_point);
    let baseline = match entry.baseline_used_bytes {
        Some(baseline) => baseline,
        None => {
            entry.baseline_used_bytes = Some(used_bytes);
            entry.usage_growth_bytes = 0;
            return None;
        }
    };

    entry.usage_growth_bytes = used_bytes.saturating_sub(baseline);
    let usage_growth_bytes = entry.usage_growth_bytes;
    let multiple = usage_growth_bytes / threshold_bytes;
    if multiple == 0 || multiple <= entry.last_warned_usage_multiple {
        entry.usage_growth_pending_confirmation = false;
        return None;
    }

    // Bash can create large, short-lived temp trees. A single mount-wide peak
    // does not prove that an artifact was staged, so require the growth to
    // survive one subsequent sample. Direct Write/Edit calls remain immediate
    // because their target and byte count are known.
    if !entry.usage_growth_pending_confirmation {
        entry.usage_growth_pending_confirmation = true;
        return None;
    }

    entry.usage_growth_pending_confirmation = false;
    entry.last_warned_usage_multiple = multiple;
    Some(format_warning(
        mount_point,
        usage_growth_bytes,
        threshold_bytes,
        staging_dir,
    ))
}

fn warning_for_counter(
    last_warned_multiple: &mut u64,
    mount_point: &Path,
    total: u64,
    threshold_bytes: u64,
    staging_dir: Option<&str>,
) -> Option<String> {
    let multiple = total / threshold_bytes;
    if multiple == 0 || multiple <= *last_warned_multiple {
        return None;
    }
    *last_warned_multiple = multiple;

    Some(format_warning(
        mount_point,
        total,
        threshold_bytes,
        staging_dir,
    ))
}

fn format_warning(
    mount_point: &Path,
    total_bytes: u64,
    threshold_bytes: u64,
    staging_dir: Option<&str>,
) -> String {
    let alternative = staging_dir
        .filter(|dir| !dir.trim().is_empty())
        .map(|dir| {
            format!(
                "Restate the approved staging location `{}` before continuing large writes.",
                dir.trim()
            )
        })
        .unwrap_or_else(|| {
            "Restate a durable, disk-backed staging mount before continuing large writes."
                .to_string()
        });

    format!(
        "WARNING: This session has written or grown tmpfs/ramfs-backed storage at `{}` by {} bytes, crossing the configured threshold of {} bytes. tmpfs/ramfs consumes system memory and can wedge the host when large artifacts are staged there. {} This is a warning only; CAS did not deny the tool.",
        mount_point.display(),
        total_bytes,
        threshold_bytes,
        alternative
    )
}

impl GuardrailState {
    fn entry_mut(&mut self, mount_point: &Path) -> &mut MountState {
        let key = mount_point.to_string_lossy().to_string();
        if let Some(index) = self
            .mounts
            .iter()
            .position(|entry| entry.mount_point == key)
        {
            return &mut self.mounts[index];
        }
        self.mounts.push(MountState {
            mount_point: key,
            written_bytes: 0,
            usage_growth_bytes: 0,
            baseline_used_bytes: None,
            last_warned_written_multiple: 0,
            last_warned_usage_multiple: 0,
            usage_growth_pending_confirmation: false,
        });
        self.mounts.last_mut().expect("just pushed mount state")
    }
}

fn tool_write_size(input: &HookInput, path: &Path) -> Option<u64> {
    if let Some(tool_input) = &input.tool_input {
        let field = match input.tool_name.as_deref() {
            Some("Write") => "content",
            Some("Edit") => "new_string",
            _ => "",
        };
        if !field.is_empty() {
            if let Some(text) = tool_input.get(field).and_then(|value| value.as_str()) {
                // Hook payloads carry strings; count bytes, not chars, because
                // the guardrail threshold is configured in filesystem bytes.
                return Some(text.len() as u64);
            }
        }
    }

    fs::metadata(path).ok().map(|metadata| metadata.len())
}

fn resolve_probe_path(path: &str, cwd: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else if cwd.trim().is_empty() {
        path
    } else {
        PathBuf::from(cwd).join(path)
    }
}

pub(crate) fn parse_mounts(content: &str) -> Vec<MountInfo> {
    content
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _source = fields.next()?;
            let mount_point = decode_mount_field(fields.next()?);
            let fs_type = fields.next()?.to_string();
            Some(MountInfo {
                mount_point: PathBuf::from(mount_point),
                fs_type,
            })
        })
        .collect()
}

pub(crate) fn find_mount_for_path<'a>(
    path: &Path,
    mounts: &'a [MountInfo],
) -> Option<&'a MountInfo> {
    mounts
        .iter()
        .filter(|mount| path.starts_with(&mount.mount_point))
        .max_by_key(|mount| mount.mount_point.as_os_str().len())
}

fn is_memory_fs(fs_type: &str) -> bool {
    MEMORY_FS_TYPES
        .iter()
        .any(|memory_fs| fs_type.eq_ignore_ascii_case(memory_fs))
}

fn read_system_mounts() -> Option<Vec<MountInfo>> {
    fs::read_to_string("/proc/mounts")
        .ok()
        .map(|content| parse_mounts(&content))
}

fn decode_mount_field(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn state_path(cas_root: &Path, session_id: &str) -> PathBuf {
    let safe_session = session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let safe_session = if safe_session.is_empty() {
        "unknown".to_string()
    } else {
        safe_session
    };
    cas_root
        .join(STATE_DIR)
        .join(format!("{safe_session}.json"))
}

fn with_state_lock<T>(cas_root: &Path, f: impl FnOnce() -> T) -> T {
    use fs2::FileExt;

    struct LockGuard(std::fs::File, bool);
    impl Drop for LockGuard {
        fn drop(&mut self) {
            if self.1 {
                let _ = FileExt::unlock(&self.0);
            }
        }
    }

    let lock_path = cas_root.join(STATE_LOCK_FILE);
    let _guard = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .ok()
        .map(|file| {
            let locked = file.lock_exclusive().is_ok();
            LockGuard(file, locked)
        });
    f()
}

fn load_state(cas_root: &Path, session_id: &str) -> GuardrailState {
    let path = state_path(cas_root, session_id);
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_state(cas_root: &Path, session_id: &str, state: &GuardrailState) {
    let path = state_path(cas_root, session_id);
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    prune_old_state_files(parent, STATE_RETENTION);
    let Ok(content) = serde_json::to_string(state) else {
        return;
    };
    let tmp_path = path.with_extension("json.tmp");
    if fs::write(&tmp_path, content).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

fn prune_old_state_files(dir: &Path, retention: Duration) {
    let now = SystemTime::now();
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age > retention {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(unix)]
fn fs_usage(path: &Path) -> Option<MountUsage> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    let block_size = stat.f_frsize as u64;
    let used_blocks = (stat.f_blocks as u64).saturating_sub(stat.f_bfree as u64);
    Some(MountUsage {
        used_bytes: used_blocks.saturating_mul(block_size),
    })
}

#[cfg(not(unix))]
fn fs_usage(_path: &Path) -> Option<MountUsage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_lock_runs_closure_and_creates_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let called = std::cell::Cell::new(false);

        with_state_lock(tmp.path(), || called.set(true));

        assert!(called.get());
        assert!(tmp.path().join(STATE_LOCK_FILE).exists());
    }

    #[test]
    fn prune_old_state_files_removes_expired_json_only() {
        let tmp = tempfile::tempdir().unwrap();
        let stale = tmp.path().join("old.json");
        let lock = tmp.path().join(STATE_LOCK_FILE);
        std::fs::write(&stale, "{}").unwrap();
        std::fs::write(&lock, "").unwrap();

        std::thread::sleep(Duration::from_millis(2));
        prune_old_state_files(tmp.path(), Duration::ZERO);

        assert!(!stale.exists(), "expired state json should be pruned");
        assert!(lock.exists(), "non-json lock sentinel must be retained");
    }

    #[cfg(unix)]
    #[test]
    fn fs_usage_reports_used_bytes_for_a_real_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        let usage = fs_usage(temp.path()).expect("statvfs should inspect a temp directory");
        assert!(
            usage.used_bytes < u64::MAX,
            "widened, saturating statvfs arithmetic must stay in byte range"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fs_usage_preserves_none_for_nul_path() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/tmp/bad\0path"));
        assert_eq!(fs_usage(path), None);
    }
}
