//! Local, durable legacy-intake admission. A pause has no automatic resume path.
//!
//! File locks serialize short admission/policy decisions, never network work.
//! The daemon lifecycle separately drains work admitted before the pause.
use std::{fs::{self, File, OpenOptions}, io::Write, path::{Path, PathBuf}};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeScope {
    pub team_id: String,
    pub canonical_project_id: String,
    pub channel_id: String,
}

impl IntakeScope {
    fn validate(&self) -> Result<(), String> {
        if Uuid::parse_str(&self.team_id).is_err()
            || self.canonical_project_id.trim().is_empty()
            || self.canonical_project_id.len() > 256
            || !self.channel_id.starts_with('C')
            || !self.channel_id.bytes().all(|b| b.is_ascii_alphanumeric())
        { return Err("invalid binding intake scope".into()); }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseRecord {
    pub operation_id: Uuid,
    pub scope: IntakeScope,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeState {
    pub schema_version: u32,
    pub generation: u64,
    pub pauses: Vec<PauseRecord>,
}

impl Default for IntakeState {
    fn default() -> Self { Self { schema_version: 1, generation: 0, pauses: Vec::new() } }
}

/// An absent journal is legacy-compatible. Any present unreadable, invalid or
/// future journal refuses admission; it is never reset or silently repaired.
pub struct IntakeJournal { directory: PathBuf }
impl IntakeJournal {
    pub fn new(cas_dir: &Path) -> Self {
        Self { directory: cas_dir.join("coordination/binding-intake") }
    }
    pub fn path(&self) -> PathBuf { self.directory.join("state.json") }
    fn lock(&self) -> Result<File, String> {
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true)
            .open(self.directory.join("state.lock")).map_err(|e| e.to_string())?;
        lock.try_lock_exclusive().map_err(|e| format!("binding intake journal busy/unavailable: {e}"))?;
        Ok(lock)
    }
    fn read_locked(&self) -> Result<IntakeState, String> {
        let bytes = match fs::read(self.path()) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(IntakeState::default()),
            Err(e) => return Err(e.to_string()),
        };
        let state: IntakeState = serde_json::from_slice(&bytes).map_err(|e| format!("invalid binding intake journal: {e}"))?;
        if state.schema_version != 1 || state.generation != state.pauses.len() as u64 {
            return Err("unsupported or inconsistent binding intake journal".into());
        }
        let mut operations = std::collections::HashSet::new();
        for (index, pause) in state.pauses.iter().enumerate() {
            pause.scope.validate()?;
            if pause.operation_id.is_nil() || pause.generation != index as u64 + 1
                || !operations.insert(pause.operation_id) {
                return Err("invalid binding intake journal generation/operation".into());
            }
        }
        Ok(state)
    }
    pub fn read(&self) -> Result<IntakeState, String> {
        let _lock = self.lock()?;
        self.read_locked()
    }
    /// Project-wide v1 claims cannot select channels. Any pause closes that
    /// admission path. C will refine direct reads without opening v1 claims.
    pub fn legacy_allowed(&self) -> Result<bool, String> { Ok(self.read()?.pauses.is_empty()) }
    pub fn pause(&self, scope: IntakeScope, operation_id: Uuid, expected_generation: u64) -> Result<PauseRecord, String> {
        scope.validate()?;
        if operation_id.is_nil() { return Err("missing intake operation identity".into()); }
        let _lock = self.lock()?;
        let mut state = self.read_locked()?;
        if let Some(existing) = state.pauses.iter().find(|p| p.operation_id == operation_id) {
            return if existing.scope == scope { Ok(existing.clone()) } else { Err("operation belongs to another scope".into()) };
        }
        if expected_generation != state.generation { return Err("binding intake generation changed; reread status".into()); }
        state.generation = state.generation.checked_add(1).ok_or("intake generation exhausted")?;
        let pause = PauseRecord { operation_id, scope, generation: state.generation };
        state.pauses.push(pause.clone());
        // RED: persistence is deliberately absent until the real-path control proves it.
        Ok(pause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cas_9021_pause_is_durable_and_corrupt_or_busy_state_never_admits() {
        let temp = tempfile::tempdir().unwrap();
        let journal = IntakeJournal::new(temp.path());
        let scope = IntakeScope { team_id: Uuid::new_v4().to_string(), canonical_project_id: "project".into(), channel_id: "C123".into() };
        assert!(journal.legacy_allowed().unwrap());
        let operation = Uuid::new_v4();
        let pause = journal.pause(scope.clone(), operation, 0).unwrap();
        assert!(!IntakeJournal::new(temp.path()).legacy_allowed().unwrap(), "a restart must retain the pause");
        assert_eq!(journal.pause(scope.clone(), operation, 0).unwrap(), pause);
        assert!(journal.pause(scope, Uuid::new_v4(), 0).is_err());
        let before = fs::read(journal.path()).unwrap();
        { let _held = journal.lock().unwrap(); assert!(journal.legacy_allowed().is_err()); }
        assert_eq!(fs::read(journal.path()).unwrap(), before);
        for invalid in [b"{".as_slice(), br#"{"schema_version":2,"generation":0,"pauses":[]}"#] {
            fs::write(journal.path(), invalid).unwrap();
            assert!(journal.legacy_allowed().is_err());
            assert_eq!(fs::read(journal.path()).unwrap(), invalid);
        }
    }
}
