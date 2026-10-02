//! Successful CI for merge fixtures that exercise a different gate.

use std::path::{Path, PathBuf};

pub fn green_ci(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let binary = dir.join("green-ci-gh");
    std::fs::write(
        &binary,
        "#!/bin/sh\ncat <<'JSON'\n{\"check_runs\":[{\"name\":\"Scoped Validation (factory/PR)\",\"status\":\"completed\",\"conclusion\":\"success\"}]}\nJSON\n",
    )
    .expect("write successful CI fixture");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
        .expect("chmod CI fixture");
    binary
}
