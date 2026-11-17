//! Claude worker OS sandbox profile (cas-3751a, capability guards stage A).
//!
//! A Claude worker's Bash tool runs inside Claude Code's sandbox (Seatbelt on
//! macOS). Cassy writes the `sandbox` settings block in two halves:
//! [`claude_sandbox_block`] from the project's resolved roots when the worker
//! settings file is written, and [`apply_worktree`] from the worker checkout at
//! launch. Git paths are exact. The shared `.git` directory itself is never
//! writable: allowlisting it re-opens `.git/config` and `.git/hooks`, which the
//! sandbox's built-in protection only covers for repositories under the
//! working directory (cas-ac77, measured in a throwaway repository).

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Harness environment marker the PreToolUse hook reads. Hooks inherit the
/// harness process environment, never the sandboxed shell's.
pub const SANDBOX_ENV: &str = "CAS_WORKER_SANDBOX";
/// Stage A: the sandbox enforces and the replaced text guards only log.
pub const SHADOW: &str = "shadow";

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

/// Inputs resolved by Cassy for the static half of the profile.
pub struct ProfileRoots<'a> {
    /// Effective operator write roots, artifact and scratch roots.
    pub write_roots: &'a [PathBuf],
    /// The project store (`CAS_ROOT`).
    pub cas_root: &'a Path,
    /// Protected file names (`.env`): writes to `name` and `name.*` are denied.
    pub protected_files: &'a [String],
    /// Protected path fragments (`.pem`, `id_rsa`): writes to matching files are denied.
    pub protected_patterns: &'a [String],
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
    let deny = secret_denies(&allow, roots.protected_files, roots.protected_patterns);
    json!({
        "enabled": true,
        "enabledPlatforms": ["macos"],
        "autoAllowBashIfSandboxed": true,
        "allowUnsandboxedCommands": false,
        // `gh` and other Go tools verify TLS through trustd (cas-ac77).
        "enableWeakerNetworkIsolation": true,
        "network": { "allowedDomains": ALLOWED_DOMAINS },
        "filesystem": { "allowWrite": allow, "denyWrite": deny },
    })
}

fn secret_denies(roots: &[String], files: &[String], patterns: &[String]) -> Vec<String> {
    let mut globs = Vec::new();
    // Each glob is written at the root and below it, so a match does not rely
    // on `**/` also matching zero directories.
    for root in roots {
        for dir in [root.clone(), format!("{root}/**")] {
            for name in files.iter().filter(|name| safe_glob_part(name)) {
                globs.push(format!("{dir}/{name}"));
                globs.push(format!("{dir}/{name}.*"));
            }
            for fragment in patterns.iter().filter(|fragment| safe_glob_part(fragment)) {
                globs.push(format!("{dir}/*{fragment}*"));
            }
        }
    }
    globs
}

fn safe_glob_part(part: &str) -> bool {
    !part.is_empty() && !part.contains(['/', '*', '?', '[', ']', '{', '}'])
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
/// exact git paths, and denies for Claude settings a worker could otherwise
/// widen its own profile with.
pub fn apply_worktree(sandbox: &mut Value, worktree: &Path, git: Option<&GitPaths>) {
    let Some(filesystem) = sandbox.get_mut("filesystem").and_then(Value::as_object_mut) else {
        return;
    };
    let mut allow = vec![worktree.to_path_buf()];
    let mut deny = vec![
        worktree.join(".claude/settings.json"),
        worktree.join(".claude/settings.local.json"),
    ];
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
