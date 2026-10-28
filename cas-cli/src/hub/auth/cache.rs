//! Cached durable identities with stat-based cross-process invalidation.
//!
//! Only durable transactions take auth.lock. Ordinary readers never take the
//! inter-process lock; atomic rename makes their snapshot safe to read.
use super::*;
use std::sync::{RwLockReadGuard, RwLockWriteGuard};

#[derive(Default)]
pub(super) struct CachedState {
    state: PersistedState,
    fingerprint: Option<Fingerprint>,
    pub(super) needs_compaction: bool,
}

#[derive(PartialEq, Eq)]
struct Fingerprint {
    modified: std::time::SystemTime,
    len: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    device: u64,
}

impl Fingerprint {
    fn of(metadata: &fs::Metadata) -> Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            modified: metadata.modified()?,
            len: metadata.len(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            device: metadata.dev(),
        })
    }

    fn at(path: &Path) -> Result<Option<Self>> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                // Check permissions even when size/mtime/inode did not change.
                secure_regular_file(path)?;
                Ok(Some(Self::of(&metadata)?))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

pub(super) struct CachedRead<'a> {
    cache: RwLockReadGuard<'a, CachedState>,
    _timing: LockTiming,
}
impl Deref for CachedRead<'_> {
    type Target = PersistedState;
    fn deref(&self) -> &Self::Target {
        &self.cache.state
    }
}

pub(super) struct CachedWrite<'a> {
    cache: RwLockWriteGuard<'a, CachedState>,
    _timing: LockTiming,
}
impl Deref for CachedWrite<'_> {
    type Target = PersistedState;
    fn deref(&self) -> &Self::Target {
        &self.cache.state
    }
}
impl DerefMut for CachedWrite<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cache.state
    }
}

/// Keep activity and process-local capabilities across a CLI's durable edit.
/// Revocation and credential rotation still come from the newly read snapshot.
fn keep_volatile(next: &mut PersistedState, previous: &PersistedState, keep_leases: bool) {
    if keep_leases {
        next.leases = previous.leases.clone();
    }
    next.tickets = previous.tickets.clone();
    next.dpop_jtis = previous.dpop_jtis.clone();
    for device in &mut next.devices {
        if let Some(active) = previous.devices.iter().find(|old| {
            old.device_id == device.device_id
                && old.credential_id == device.credential_id
                && old.credential_hash == device.credential_hash
        }) {
            device.last_used_at = device.last_used_at.max(active.last_used_at);
        }
    }
}

impl AuthStore {
    fn reload_cache(&self, cache: &mut CachedState) -> Result<()> {
        let target = self.0.root.join("auth.json");
        let fingerprint = Fingerprint::at(&target)?;
        if cache.fingerprint == fingerprint {
            return Ok(());
        }
        anyhow::ensure!(fingerprint.is_some(), "hub auth state disappeared");
        // Fingerprint the opened descriptor, not the pathname after the read:
        // an external atomic rename during deserialization must be noticed on
        // the next request, rather than marking the old bytes as current.
        let file = File::open(&target)?;
        let fingerprint = Fingerprint::of(&file.metadata()?)?;
        let mut next: PersistedState = serde_json::from_reader(std::io::BufReader::new(file))
            .context("invalid hub auth state")?;
        cache.needs_compaction = !next.tickets.is_empty() || !next.dpop_jtis.is_empty();
        keep_volatile(&mut next, &cache.state, cache.fingerprint.is_some());
        cache.state = next;
        cache.fingerprint = Some(fingerprint);
        Ok(())
    }

    pub(super) fn read_lock(&self) -> Result<CachedRead<'_>> {
        let started = std::time::Instant::now();
        let fingerprint = Fingerprint::at(&self.0.root.join("auth.json"))?;
        let cache = self
            .0
            .cache
            .read()
            .map_err(|_| anyhow::anyhow!("hub auth state poisoned"))?;
        if cache.fingerprint == fingerprint {
            return Ok(CachedRead {
                cache,
                _timing: LockTiming {
                    started,
                    acquired: std::time::Instant::now(),
                },
            });
        }
        drop(cache);
        drop(self.memory_lock()?);
        let cache = self
            .0
            .cache
            .read()
            .map_err(|_| anyhow::anyhow!("hub auth state poisoned"))?;
        Ok(CachedRead {
            cache,
            _timing: LockTiming {
                started,
                acquired: std::time::Instant::now(),
            },
        })
    }

    pub(super) fn memory_lock(&self) -> Result<CachedWrite<'_>> {
        let started = std::time::Instant::now();
        let mut cache = self
            .0
            .cache
            .write()
            .map_err(|_| anyhow::anyhow!("hub auth state poisoned"))?;
        let acquired = std::time::Instant::now();
        self.reload_cache(&mut cache)?;
        Ok(CachedWrite {
            cache,
            _timing: LockTiming { started, acquired },
        })
    }

    pub(super) fn publish(&self, state: &PersistedState) -> Result<()> {
        let mut cache = self
            .0
            .cache
            .write()
            .map_err(|_| anyhow::anyhow!("hub auth state poisoned"))?;
        let mut next = state.clone();
        keep_volatile(&mut next, &cache.state, true);
        cache.state = next;
        cache.fingerprint = Fingerprint::at(&self.0.root.join("auth.json"))?;
        cache.needs_compaction = false;
        Ok(())
    }
}
