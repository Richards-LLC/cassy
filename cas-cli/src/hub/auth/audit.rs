//! One bounded, synchronous-API writer per store. Appends are acknowledged
//! after write(2); durability is group-committed every 250 ms or 64 records.
//! A dedicated thread also serves CLI callers without a Tokio runtime.
use super::*;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration as StdDuration, Instant};

const COMMIT_INTERVAL: StdDuration = StdDuration::from_millis(250);
const BATCH_RECORDS: usize = 64;
const ROTATE_BYTES: u64 = 8 * 1024 * 1024;
const BACKUPS: usize = 3;

type Reply = SyncSender<std::result::Result<(), String>>;
struct Entry {
    bytes: Vec<u8>,
    reply: Reply,
}

pub(super) struct AuditWriter {
    sender: Option<SyncSender<Entry>>,
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(test)]
    pub(super) syncs: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for AuditWriter {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl AuditWriter {
    pub(super) fn start(root: PathBuf, health: Arc<Mutex<Option<AuditHealth>>>) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1024);
        let lock_path = root.join("audit.lock");
        if lock_path.exists() {
            secure_regular_file(&lock_path)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options.open(&lock_path)?;
        secure_regular_file(&lock_path)?;
        #[cfg(test)]
        let syncs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let worker_syncs = syncs.clone();
        let thread = std::thread::Builder::new()
            .name("hub-audit".into())
            .spawn(move || {
                let mut writer = BatchWriter {
                    root,
                    lock,
                    pending: None,
                    records: 0,
                    last_sync: Instant::now(),
                    #[cfg(test)]
                    syncs: worker_syncs,
                };
                writer.run(receiver, health);
            })?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
            #[cfg(test)]
            syncs,
        })
    }

    pub(super) fn append<T: Serialize>(&self, value: &T) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        let (reply, received) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .context("hub audit writer stopped")?
            .try_send(Entry { bytes, reply })
            .map_err(|_| anyhow::anyhow!("hub audit queue unavailable"))?;
        received
            .recv()
            .context("hub audit writer stopped")?
            .map_err(anyhow::Error::msg)
    }
}

struct BatchWriter {
    root: PathBuf,
    lock: File,
    pending: Option<File>,
    records: usize,
    last_sync: Instant,
    #[cfg(test)]
    syncs: Arc<std::sync::atomic::AtomicUsize>,
}

impl BatchWriter {
    fn run(&mut self, receiver: Receiver<Entry>, health: Arc<Mutex<Option<AuditHealth>>>) {
        let mut sync_failed = false;
        loop {
            let timeout = COMMIT_INTERVAL.saturating_sub(self.last_sync.elapsed());
            match receiver.recv_timeout(timeout) {
                Ok(entry) => {
                    // Retry a failed commit before accepting another append.
                    let result = if sync_failed {
                        self.flush().and_then(|()| self.append(&entry.bytes))
                    } else {
                        self.append(&entry.bytes)
                    };
                    sync_failed = result.is_err() && self.pending.is_some();
                    let _ = entry
                        .reply
                        .send(result.map_err(|error| format!("{error:#}")));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Err(error) = self.flush() {
                        if !sync_failed {
                            record_audit_outcome(
                                &self.root,
                                &health,
                                "audit_batch_flush",
                                Utc::now(),
                                Some(&error),
                            );
                        }
                        sync_failed = true;
                        // Avoid a busy loop when an fsync keeps failing.
                        self.last_sync = Instant::now();
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if let Err(error) = self.flush() {
                        record_audit_outcome(
                            &self.root,
                            &health,
                            "audit_shutdown_flush",
                            Utc::now(),
                            Some(&error),
                        );
                    }
                    break;
                }
            }
        }
    }

    fn flush(&mut self) -> Result<()> {
        if let Some(file) = &self.pending {
            let started = Instant::now();
            file.sync_data()?;
            #[cfg(test)]
            self.syncs
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::debug!(target: "cas::hub::auth::timing", audit_batch_records = self.records,
                audit_sync_us = started.elapsed().as_micros() as u64, "hub audit group commit");
        }
        self.pending = None;
        self.records = 0;
        self.last_sync = Instant::now();
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        // Separate from auth.lock. Also serialize independent CLI writers with
        // rotation, so no process appends to an unlinked/renamed log by mistake.
        self.lock.lock_exclusive()?;
        let locked = self.append_locked(bytes);
        let unlocked = FileExt::unlock(&self.lock);
        locked?;
        unlocked?;
        Ok(())
    }

    fn append_locked(&mut self, bytes: &[u8]) -> Result<()> {
        let path = self.root.join(AUDIT_LOG_FILE);
        if path.exists() {
            secure_regular_file(&path)?;
        }
        let mut file = Self::open_log(&path)?;
        if let Some(pending) = &self.pending {
            // An independent process may have rotated since our last append.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if pending.metadata()?.ino() != file.metadata()?.ino() {
                    self.flush()?;
                }
            }
            #[cfg(not(unix))]
            if pending.metadata()?.created()? != file.metadata()?.created()? {
                self.flush()?;
            }
        }
        let size = file.metadata()?.len();
        if size > 0 && size.saturating_add(bytes.len() as u64) > ROTATE_BYTES {
            self.flush()?;
            for number in (1..=BACKUPS).rev() {
                let source = if number == 1 {
                    path.clone()
                } else {
                    self.root.join(format!("{AUDIT_LOG_FILE}.{}", number - 1))
                };
                let target = self.root.join(format!("{AUDIT_LOG_FILE}.{number}"));
                if source.exists() {
                    secure_regular_file(&source)?;
                    if target.exists() {
                        secure_regular_file(&target)?;
                        fs::remove_file(&target)?;
                    }
                    fs::rename(source, target)?;
                }
            }
            file = Self::open_log(&path)?;
        }
        file.write_all(bytes)?;
        if self.pending.is_none() {
            self.pending = Some(file);
        }
        self.records += 1;
        if self.records >= BATCH_RECORDS || self.last_sync.elapsed() >= COMMIT_INTERVAL {
            self.flush()?;
        }
        Ok(())
    }

    fn open_log(path: &Path) -> Result<File> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        Ok(options.open(path)?)
    }
}
