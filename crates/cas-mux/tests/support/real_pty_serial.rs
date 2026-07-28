use fs2::FileExt;
use std::fs::{File, OpenOptions};

pub struct RealPtySerialGuard(File);

impl Drop for RealPtySerialGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

/// Serialize resource-heavy real-PTY tests, including across integration
/// test binaries and concurrent Cargo invocations.
pub fn lock() -> RealPtySerialGuard {
    let path = std::env::temp_dir().join("cas-mux-real-pty-runtime-tests.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .expect("open shared real-PTY test lock");
    file.lock_exclusive()
        .expect("lock shared real-PTY test guard");
    RealPtySerialGuard(file)
}
