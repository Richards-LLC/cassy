//! `BLOCKED_EXTERNAL{provider, error}` (cas-9779): a provider failed,
//! throttled or ran out (`external`, `rate_limited`, `insufficient`) before a
//! row could be judged. The row stays owed: it is never PASS and never FAIL.
//! Read-back outcomes and `LEDGER.md` rows share this one type.
use super::{AdapterError, ErrorKind};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The row verdict word.
pub const VERDICT: &str = "BLOCKED_EXTERNAL";
/// A ledger row's evidence cell: `blocked: provider=<provider> error=<error>`.
pub const EVIDENCE_PREFIX: &str = "blocked:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedExternal {
    pub provider: String,
    pub error: String,
}

impl BlockedExternal {
    /// The verdict for an adapter error a provider caused, or `None` when the
    /// adapter or its request is at fault. An unnamed provider stays visible.
    pub fn from_adapter(error: &AdapterError) -> Option<Self> {
        matches!(
            error.kind,
            ErrorKind::External | ErrorKind::RateLimited | ErrorKind::Insufficient
        )
        .then(|| Self {
            provider: error
                .provider
                .clone()
                .filter(|p| !p.trim().is_empty())
                .unwrap_or_else(|| "unnamed provider".into()),
            error: error.message.clone(),
        })
    }

    /// Parses a ledger evidence cell; both fields must be non-empty.
    pub fn from_evidence(cell: &str) -> Option<Self> {
        let rest = cell.strip_prefix(EVIDENCE_PREFIX)?.trim();
        let (provider, error) = rest.strip_prefix("provider=")?.split_once(" error=")?;
        let (provider, error) = (provider.trim(), error.trim());
        (!provider.is_empty() && !provider.contains(char::is_whitespace) && !error.is_empty()).then(
            || Self {
                provider: provider.into(),
                error: error.into(),
            },
        )
    }

    /// Finds the provider and error for a row whose verdict cell starts with
    /// [`VERDICT`]: after the verdict word, or in any `blocked:` cell.
    pub fn from_row(verdict: &str, cells: &[&str]) -> Option<Self> {
        let rest = verdict.strip_prefix(VERDICT)?;
        let rest =
            rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '*' | ':' | '—' | '-'));
        Self::from_evidence(rest)
            .or_else(|| cells.iter().find_map(|cell| Self::from_evidence(cell)))
    }

    /// `<row> (cell <cell>) is BLOCKED_EXTERNAL{…}`, shared by every ledger gate.
    pub fn owed(&self, row: &str, cell: Option<&str>) -> String {
        match cell.filter(|cell| !cell.is_empty()) {
            Some(cell) => format!("{row} (cell {cell}) is {self}"),
            None => format!("{row} is {self}"),
        }
    }
}

impl fmt::Display for BlockedExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{VERDICT}{{provider={}, error={}}}",
            self.provider, self.error
        )
    }
}
