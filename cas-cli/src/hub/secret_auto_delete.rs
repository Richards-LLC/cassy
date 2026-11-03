//! Optional auto-delete for a secret saved from Commander (cas-2f55).
//!
//! Off unless the operator chooses it on the card or sets the project's
//! `secrets.auto_delete`. A chosen secret is listed in the project's
//! `.cas/secret-auto-delete.json` with its path, file identity and trigger:
//! a stated time after saving, or the supervisor's reply to the hub's
//! "saved" message confirming use. Each removal appends one line to
//! `.cas/secret-receipts.jsonl` naming the secret, path, trigger and outcome.
//! Neither file ever holds the value or a digest of it.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LEDGER: &str = "secret-auto-delete.json";
const RECEIPTS: &str = "secret-receipts.jsonl";
const MAX_AFTER: Duration = Duration::from_secs(30 * 24 * 3600);

/// When a saved secret is removed. `Keep` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoDelete {
    Keep,
    AfterUse,
    After { secs: u64 },
}

impl AutoDelete {
    /// `off`/`keep`, `after_use`, or a duration such as `30m`, `1h` or `7d`
    /// (one minute to thirty days).
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        match value {
            "" | "off" | "keep" => return Some(Self::Keep),
            "after_use" => return Some(Self::AfterUse),
            _ => {}
        }
        let (digits, unit) = value.split_at(value.len().checked_sub(1)?);
        let count: u64 = digits.parse().ok().filter(|count| *count > 0)?;
        let secs = count.checked_mul(match unit {
            "m" => 60,
            "h" => 3600,
            "d" => 86_400,
            _ => return None,
        })?;
        (secs <= MAX_AFTER.as_secs()).then_some(Self::After { secs })
    }

    /// The project's `secrets.auto_delete`, else keep.
    pub fn project_default(cas_dir: &Path) -> Self {
        crate::config::Config::load(cas_dir)
            .ok()
            .and_then(|config| config.secrets)
            .and_then(|secrets| secrets.auto_delete)
            .and_then(|value| Self::parse(&value))
            .unwrap_or(Self::Keep)
    }

    /// The wire and config spelling, `None` for keep.
    pub fn label(self) -> Option<String> {
        match self {
            Self::Keep => None,
            Self::AfterUse => Some("after_use".into()),
            Self::After { secs } if secs % 86_400 == 0 => Some(format!("{}d", secs / 86_400)),
            Self::After { secs } if secs % 3600 == 0 => Some(format!("{}h", secs / 3600)),
            Self::After { secs } => Some(format!("{}m", secs / 60)),
        }
    }

    /// How the supervisor's reply states it.
    pub fn sentence(self) -> Option<String> {
        match self {
            Self::Keep => None,
            Self::AfterUse => Some(
                "It is deleted once you confirm you have used it: reply to this message \
                 (in_reply_to=its notification id) when you are done with it."
                    .into(),
            ),
            Self::After { .. } => Some(format!(
                "It is deleted {} after saving; read it before then.",
                self.label().unwrap_or_default()
            )),
        }
    }
}

/// One saved secret waiting for its trigger. No value, no digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pending {
    name: String,
    path: PathBuf,
    session: String,
    request_id: i64,
    #[serde(default)]
    reply_id: Option<i64>,
    saved_at: chrono::DateTime<chrono::Utc>,
    trigger: AutoDelete,
    file: (u64, u64),
}

/// One removal, appended to the receipts file.
#[derive(Debug, Serialize)]
struct Receipt<'a> {
    name: &'a str,
    path: &'a Path,
    session: &'a str,
    request_id: i64,
    saved_at: chrono::DateTime<chrono::Utc>,
    deleted_at: chrono::DateTime<chrono::Utc>,
    trigger: String,
    outcome: &'static str,
}

/// Record a just-saved secret. Keep records nothing.
pub(crate) fn schedule(
    cas_dir: &Path,
    name: &str,
    path: &Path,
    session: &str,
    request_id: i64,
    trigger: AutoDelete,
) -> anyhow::Result<()> {
    if trigger == AutoDelete::Keep {
        return Ok(());
    }
    let file = identity(path)?.ok_or_else(|| anyhow::anyhow!("saved secret is missing"))?;
    update(cas_dir, |pending| {
        pending.push(Pending {
            name: name.into(),
            path: path.into(),
            session: session.into(),
            request_id,
            reply_id: None,
            saved_at: chrono::Utc::now(),
            trigger,
            file,
        });
        Vec::new()
    })
    .map(drop)
}

/// Bind the hub's "saved" reply, so the supervisor's answer to it confirms use.
pub(crate) fn bind_reply(cas_dir: &Path, request_id: i64, reply_id: i64) -> anyhow::Result<()> {
    update(cas_dir, |pending| {
        for entry in pending
            .iter_mut()
            .filter(|entry| entry.request_id == request_id)
        {
            entry.reply_id = Some(reply_id);
        }
        Vec::new()
    })
    .map(drop)
}

/// The supervisor answered `reply_id`: remove an after-use secret saved for it.
/// Returns the removed secrets' names.
pub fn confirm_use(cas_dir: &Path, session: &str, reply_id: i64) -> anyhow::Result<Vec<String>> {
    if !cas_dir.join(LEDGER).exists() {
        return Ok(Vec::new());
    }
    update(cas_dir, |pending| {
        take(pending, |entry| {
            entry.trigger == AutoDelete::AfterUse
                && entry.reply_id == Some(reply_id)
                && entry.session == session
        })
    })
}

