//! Reuse an assessed shell executable across fixture paths and test processes.

use std::path::Path;

#[cfg(unix)]
pub fn warm_stub(path: &Path, script: &str) {
    use fs2::FileExt;
    use sha2::{Digest, Sha256};
    use std::fs::{self, OpenOptions};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Command;

    let (shebang, body) = script
        .split_once('\n')
        .expect("shell stub must have a shebang and body");
    assert!(shebang.starts_with("#!"), "shell stub needs a shebang");
    let digest = hex::encode(Sha256::digest(script.as_bytes()));
    let cache = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("test executable directory")
        .join("warm-test-stubs");
    fs::create_dir_all(&cache).expect("stub cache directory");
    let shared = cache.join(format!("{digest}.sh"));
    let ready = cache.join(format!("{digest}.ready"));
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(cache.join(format!("{digest}.lock")))
        .expect("stub cache lock");
    lock.lock_exclusive().expect("lock stub cache");
    if !shared.exists() {
        let staged = cache.join(format!("{digest}.{}.tmp", std::process::id()));
        // The warm launch must not run fixture commands before trust is tested.
        fs::write(
            &staged,
            format!("{shebang}\n[ \"${{CAS_TEST_STUB_WARMUP:-}}\" = 1 ] && exit 0\n{body}"),
        )
        .expect("write stub");
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o700))
            .expect("make stub executable");
        fs::rename(&staged, &shared).expect("publish stub");
    }
    if !ready.exists() {
        let output = Command::new(&shared)
            .env("CAS_TEST_STUB_WARMUP", "1")
            .output()
            .expect("launch stub warm-up");
        assert!(output.status.success(), "stub warm-up failed: {output:?}");
        fs::write(&ready, b"ready").expect("record warmed stub");
    }
    lock.unlock().expect("unlock stub cache");
    if path.exists() || path.is_symlink() {
        fs::remove_file(path).expect("replace test stub");
    }
    symlink(&shared, path).expect("link warmed test stub");
}
