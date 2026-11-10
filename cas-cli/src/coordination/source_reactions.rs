//! Receipted source-post status, reconciled by the owning supervisor.
use std::path::Path;
use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::cli::violet_hub::VioletError;

pub(crate) fn reconcile(
    _root: &Path, _registration: &str, _session: &str, _now: DateTime<Utc>,
    _call: &mut dyn FnMut(&str, Value) -> Result<Value, VioletError>,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