/// Remove every timed secret whose time has come. Returns the removed names.
pub fn sweep_due(
    cas_dir: &Path,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<Vec<String>> {
    if !cas_dir.join(LEDGER).exists() {
        return Ok(Vec::new());
    }
    update(cas_dir, |pending| {
        take(pending, |entry| match entry.trigger {
            AutoDelete::After { secs } => {
                now >= entry.saved_at + chrono::Duration::seconds(secs.min(i64::MAX as u64) as i64)
            }
            _ => false,
        })
    })
}

/// Sweep every registered project, as the hub does each minute.
pub(crate) fn sweep_registered(now: chrono::DateTime<chrono::Utc>) {
    for repo in super::idle_sync::registered_repos() {
        let cas_dir = repo.join(".cas");
        match sweep_due(&cas_dir, now) {
            Ok(removed) if !removed.is_empty() => {
                tracing::info!(project = %repo.display(), removed = removed.len(), "cas-2f55: timed secrets deleted");
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, project = %repo.display(), "cas-2f55: secret sweep failed")
            }
        }
    }
}

/// Start the hub's minute sweep for timed secrets.
pub(crate) fn spawn_sweep_loop() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Err(error) =
                tokio::task::spawn_blocking(|| sweep_registered(chrono::Utc::now())).await
            {
                tracing::warn!(%error, "cas-2f55: secret sweep panicked");
            }
        }
    })
}

/// Remove the matching entries' files and write a receipt for each.
fn take(
    pending: &mut Vec<Pending>,
    due: impl Fn(&Pending) -> bool,
) -> Vec<(Pending, &'static str)> {
    let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(pending)
        .into_iter()
        .partition(|entry| due(entry));
    *pending = kept;
    taken
        .into_iter()
        .map(|entry| {
            let outcome = match identity(&entry.path) {
                // Only the file this hub wrote is removed; a replacement stays.
                Ok(Some(file)) if file == entry.file => match std::fs::remove_file(&entry.path) {
                    Ok(()) => "deleted",
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => "already_gone",
                    Err(_) => "delete_failed",
                },
                Ok(Some(_)) => "replaced_kept",
                Ok(None) => "already_gone",
                Err(_) => "delete_failed",
            };
            (entry, outcome)
        })
        .collect()
}

/// The file's device and inode, `None` when absent. A symlink is never ours.
fn identity(path: &Path) -> anyhow::Result<Option<(u64, u64)>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                Ok(Some((metadata.dev(), metadata.ino())))
            }
            #[cfg(not(unix))]
            {
                Ok(Some((0, metadata.len())))
            }
        }
        Ok(_) => Ok(Some((u64::MAX, u64::MAX))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// A mode-600 file that is never followed through a symlink.
fn private_file(path: &Path, create_new: bool) -> anyhow::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

/// Read, change and rewrite the ledger under a lock shared by the hub and the
/// MCP server, then append receipts for what `change` removed.
fn update(
    cas_dir: &Path,
    change: impl FnOnce(&mut Vec<Pending>) -> Vec<(Pending, &'static str)>,
) -> anyhow::Result<Vec<String>> {
    let lock = private_file(&cas_dir.join(".secret-auto-delete.lock"), false)?;
    lock.lock_exclusive()?;
    let path = cas_dir.join(LEDGER);
    let mut pending: Vec<Pending> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let removed = change(&mut pending);
    let scratch = cas_dir.join(format!(".{LEDGER}.{}.pending", uuid::Uuid::new_v4()));
    let written = (|| -> anyhow::Result<()> {
        let mut file = private_file(&scratch, true)?;
        file.write_all(&serde_json::to_vec_pretty(&pending)?)?;
        file.sync_all()?;
        std::fs::rename(&scratch, &path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&scratch);
    }
    written?;
    if !removed.is_empty() {
        let mut receipts = private_file(&cas_dir.join(RECEIPTS), false)?;
        use std::io::Seek;
        receipts.seek(std::io::SeekFrom::End(0))?;
        let now = chrono::Utc::now();
        for (entry, outcome) in &removed {
            let receipt = Receipt {
                name: &entry.name,
                path: &entry.path,
                session: &entry.session,
                request_id: entry.request_id,
                saved_at: entry.saved_at,
                deleted_at: now,
                trigger: entry.trigger.label().unwrap_or_default(),
                outcome,
            };
            writeln!(receipts, "{}", serde_json::to_string(&receipt)?)?;
        }
        receipts.sync_all()?;
    }
    Ok(removed.into_iter().map(|(entry, _)| entry.name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stated time removes the file only once it has passed, and the
    /// receipt and the ledger hold no secret material.
    #[test]
    fn timed_auto_delete_removes_the_file_after_its_time_with_a_secret_free_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let path = temp.path().join("TIMED_KEY");
        std::fs::write(&path, "sk-live-value").unwrap();
        schedule(
            &cas_dir,
            "TIMED_KEY",
            &path,
            "factory-a",
            9,
            AutoDelete::parse("1h").unwrap(),
        )
        .unwrap();
        let ledger = std::fs::read_to_string(cas_dir.join(LEDGER)).unwrap();
        assert!(
            sweep_due(&cas_dir, chrono::Utc::now() + chrono::Duration::minutes(59))
                .unwrap()
                .is_empty()
        );
        assert!(path.exists());
        assert_eq!(
            sweep_due(&cas_dir, chrono::Utc::now() + chrono::Duration::minutes(61)).unwrap(),
            ["TIMED_KEY"]
        );
        assert!(!path.exists());
        let receipts = std::fs::read_to_string(cas_dir.join(RECEIPTS)).unwrap();
        assert_eq!(receipts.lines().count(), 1, "{receipts}");
        assert!(
            receipts.contains("\"outcome\":\"deleted\"") && receipts.contains("\"trigger\":\"1h\""),
            "{receipts}"
        );
        assert!(
            !receipts.contains("sk-live") && !ledger.contains("sk-live"),
            "no secret material"
        );
    }
}
