//! Claude worker OS sandbox profile (cas-3751a, capability guards stage A).
//!
//! A Claude worker's Bash tool runs inside Claude Code's sandbox (Seatbelt on
//! macOS). Cassy writes the `sandbox` settings block in two halves:
//! [`claude_sandbox_block`] from the project's resolved roots and the worker's
//! current tasks when the worker settings file is written (each spawn or
//! recycle), and [`apply_worktree`] from the worker checkout at launch. The
//! profile grants only the current tasks' artifact directories and the
//! checkout's own scratch, so a worker whose assignment changes is relaunched
//! (cas-f157). Git paths are exact. The shared `.git` directory itself is never
//! writable: allowlisting it re-opens `.git/config` and `.git/hooks`, which the
//! sandbox's built-in protection only covers for repositories under the
//! working directory (cas-ac77, measured in a throwaway repository).

use cas_types::factory_scratch::{checkout_tmp_dir, worker_target_dir};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Harness environment marker the PreToolUse hook reads. Hooks inherit the
/// harness process environment, never the sandboxed shell's.
pub const SANDBOX_ENV: &str = "CAS_WORKER_SANDBOX";
/// Stage A: the sandbox enforces and the replaced text guards only log.
pub const SHADOW: &str = "shadow";
/// Harness environment listing the tasks whose artifact directories the
/// launched profile grants, comma-separated; empty when it grants none.
pub const TASKS_ENV: &str = "CAS_WORKER_SANDBOX_TASKS";
/// Top-level worker settings key carrying [`LaunchInputs`]. The launch
/// removes it before Claude reads the settings.
pub const LAUNCH_KEY: &str = "casWorkerLaunch";

/// Hosts a worker shell reaches: GitHub, crates.io and the npm registry.
pub const ALLOWED_DOMAINS: [&str; 9] = [
    "github.com",
    "*.github.com",
    "api.github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "crates.io",
    "index.crates.io",
    "static.crates.io",
    "registry.npmjs.org",
];

/// Cassy runtime directories the capped runner writes under `CAS_ROOT`.
pub const CAS_ROOT_RUNNER_DIRS: [&str; 3] =
    ["worker-check-slots", "worker-target-owners", "worker-checks"];

/// Secret files a worker cannot write in its checkout: exact names, and
/// names containing a key or credential file name. File extensions such as
/// `.pem` are not here: package trees ship certificates, so their text guard
/// keeps refusing instead (H-33 is shadowed only for these names).
pub const SECRET_FILE_NAMES: [&str; 1] = [".env"];
pub const SECRET_NAME_FRAGMENTS: [&str; 5] =
    ["id_rsa", "id_ed25519", "credentials.json", "secrets.yaml", "secrets.json"];

/// Login-persistence directories under the home directory, denied whatever
/// the operator's `write_roots` say: anything written there runs at the next
/// login, outside the sandbox (GH #2324).
pub const PERSISTENCE_DIRS: [&str; 1] = ["Library/LaunchAgents"];

fn persistence_denies() -> Vec<String> {
    dirs::home_dir()
        .map(|home| PERSISTENCE_DIRS.iter().map(|dir| home.join(dir).display().to_string()).collect())
        .unwrap_or_default()
}

/// Whether the generated profile denies writing a file with this name.
pub fn secret_write_denied(file_name: &str) -> bool {
    SECRET_FILE_NAMES.contains(&file_name)
        || SECRET_NAME_FRAGMENTS.iter().any(|fragment| file_name.contains(fragment))
}

/// Cassy's inputs for the launch half of the profile, which knows the checkout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchInputs {
    /// The tasks whose artifact directories the static half grants; `None`
    /// when Cassy could not read the worker's assignment, so no start is
    /// compared against it.
    pub tasks: Option<Vec<String>>,
    /// Root of every checkout's private temporary directory.
    pub tmp_root: Option<PathBuf>,
    /// Configured `factory.scratch_root`, which relocates a managed
    /// checkout's Cargo target.
    pub scratch_root: Option<PathBuf>,
}

impl LaunchInputs {
    /// The checkout's own scratch directories: its private temporary
    /// directory and a relocated Cargo target. Never a scratch root itself.
    /// Their parents are created first, as the capped runner would: inside
    /// the sandbox only the directory itself is writable, so a missing parent
    /// could never be created.
    pub fn checkout_scratch(&self, worktree: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(root) = &self.tmp_root {
            dirs.push(checkout_tmp_dir(root, worktree));
        }
        let target = worker_target_dir(self.scratch_root.as_deref(), worktree);
        if !target.starts_with(worktree) {
            dirs.push(target);
        }
        for parent in dirs.iter().filter_map(|dir| dir.parent()) {
            if let Err(error) = std::fs::create_dir_all(parent) {
                tracing::warn!(path = %parent.display(), %error, "worker sandbox: could not pre-create a scratch parent");
            }
        }
        dirs.iter().map(|dir| canonical_spelling(dir)).collect()
    }
}

/// Seatbelt matches the kernel's canonical path, and sandbox-runtime keeps an
/// absent path as spelled, so a grant for a directory created later must
/// already be canonical (macOS `/tmp` is `/private/tmp`). Resolves the
/// deepest existing ancestor and keeps the rest.
pub fn canonical_spelling(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(real) = ancestor.canonicalize() {
            let rest = path.strip_prefix(ancestor).unwrap_or(Path::new(""));
            return if rest.as_os_str().is_empty() { real } else { real.join(rest) };
        }
    }
    path.to_path_buf()
}

