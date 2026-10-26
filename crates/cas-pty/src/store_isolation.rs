//! Hide the operator's Neon and Vercel credential stores from factory workers
//! (GH #1293, cas-5b3d).
//!
//! Workers share the operator's uid and `$HOME`, so command parsing in the
//! PreToolUse hook cannot stop every reader (interpreter one-liners, piped
//! shells, variable indirection). On Linux a worker instead starts inside a
//! bubblewrap mount namespace that shows the host filesystem unchanged except
//! that each non-empty store directory is covered by an empty read-only
//! tmpfs. PIDs, environment and working directory are unchanged.
//!
//! Policy:
//! - Only `CAS_AGENT_ROLE=worker` launches are wrapped, and only when a
//!   non-empty store exists; with nothing to hide the launch is unchanged.
//! - When a store exists and the namespace cannot be created, spawn fails
//!   with a remedy rather than falling back.
//! - `CAS_WORKER_STORE_ISOLATION=off` in the daemon environment opts out.
//! - macOS has no backend yet: the launch proceeds with a warning and the
//!   hook remains the only control (documented gap).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Daemon-environment switch that disables masking (`off`).
pub const STORE_ISOLATION_ENV: &str = "CAS_WORKER_STORE_ISOLATION";

/// Home-relative locations of the operator's Neon and Vercel credential
/// stores (Linux XDG defaults and the macOS Vercel location).
pub const HOME_STORE_SUFFIXES: &[&str] = &[
    ".config/neonctl",
    ".local/share/com.vercel.cli",
    "Library/Application Support/com.vercel.cli",
    ".config/com.vercel.cli",
];

/// Every credential store root implied by one environment: the home-relative
/// defaults, the XDG overrides and `VERCEL_CONFIG_DIR`.
pub fn operator_credential_stores(lookup: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let home = lookup("HOME").map(PathBuf::from);
    let config_home = lookup("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    let data_home = lookup("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    let mut roots = Vec::new();
    if let Some(home) = &home {
        roots.extend(HOME_STORE_SUFFIXES.iter().map(|suffix| home.join(suffix)));
    }
    if let Some(config_home) = config_home {
        roots.push(config_home.join("neonctl"));
        roots.push(config_home.join("com.vercel.cli"));
    }
    if let Some(data_home) = data_home {
        roots.push(data_home.join("com.vercel.cli"));
    }
    if let Some(vercel) = lookup("VERCEL_CONFIG_DIR") {
        roots.push(PathBuf::from(vercel));
    }
    let mut unique = Vec::new();
    for root in roots {
        if !unique.contains(&root) {
            unique.push(root);
        }
    }
    unique
}

/// The program and arguments that actually start a worker: `None` keeps the
/// configured command, `Some` is the masking wrapper, `Err` refuses the spawn.
pub(crate) fn worker_launch(
    command: &str,
    args: &[String],
    worker_env: &[(String, String)],
) -> Result<Option<(String, Vec<String>)>, String> {
    let worker_value = |name: &str| {
        worker_env
            .iter()
            .rev()
            .find_map(|(key, value)| (key == name).then(|| OsString::from(value)))
    };
    if worker_value("CAS_AGENT_ROLE").as_deref() != Some("worker".as_ref()) {
        return Ok(None);
    }
    // Both the daemon's and the worker's view of HOME/XDG name stores.
    let mut stores = operator_credential_stores(|name| std::env::var_os(name));
    for store in
        operator_credential_stores(|name| worker_value(name).or_else(|| std::env::var_os(name)))
    {
        if !stores.contains(&store) {
            stores.push(store);
        }
    }
    let stores: Vec<PathBuf> = stores
        .into_iter()
        .filter(|path| has_content(path))
        .collect();
    if stores.is_empty() {
        return Ok(None);
    }
    if std::env::var(STORE_ISOLATION_ENV).is_ok_and(|value| value.eq_ignore_ascii_case("off")) {
        tracing::warn!(
            stores = stores.len(),
            "factory worker starts with operator credential stores visible: {STORE_ISOLATION_ENV}=off"
        );
        return Ok(None);
    }
    if !cfg!(target_os = "linux") {
        tracing::warn!(
            stores = stores.len(),
            "factory worker credential stores cannot be masked on this platform; \
             only the PreToolUse hook guards them (cas-5b3d macOS gap)"
        );
        return Ok(None);
    }
    isolation_available()?;
    Ok(Some((
        "bwrap".to_string(),
        bwrap_args(&stores, command, args),
    )))
}

/// A store with anything in it. An absent path or an empty directory (for
/// example an already-masked view inside a worker) has nothing to hide.
fn has_content(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::read_dir(path)
            .map(|mut entries| entries.next().is_some())
            // Unreadable but present: mask it anyway.
            .unwrap_or(true),
        Ok(_) => true,
        Err(_) => false,
    }
}

