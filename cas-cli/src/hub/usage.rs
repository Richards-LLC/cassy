//! Commander usage counts (cas-0b3d), kept on this machine only.
//!
//! A browser posts its cumulative per-day count of each counted action and the
//! journey that action belongs to. The ledger keeps, per paired device, the
//! highest count seen for each (day, action), so a retried or repeated post
//! never double counts and every browser that reached this hub is summed by
//! `cas qa journeys --traffic`. Rows carry no session, project or message
//! content; the device key is the hub's own pairing id and is never printed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub const USAGE_FILE: &str = "commander-usage.json";
/// Oldest day a post may still report, matching the browser's retention.
pub const MAX_REPORT_AGE_DAYS: i64 = 35;
/// Days the ledger keeps before pruning.
pub const RETENTION_DAYS: i64 = 400;
pub const MAX_ENTRIES: usize = 1024;
pub const MAX_COUNT: u64 = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCount {
    pub journey: String,
    pub count: u64,
}

/// device -> day -> action -> count.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UsageLedger {
    pub version: u32,
    pub devices: BTreeMap<String, BTreeMap<String, BTreeMap<String, UsageCount>>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageBatch {
    pub entries: Vec<UsageEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageEntry {
    pub day: String,
    pub action: String,
    pub journey: String,
    pub count: u64,
}

fn valid_action(value: &str) -> bool {
    value.len() <= 48
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

fn valid_journey(value: &str) -> bool {
    value.len() <= 32
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Refuse the whole batch on the first invalid entry, naming it.
pub fn validate(batch: &UsageBatch, today: NaiveDate) -> std::result::Result<(), String> {
    if batch.entries.is_empty() || batch.entries.len() > MAX_ENTRIES {
        return Err(format!("entries must hold 1 to {MAX_ENTRIES} rows"));
    }
    for (index, entry) in batch.entries.iter().enumerate() {
        let day = NaiveDate::parse_from_str(&entry.day, "%Y-%m-%d")
            .map_err(|_| format!("entries[{index}].day must be YYYY-MM-DD"))?;
        let age = (today - day).num_days();
        if !(-1..=MAX_REPORT_AGE_DAYS).contains(&age) {
            return Err(format!("entries[{index}].day is outside the last {MAX_REPORT_AGE_DAYS} days"));
        }
        if !valid_action(&entry.action) {
            return Err(format!("entries[{index}].action is not a counted action name"));
        }
        if !valid_journey(&entry.journey) {
            return Err(format!("entries[{index}].journey is not a journey id"));
        }
        if entry.count == 0 || entry.count > MAX_COUNT {
            return Err(format!("entries[{index}].count must be 1 to {MAX_COUNT}"));
        }
    }
    Ok(())
}

pub fn read_ledger(root: &Path) -> Result<Option<UsageLedger>> {
    let path = root.join(USAGE_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not a usage ledger", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

#[derive(Clone)]
pub struct UsageStore {
    root: PathBuf,
    gate: Arc<Mutex<()>>,
}

impl UsageStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), gate: Arc::new(Mutex::new(())) }
    }

    /// Merge a validated batch for one device, keeping each row's highest count.
    pub fn record(&self, device: &str, batch: &UsageBatch, today: NaiveDate) -> Result<usize> {
        let _gate = self.gate.lock().map_err(|_| anyhow::anyhow!("usage ledger lock poisoned"))?;
        let mut ledger = read_ledger(&self.root)?.unwrap_or_default();
        ledger.version = 1;
        let days = ledger.devices.entry(device.to_owned()).or_default();
        let mut changed = 0;
        for entry in &batch.entries {
            let row = days.entry(entry.day.clone()).or_default().entry(entry.action.clone()).or_insert(UsageCount {
                journey: entry.journey.clone(),
                count: 0,
            });
            if entry.count > row.count || row.journey != entry.journey {
                row.count = row.count.max(entry.count);
                row.journey = entry.journey.clone();
                changed += 1;
            }
        }
        let oldest = (today - chrono::Duration::days(RETENTION_DAYS)).format("%Y-%m-%d").to_string();
        for days in ledger.devices.values_mut() {
            days.retain(|day, _| *day >= oldest);
        }
        ledger.devices.retain(|_, days| !days.is_empty());
        super::ensure_private_dir(&self.root)?;
        let path = self.root.join(USAGE_FILE);
        let temporary = self.root.join(format!("{USAGE_FILE}.tmp"));
        let _ = std::fs::remove_file(&temporary);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&ledger)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(changed)
    }
}

/// Per-journey totals for the `window_days` days ending at `today`, summed
/// across every device in the ledger.
pub fn journey_totals(ledger: &UsageLedger, today: NaiveDate, window_days: u32) -> BTreeMap<String, u64> {
    let first = (today - chrono::Duration::days(i64::from(window_days.max(1)) - 1)).format("%Y-%m-%d").to_string();
    let last = today.format("%Y-%m-%d").to_string();
    let mut totals = BTreeMap::new();
    for days in ledger.devices.values() {
        for (_, actions) in days.range(first.clone()..=last.clone()) {
            for row in actions.values() {
                *totals.entry(row.journey.clone()).or_insert(0) += row.count;
            }
        }
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(day: &str, action: &str, journey: &str, count: u64) -> UsageEntry {
        UsageEntry { day: day.into(), action: action.into(), journey: journey.into(), count }
    }

    #[test]
    fn repeated_posts_keep_the_highest_count_and_devices_sum() {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let store = UsageStore::new(temp.path().join("hub"));
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let batch = UsageBatch { entries: vec![entry("2026-10-26", "reply.typed", "HUB-J5", 3)] };
        store.record("device-a", &batch, today).unwrap();
        store.record("device-a", &batch, today).unwrap();
        let later = UsageBatch {
            entries: vec![entry("2026-10-26", "reply.typed", "HUB-J5", 4), entry("2026-10-25", "ask.answer", "HUB-J7", 2)],
        };
        store.record("device-a", &later, today).unwrap();
        store.record("device-b", &UsageBatch { entries: vec![entry("2026-10-26", "reply.typed", "HUB-J5", 1)] }, today).unwrap();
        let ledger = read_ledger(&temp.path().join("hub")).unwrap().unwrap();
        let totals = journey_totals(&ledger, today, 30);
        assert_eq!(totals.get("HUB-J5"), Some(&5));
        assert_eq!(totals.get("HUB-J7"), Some(&2));
        assert_eq!(journey_totals(&ledger, today, 1).get("HUB-J7"), None);
    }
}