/// Inputs resolved by Cassy for the static half of the profile.
pub struct ProfileRoots<'a> {
    /// Effective operator write roots and the current tasks' artifact dirs.
    pub write_roots: &'a [PathBuf],
    /// The project store (`CAS_ROOT`).
    pub cas_root: &'a Path,
}

/// The worker `sandbox` settings block before the checkout is known.
pub fn claude_sandbox_block(roots: &ProfileRoots<'_>) -> Value {
    let mut allow: Vec<String> = Vec::new();
    let mut push = |path: &Path| {
        let path = path.display().to_string();
        if !allow.contains(&path) {
            allow.push(path);
        }
    };
    for root in roots.write_roots {
        push(root);
    }
    for dir in CAS_ROOT_RUNNER_DIRS {
        push(&roots.cas_root.join(dir));
    }
    json!({
        "enabled": true,
        "enabledPlatforms": ["macos"],
        "autoAllowBashIfSandboxed": true,
        "allowUnsandboxedCommands": false,
        // `gh` and other Go tools verify TLS through trustd (cas-ac77).
        "enableWeakerNetworkIsolation": true,
        "network": { "allowedDomains": ALLOWED_DOMAINS },
        "filesystem": { "allowWrite": allow, "denyWrite": persistence_denies() },
    })
}

fn secret_denies(worktree: &Path) -> Vec<PathBuf> {
    // Each glob is written at the checkout root and below it, so a match does
    // not rely on `**/` also matching zero directories.
    let mut globs = Vec::new();
    for dir in [worktree.to_path_buf(), worktree.join("**")] {
        globs.extend(SECRET_FILE_NAMES.iter().map(|name| dir.join(name)));
        globs.extend(SECRET_NAME_FRAGMENTS.iter().map(|fragment| dir.join(format!("*{fragment}*"))));
    }
    globs
}

/// Exact git paths a linked worktree needs to commit, branch `factory/*` and
/// fetch, and the git paths that must stay read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPaths {
    pub allow: Vec<PathBuf>,
    pub deny: Vec<PathBuf>,
    /// Directories created before launch so a write inside them stays covered.
    pub create: Vec<PathBuf>,
}

/// Resolve the git layout of `worktree`. `None` when it is not a git checkout.
pub fn git_paths(worktree: &Path) -> Option<GitPaths> {
    let output = Command::new("git")
        .current_dir(worktree)
        .args(["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines().map(PathBuf::from);
    let (git_dir, common) = (lines.next()?, lines.next()?);
    Some(git_paths_for(&git_dir, &common))
}

/// Pure layout rules, tested without a repository.
pub fn git_paths_for(git_dir: &Path, common: &Path) -> GitPaths {
    let shared = [
        "objects",
        "refs/heads/factory",
        "logs/refs/heads/factory",
        "refs/remotes",
        "logs/refs/remotes",
    ];
    let mut allow = Vec::new();
    // A linked worktree's private dir holds HEAD, index and FETCH_HEAD. A
    // primary checkout's git dir is the common dir and is never allowlisted.
    if git_dir != common {
        allow.push(git_dir.to_path_buf());
    }
    let create: Vec<PathBuf> = shared.iter().map(|rel| common.join(rel)).collect();
    allow.extend(create.iter().cloned());
    let mut deny = vec![common.join("config"), common.join("hooks")];
    if git_dir != common {
        // Worktree config can set core.hooksPath or fsmonitor; the Cassy push
        // guard hooks live beside it.
        deny.push(git_dir.join("config.worktree"));
        deny.push(git_dir.join("hooks-cas-guard"));
    }
    GitPaths { allow, deny, create }
}

/// Add the worker checkout's paths to a `sandbox` block: the checkout, its
/// own scratch directories, its exact git paths, and denies for Claude
/// settings a worker could otherwise widen its own profile with.
pub fn apply_worktree(sandbox: &mut Value, worktree: &Path, git: Option<&GitPaths>, scratch: &[PathBuf]) {
    let Some(filesystem) = sandbox.get_mut("filesystem").and_then(Value::as_object_mut) else {
        return;
    };
    let mut allow = vec![worktree.to_path_buf()];
    allow.extend(scratch.iter().cloned());
    let mut deny = vec![
        worktree.join(".claude/settings.json"),
        worktree.join(".claude/settings.local.json"),
    ];
    deny.extend(secret_denies(worktree));
    if let Some(git) = git {
        allow.extend(git.allow.iter().cloned());
        deny.extend(git.deny.iter().cloned());
    }
    extend_paths(filesystem, "allowWrite", &allow);
    extend_paths(filesystem, "denyWrite", &deny);
}

fn extend_paths(filesystem: &mut serde_json::Map<String, Value>, key: &str, paths: &[PathBuf]) {
    let list = filesystem.entry(key).or_insert_with(|| Value::Array(Vec::new()));
    let Some(list) = list.as_array_mut() else {
        return;
    };
    for path in paths {
        let path = Value::String(path.display().to_string());
        if !list.contains(&path) {
            list.push(path);
        }
    }
}

/// The same profile in sandbox-runtime (`srt --settings`) form, for tests and
/// manual probes outside the harness.
pub fn srt_settings(sandbox: &Value) -> Value {
    json!({
        "network": {
            "allowedDomains": sandbox["network"]["allowedDomains"].clone(),
            "deniedDomains": [],
        },
        "filesystem": {
            "denyRead": [],
            "allowWrite": sandbox["filesystem"]["allowWrite"].clone(),
            "denyWrite": sandbox["filesystem"]["denyWrite"].clone(),
        },
        "enableWeakerNetworkIsolation": sandbox["enableWeakerNetworkIsolation"].clone(),
    })
}

#[cfg(test)]
#[path = "worker_sandbox_tests.rs"]
mod tests;
