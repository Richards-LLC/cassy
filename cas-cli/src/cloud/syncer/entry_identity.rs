//! Recover independent legacy counter identities without changing ordinary edits.
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use sha2::{Digest, Sha256};

use super::CloudSyncer;
use crate::cloud::{EntityType, SyncOperation, project_ids_match};
use crate::error::CasError;
use crate::store::Store;
use crate::types::Entry;

fn legacy_counter_id(id: &str) -> bool {
    let Some((date, counter)) = id.rsplit_once('-') else {
        return false;
    };
    !counter.is_empty()
        && counter.bytes().all(|c| c.is_ascii_digit())
        && NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
}

/// Cloud rule v1 suffix recognition is only an occupied-destination guard.
/// Never strip this suffix or recompute an alias fold: Cloud owns that family.
fn cloud_fold_id(id: &str) -> bool {
    id.rsplit_once("-fold").is_some_and(|(source, hash)| {
        !source.is_empty()
            && hash.len() == 12
            && hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}

fn same_memory(local: &Entry, incoming: &Entry) -> bool {
    (local.has_known_creation()
        && incoming.has_known_creation()
        && local.created == incoming.created)
        || (local.title == incoming.title && local.content == incoming.content)
}

pub(super) fn recovery_id(entry: &Entry) -> String {
    let mut hash = Sha256::new();
    hash.update(b"cas-memory-collision-v1\0");
    hash.update(entry.id.as_bytes());
    hash.update(b"\0");
    if entry.has_known_creation() {
        hash.update(
            entry
                .created
                .to_rfc3339_opts(SecondsFormat::Nanos, true)
                .as_bytes(),
        );
    } else {
        // No birth was recorded. Keep the local recovery namespace, using
        // title/body as the only available identity, never the sentinel.
        hash.update(
            serde_json::to_vec(&serde_json::json!({
                "content": entry.content, "title": entry.title,
            }))
            .expect("entry strings are JSON serializable"),
        );
    }
    format!("{}-recovered-{:x}", entry.id, hash.finalize())
}

pub(super) fn verify_recovery_birth(local: &Entry, incoming: &Entry) -> Result<(), CasError> {
    let recovery = incoming
        .id
        .rsplit_once("-recovered-")
        .is_some_and(|(source, hash)| {
            legacy_counter_id(source)
                && hash.len() == 64
                && hash.bytes().all(|c| c.is_ascii_hexdigit())
        });
    let different_birth = if local.has_known_creation() && incoming.has_known_creation() {
        local.created != incoming.created
    } else {
        !same_memory(local, incoming)
    };
    if recovery && different_birth {
        return Err(CasError::Other(format!(
            "Memory collision recovery target {} belongs to another creation; no memory was overwritten",
            incoming.id
        )));
    }
    Ok(())
}

fn existing_entry(store: &dyn Store, id: &str) -> Result<Option<Entry>, CasError> {
    match store.get(id) {
        Ok(entry) => Ok(Some(entry)),
        Err(cas_store::StoreError::EntryNotFound(_)) => match store.get_archived(id) {
            Ok(entry) => Ok(Some(entry)),
            Err(cas_store::StoreError::EntryNotFound(_)) => Ok(None),
            Err(error) => Err(error.into()),
        },
        Err(error) => Err(error.into()),
    }
}

impl CloudSyncer {
    /// Preserve both bodies durably before consuming a contested Cloud key.
    /// Returning a local conflict lets the pull advance its watermark instead
    /// of fetching this row forever. The incumbent (active or archived) stays
    /// untouched and the incoming copy is inspectable in cloud conflicts.
    pub(super) fn preserve_fold_conflict(
        &self,
        local: &Entry,
        incoming: &Entry,
        remote_updated_at: DateTime<Utc>,
        warnings: &mut Vec<String>,
    ) -> Result<bool, CasError> {
        if !cloud_fold_id(&incoming.id) || same_memory(local, incoming) {
            return Ok(false);
        }
        let journal = serde_json::to_string(&serde_json::json!({
            "local": local, "incoming": incoming,
            "remote_updated_at": remote_updated_at, "reason": "fold_conflict",
        }))?;
        self.queue.record_conflict(
            EntityType::Entry.as_str(),
            &incoming.id,
            &journal,
            "local",
            crate::cloud::MEMORY_FOLD_CONFLICT,
            self.queue.revision(EntityType::Entry, &local.id)?,
            self.incoming_revisions.lock().ok().and_then(|revisions| {
                revisions
                    .get(&(EntityType::Entry.as_str().into(), incoming.id.clone()))
                    .copied()
            }),
        )?;
        warnings.push(incoming.id.clone());
        tracing::warn!(id = %incoming.id, "Folded memory conflict: both copies retained; inspect cas cloud conflicts --json");
        Ok(true)
    }

    /// A legacy counter is not a globally unique identity. Unequal immutable
    /// creation instants plus unequal title/body prove independent memories.
    /// Equal creation instants remain ordinary LWW edits: historical independent
    /// births with exactly equal timestamps cannot be distinguished reliably.
    /// New random IDs and custom IDs keep their existing identity semantics.
    ///
    /// The incoming memory keeps one stable recovery ID across retries and
    /// content edits. Mutable content and project aliases never enter its hash.
    pub(super) fn prepare_pulled_entry(
        &self,
        store: &dyn Store,
        mut incoming: Entry,
        remote_updated_at: DateTime<Utc>,
        project_id: &str,
        team_id: Option<&str>,
        row_origin: Option<&str>,
    ) -> Result<Entry, CasError> {
        if !legacy_counter_id(&incoming.id) {
            return Ok(incoming);
        }
        let Some(local) = existing_entry(store, &incoming.id)? else {
            return Ok(incoming);
        };
        if same_memory(&local, &incoming) {
            return Ok(incoming);
        }
        let source_id = incoming.id.clone();
        incoming.id = recovery_id(&incoming);
        let recovered = existing_entry(store, &incoming.id)?;
        if let Some(row) = &recovered {
            verify_recovery_birth(row, &incoming)?;
        }
        let needs_publish = match &recovered {
            None => true,
            Some(row) => {
                (row.title != incoming.title
                    || row.content != incoming.content
                    || row.archived != incoming.archived)
                    && remote_updated_at > store.recent_timestamp(row)?
            }
        };
        if recovered.is_none() {
            let journal = serde_json::to_string(&serde_json::json!({
                "local": local, "incoming": incoming, "source_id": source_id,
                "recovered_id": incoming.id, "reason": "independent_legacy_memory_birth",
            }))?;
            self.queue.record_conflict(
                EntityType::Entry.as_str(),
                &source_id,
                &journal,
                "both",
                "entry_id_collision_rekey",
                None,
                None,
            )?;
        }
        // Queue before applying so a failed local write cannot strand the
        // preserved body behind an advanced pull watermark. Queue upserts are
        // idempotent; an unchanged replay never re-enqueues an acknowledged row.
        // Scope echoes alone do not prove authorship: never republish an
        // unattributed pull under this project's identity.
        if needs_publish && row_origin.is_some_and(|origin| project_ids_match(origin, project_id)) {
            if incoming.origin_project.is_none() {
                incoming.origin_project = row_origin.map(str::to_owned);
            }
            let payload = serde_json::to_string(&incoming)?;
            match team_id {
                Some(team) => self.queue.enqueue_for_team(
                    EntityType::Entry,
                    &incoming.id,
                    SyncOperation::Upsert,
                    Some(&payload),
                    team,
                )?,
                None => self.queue.enqueue(
                    EntityType::Entry,
                    &incoming.id,
                    SyncOperation::Upsert,
                    Some(&payload),
                )?,
            }
        }
        Ok(incoming)
    }
}
