//! Typed, machine-readable harness conformance receipts.
//!
//! Preflight code consumes this schema directly. Markdown diaries remain useful
//! operator context, but are never the source of truth for validation status.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    ClaudeCode,
    CodexCli,
    GrokBuild,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceStatus {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ConformanceCheck {
    pub id: String,
    pub required: bool,
    pub status: ConformanceStatus,
    pub evidence_refs: Vec<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ConformanceEvidence {
    pub id: String,
    pub kind: String,
    pub reference: String,
    pub summary: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HarnessConformanceReceipt {
    pub schema_version: u32,
    pub receipt_id: String,
    pub harness: Harness,
    pub harness_version: String,
    pub validated_at: String,
    pub result: ConformanceStatus,
    pub checklist: Vec<ConformanceCheck>,
    pub evidence: Vec<ConformanceEvidence>,
}

impl HarnessConformanceReceipt {
    /// A version is eligible for CAS's validated pin only when the receipt and
    /// every required checklist entry pass. This intentionally fails closed.
    pub fn validates_pin(&self) -> bool {
        self.result == ConformanceStatus::Pass
            && self
                .checklist
                .iter()
                .filter(|check| check.required)
                .all(|check| check.status == ConformanceStatus::Pass)
    }
}

const CODEX_0146_RECEIPT: &str = include_str!("../conformance/codex-cli-0.146.0-2026-07-30.json");

pub fn codex_0146_conformance_receipt() -> Result<HarnessConformanceReceipt, serde_json::Error> {
    serde_json::from_str(CODEX_0146_RECEIPT)
}

/// Latest recorded receipt for each harness that currently has typed evidence.
/// Later preflight work can consume this without parsing comments or Markdown.
pub fn harness_conformance_receipts() -> Result<Vec<HarnessConformanceReceipt>, serde_json::Error> {
    Ok(vec![codex_0146_conformance_receipt()?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codex_0146_receipt_is_typed_complete_and_passes_every_required_check() {
        let receipt = codex_0146_conformance_receipt().expect("embedded receipt must parse");
        assert_eq!(receipt.schema_version, 1);
        assert_eq!(receipt.harness, Harness::CodexCli);
        assert_eq!(receipt.harness_version, "0.146.0");
        assert_eq!(receipt.validated_at, "2026-07-30");
        assert_eq!(receipt.result, ConformanceStatus::Pass);
        assert!(receipt.validates_pin());

        let evidence_ids: HashSet<&str> = receipt
            .evidence
            .iter()
            .map(|evidence| evidence.id.as_str())
            .collect();
        assert!(!receipt.checklist.is_empty());
        for check in &receipt.checklist {
            assert!(
                !check.evidence_refs.is_empty(),
                "{} must cite evidence",
                check.id
            );
            assert!(
                check
                    .evidence_refs
                    .iter()
                    .all(|id| evidence_ids.contains(id.as_str())),
                "{} cites missing evidence: {:?}",
                check.id,
                check.evidence_refs
            );
        }
        assert!(
            receipt
                .checklist
                .iter()
                .filter(|check| check.required)
                .all(|check| check.status == ConformanceStatus::Pass),
            "a PASS receipt must pass every required check"
        );
    }

    #[test]
    fn failed_required_check_blocks_pin_even_if_top_level_remains_pass() {
        let mut receipt = codex_0146_conformance_receipt().unwrap();
        receipt
            .checklist
            .iter_mut()
            .find(|check| check.required)
            .expect("receipt has a required check")
            .status = ConformanceStatus::Fail;
        assert!(
            !receipt.validates_pin(),
            "required-check failure must independently prevent a pin bump"
        );
    }

    #[test]
    fn latest_receipts_are_unique_by_harness() {
        let receipts = harness_conformance_receipts().unwrap();
        let unique: HashSet<Harness> = receipts.iter().map(|receipt| receipt.harness).collect();
        assert_eq!(receipts.len(), unique.len());
    }
}
