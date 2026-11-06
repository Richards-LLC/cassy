//! Successful CI for merge fixtures that exercise a different gate.

use std::path::{Path, PathBuf};

pub fn green_ci(dir: &Path) -> PathBuf {
    ci_concluded(dir, "success")
}

/// A `gh` double whose lane check run finished with `conclusion`, on a
/// GitHub repository whose default branch has no merge queue.
pub fn ci_concluded(dir: &Path, conclusion: &str) -> PathBuf {
    let binary = dir.join(format!("{conclusion}-ci-gh"));
    cas::test_paths::warm_stub(
        &binary,
        &format!("#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '{{\"nameWithOwner\":\"%s\"}}\\n' \"$3\"; exit 0; fi\nif [ \"$1 $2\" = 'api graphql' ]; then printf '%s\\n' '{{\"data\":{{\"repository\":{{\"mergeQueue\":null}}}}}}'; exit 0; fi\ncat <<'JSON'\n{{\"check_runs\":[{{\"name\":\"Scoped Validation (factory/PR)\",\"status\":\"completed\",\"conclusion\":\"{conclusion}\"}}]}}\nJSON\n"),
    );
    binary
}
