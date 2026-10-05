//! Successful CI for merge fixtures that exercise a different gate.

use std::path::{Path, PathBuf};

pub fn green_ci(dir: &Path) -> PathBuf {
    let binary = dir.join("green-ci-gh");
    cas::test_paths::warm_stub(
        &binary,
        "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '{\"nameWithOwner\":\"%s\"}\\n' \"$3\"; exit 0; fi\ncat <<'JSON'\n{\"check_runs\":[{\"name\":\"Scoped Validation (factory/PR)\",\"status\":\"completed\",\"conclusion\":\"success\"}]}\nJSON\n",
    );
    binary
}