fn bwrap_args(stores: &[PathBuf], command: &str, args: &[String]) -> Vec<String> {
    let mut wrapper: Vec<String> = [
        "--dev-bind",
        "/",
        "/",
        "--unshare-user",
        "--die-with-parent",
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();
    for store in stores {
        let store = store.to_string_lossy().into_owned();
        if Path::new(&store).is_dir() {
            wrapper.extend([
                "--tmpfs".into(),
                store.clone(),
                "--remount-ro".into(),
                store,
            ]);
        } else {
            wrapper.extend(["--ro-bind".into(), "/dev/null".into(), store]);
        }
    }
    if let Some(copy) = user_owned_ssh_config_dir() {
        wrapper.extend([
            "--ro-bind".into(),
            copy.to_string_lossy().into_owned(),
            SSH_CONFIG_DIR.into(),
        ]);
    }
    wrapper.push("--".into());
    wrapper.push(command.to_string());
    wrapper.extend(args.iter().cloned());
    wrapper
}

/// Whether this host can create the worker mount namespace; probed once.
pub(crate) fn isolation_available() -> Result<(), String> {
    static PROBE: OnceLock<Result<(), String>> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            let status = std::process::Command::new("bwrap")
                .args(["--dev-bind", "/", "/", "--unshare-user", "--", "true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            match status {
                Ok(status) if status.success() => Ok(()),
                outcome => Err(format!(
                    "factory worker cannot start: the operator's Neon/Vercel credential stores \
                     must be hidden from workers (GH #1293), but bubblewrap could not create a \
                     user mount namespace ({}). Install bubblewrap (`bwrap`) and allow \
                     unprivileged user namespaces, or set {STORE_ISOLATION_ENV}=off in the \
                     factory daemon environment to accept the exposure.",
                    match outcome {
                        Ok(status) => format!("probe exited with {status}"),
                        Err(error) => format!("bwrap: {error}"),
                    }
                )),
            }
        })
        .clone()
}

const SSH_CONFIG_DIR: &str = "/etc/ssh/ssh_config.d";

/// Inside the user namespace root-owned files appear owned by `nobody`, and
/// OpenSSH refuses included config files with that owner, which breaks
/// `git push` over SSH. Bind a user-owned copy of the include directory.
/// The copy is content-addressed, so concurrent spawns share it safely.
fn user_owned_ssh_config_dir() -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let mut files: Vec<(OsString, Vec<u8>)> = std::fs::read_dir(SSH_CONFIG_DIR)
        .ok()?
        .filter_map(|entry| {
            // Follow symlinks: distributions link drop-ins from /usr/lib.
            let entry = entry.ok()?;
            let path = entry.path();
            std::fs::metadata(&path).ok()?.is_file().then_some(())?;
            Some((entry.file_name(), std::fs::read(path).ok()?))
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    files.sort();
    let mut digest = Sha256::new();
    for (name, contents) in &files {
        digest.update(name.as_encoded_bytes());
        digest.update([0]);
        digest.update(contents);
        digest.update([0]);
    }
    let hash: String = digest.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let copy = std::env::temp_dir().join(format!("cas-worker-ssh-config.d-{hash}"));
    if copy.is_dir() {
        return Some(copy);
    }
    let staging = std::env::temp_dir().join(format!(
        "cas-worker-ssh-config.d-{hash}.{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&staging).ok()?;
    for (name, contents) in &files {
        std::fs::write(staging.join(name), contents).ok()?;
    }
    if std::fs::rename(&staging, &copy).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    copy.is_dir().then_some(copy)
}
