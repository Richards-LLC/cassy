//! Local transport and successful CI for merge fixtures exercising other gates.

use std::path::{Path, PathBuf};

/// Retain the SSH origin's repository identity while serving its Git protocol
/// from a bare fixture. URL rewriting also rewrites identity probes.
pub fn local_ssh_origin(repo: &Path, remote: &Path, branches: &[&str]) {
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git").args(args).current_dir(repo).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    };
    git(&["init", "--bare", remote.to_str().unwrap()]);
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"));
    let ssh = remote.join("fixture-ssh");
    cas::test_paths::warm_stub(&ssh, &format!(r#"#!/bin/sh
case "$*" in
  *git-upload-pack*) exec git upload-pack {remote} ;;
  *git-receive-pack*) exec git receive-pack {remote} ;;
  *) exit 1 ;;
esac
"#, remote=quote(remote)));
    git(&["config", "core.sshCommand", &quote(&ssh)]);
    git(&["config", "ssh.variant", "simple"]);
    let mut push = vec!["push", "origin"];
    push.extend_from_slice(branches);
    git(&push);
}

pub fn green_ci(dir: &Path) -> PathBuf {
    ci_concluded(dir, "success")
}

/// A `gh` double whose lane check run finished with `conclusion`, on a
/// GitHub repository whose default branch has no merge queue.
pub fn ci_concluded(dir: &Path, conclusion: &str) -> PathBuf {
    ci_double(dir, conclusion, "printf '%s\\n' '{\"data\":{\"repository\":{\"mergeQueue\":null}}}'")
}

/// Green CI where GitHub does not answer the merge-queue probe, so the
/// queue state stays unconfirmed (cas-662e kept this case for epic close).
#[allow(dead_code)]
pub fn green_ci_queue_unconfirmed(dir: &Path) -> PathBuf {
    ci_double(dir, "success", "exit 1")
}

fn ci_double(dir: &Path, conclusion: &str, queue_probe: &str) -> PathBuf {
    let binary = dir.join(format!("{conclusion}-ci-gh"));
    cas::test_paths::warm_stub(
        &binary,
        &format!("#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '{{\"nameWithOwner\":\"%s\"}}\\n' \"$3\"; exit 0; fi\nif [ \"$1 $2\" = 'api graphql' ]; then {queue_probe}; exit 0; fi\ncat <<'JSON'\n{{\"check_runs\":[{{\"name\":\"Scoped Validation (factory/PR)\",\"status\":\"completed\",\"conclusion\":\"{conclusion}\"}}]}}\nJSON\n"),
    );
    binary
}
