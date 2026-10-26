use std::path::{Path, PathBuf};
use std::process::Command;

/// Map every supported Cargo target to an explicit Zig baseline.
///
/// Native targets are intentionally mapped too: omitting `-Dtarget` makes Zig
/// optimize for the build host and can leak AVX-512 into distributed binaries.
/// Unknown targets fail closed so a distributable artifact can never silently
/// inherit the build host's instruction set.
pub fn rust_target_to_zig(rust_target: &str) -> Result<&'static str, String> {
    match rust_target {
        "x86_64-unknown-linux-gnu" => Ok("x86_64-linux-gnu"),
        "aarch64-unknown-linux-gnu" => Ok("aarch64-linux-gnu"),
        "x86_64-unknown-linux-musl" => Ok("x86_64-linux-musl"),
        "aarch64-unknown-linux-musl" => Ok("aarch64-linux-musl"),
        "x86_64-apple-darwin" => Ok("x86_64-macos"),
        "aarch64-apple-darwin" => Ok("aarch64-macos"),
        _ => Err(format!(
            "unsupported Cargo target `{rust_target}` for ghostty_vt_sys; refusing to build without an explicit portable Zig target. Supported targets: x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, x86_64-unknown-linux-musl, aarch64-unknown-linux-musl, x86_64-apple-darwin, aarch64-apple-darwin"
        )),
    }
}

/// Find the pinned Zig compiler, including the main checkout of a linked worktree.
/// Explicit inputs let callers use their environment without mutating process state.
pub fn find_zig(
    workspace_root: &Path,
    configured: Option<&std::ffi::OsStr>,
    search_path: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, String> {
    // The Zig package is the version authority; bootstrap-zig.sh installs this pin.
    let required = include_str!("zig/build.zig.zon")
        .split_once(".minimum_zig_version = \"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(version, _)| version)
        .expect("Zig package must declare minimum_zig_version");
    let mut tried = Vec::new();
    let mut probe = |candidate: PathBuf| {
        // Cargo invokes build scripts from the crate, not the workspace root.
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            workspace_root.join(candidate)
        };
        let reason = if !candidate.is_file() {
            "missing or not a file".to_string()
        } else {
            match Command::new(&candidate)
                .current_dir(workspace_root)
                .arg("version")
                .output()
            {
                Ok(output) if output.status.success() => {
                    let version = String::from_utf8_lossy(&output.stdout);
                    if version.trim() == required {
                        return Some(candidate);
                    }
                    format!("version {}, expected {required}", version.trim())
                }
                Ok(output) => format!("version command failed: {}", output.status),
                Err(error) => format!("cannot execute: {error}"),
            }
        };
        tried.push(format!("{}: {reason}", candidate.display()));
        None
    };

    // Keep the same precedence in scripts/release_env.py's release preflight.
    if let Some(zig) = configured.filter(|value| !value.is_empty())
        && let Some(zig) = probe(PathBuf::from(zig))
    {
        return Ok(zig);
    }
    if let Some(path) = search_path {
        for dir in std::env::split_paths(path) {
            if let Some(zig) = probe(dir.join("zig")) {
                return Ok(zig);
            }
        }
    }
    if let Some(zig) = probe(workspace_root.join(".context/zig/zig")) {
        return Ok(zig);
    }

    // .context is ignored, so new linked worktrees share the main checkout's
    // installation via the common Git directory. Git may return a relative path.
    let common = Command::new("git")
        .current_dir(workspace_root)
        .args(["rev-parse", "--git-common-dir"])
        .output();
    match common {
        Ok(output) if output.status.success() => {
            let common = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
            let common = if common.is_absolute() {
                common
            } else {
                workspace_root.join(common)
            };
            if let Some(main) = common.parent()
                && let Some(zig) = probe(main.join(".context/zig/zig"))
            {
                return Ok(zig);
            }
        }
        Ok(output) => tried.push(format!(
            "git rev-parse --git-common-dir: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => tried.push(format!("git rev-parse --git-common-dir: {error}")),
    }
    Err(format!(
        "Zig compiler {required} not found.\nZIG: {}; PATH: {}\nTried:\n  - {}\nInstall with: ./scripts/bootstrap-zig.sh or set ZIG to a compatible executable.",
        configured.map_or_else(|| "(unset)".into(), |value| value.to_string_lossy()),
        search_path.map_or_else(|| "(unset)".into(), |value| value.to_string_lossy()),
        tried.join("\n  - "),
    ))
}
