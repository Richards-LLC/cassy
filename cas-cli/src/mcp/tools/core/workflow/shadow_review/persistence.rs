use super::{ReviewResult, Round, safe_id};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub(super) struct Store {
    root: PathBuf,
    _lock: File,
}
impl Store {
    pub(super) fn lock(cas_root: &Path) -> ReviewResult<Self> {
        let root = cas_root.join("shadow-reviews");
        if root.is_symlink() {
            return Err("shadow store must not be a symlink".into());
        }
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        }
        let lock = options
            .open(root.join("store.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock_exclusive().map_err(|_| {
            "another shadow operation is active; retry after it finishes".to_string()
        })?;
        Ok(Self { root, _lock: lock })
    }
    fn path(&self, id: &str) -> ReviewResult<PathBuf> {
        safe_id(id)?;
        let path = self.root.join(format!("{id}.json"));
        if path.is_symlink() {
            return Err("shadow round must not be a symlink".into());
        }
        Ok(path)
    }
    pub(super) fn exists(&self, id: &str) -> bool {
        self.path(id).is_ok_and(|p| p.exists())
    }
    pub(super) fn load(&self, id: &str) -> ReviewResult<Round> {
        let path = self.path(id)?;
        if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024 {
            return Err("shadow round exceeds storage limit".into());
        }
        let round: Round = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if round.id != id {
            return Err("shadow record id does not match its sealed path".into());
        }
        Ok(round)
    }
    pub(super) fn save(&self, round: &Round) -> ReviewResult<()> {
        use std::io::Write;
        let path = self.path(&round.id)?;
        let bytes = serde_json::to_vec_pretty(round).map_err(|e| e.to_string())?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("shadow round exceeds storage limit".into());
        }
        let temporary = self.root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
        let result = (|| {
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            std::fs::rename(&temporary, path).map_err(|e| e.to_string())?;
            File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}
