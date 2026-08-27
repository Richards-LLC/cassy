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
    OpenCode,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceStatus {
    Pass,
    Fail,
}

/// Serving route covered by a harness conformance/support claim.
///
/// OpenCode can reach the same model through the local OpenAI-compatible
/// server or through the hosted DashScope provider.  Keeping the route typed
/// prevents a receipt for one route from being read as evidence for the
/// other.  Existing Claude/Codex/Grok receipts omit this field for backwards
/// compatibility; OpenCode receipts must set it.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ServingRoute {
    Local,
    Hosted,
}

/// Secret-free identity of the serving stack covered by a receipt.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ServingIdentity {
    pub provider: String,
    pub model: String,
    pub endpoint: String,
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
    /// Route covered by this receipt.  Optional only so pre-route receipts
    /// already committed for the other harnesses remain readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<ServingRoute>,
    /// Provider/model/endpoint identity with credentials intentionally absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serving_identity: Option<ServingIdentity>,
    pub harness_version: String,
    /// Version selected by the host's default harness command at validation
    /// time. A differing value is a typed stale/warn signal for preflight,
    /// not grounds to rewrite which binary the receipt actually validated.
    #[serde(default)]
    pub observed_default_harness_version: Option<String>,
    pub validated_at: String,
    pub result: ConformanceStatus,
    pub checklist: Vec<ConformanceCheck>,
    pub evidence: Vec<ConformanceEvidence>,
}

impl HarnessConformanceReceipt {
    /// Whether this receipt names the route and serving stack it validates.
    /// Legacy receipts for the original harnesses may omit both fields, but a
    /// new OpenCode receipt cannot be accepted without an explicit identity.
    pub fn has_explicit_serving_identity(&self) -> bool {
        self.route.is_some() && self.serving_identity.is_some()
    }

    /// A version is eligible for CAS's validated pin only when the receipt and
    /// every required checklist entry pass. This intentionally fails closed.
    pub fn validates_pin(&self) -> bool {
        self.result == ConformanceStatus::Pass
            && (self.harness != Harness::OpenCode || self.has_explicit_serving_identity())
            && self
                .checklist
                .iter()
                .filter(|check| check.required)
                .all(|check| check.status == ConformanceStatus::Pass)
    }

    pub fn observed_default_matches_validated(&self) -> Option<bool> {
        self.observed_default_harness_version
            .as_ref()
            .map(|version| version == &self.harness_version)
    }
}

const CODEX_0149_RECEIPT: &str = include_str!("../conformance/codex-cli-0.149.1-2026-08-25.json");
const GROK_02114_RECEIPT: &str = include_str!("../conformance/grok-build-0.2.114-2026-07-30.json");
const GROK_0105_RECEIPT: &str = include_str!("../conformance/grok-build-1.0.5-2026-08-25.json");

pub fn codex_0149_conformance_receipt() -> Result<HarnessConformanceReceipt, serde_json::Error> {
    serde_json::from_str(CODEX_0149_RECEIPT)
}

pub fn grok_02114_conformance_receipt() -> Result<HarnessConformanceReceipt, serde_json::Error> {
    serde_json::from_str(GROK_02114_RECEIPT)
}

pub fn grok_0105_conformance_receipt() -> Result<HarnessConformanceReceipt, serde_json::Error> {
    serde_json::from_str(GROK_0105_RECEIPT)
}

/// Latest recorded receipt for each harness that currently has typed evidence.
/// Later preflight work can consume this without parsing comments or Markdown.
pub fn harness_conformance_receipts() -> Result<Vec<HarnessConformanceReceipt>, serde_json::Error> {
    Ok(vec![
        codex_0149_conformance_receipt()?,
        grok_0105_conformance_receipt()?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codex_0149_receipt_is_typed_complete_and_passes_every_required_check() {
        let receipt = codex_0149_conformance_receipt().expect("embedded receipt must parse");
        assert_eq!(receipt.schema_version, 1);
        assert_eq!(receipt.harness, Harness::CodexCli);
        assert_eq!(receipt.harness_version, "0.149.1");
        assert_eq!(receipt.validated_at, "2026-08-25");
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
    fn grok_02114_receipt_is_typed_complete_and_passes_every_required_check() {
        let receipt = grok_02114_conformance_receipt().expect("embedded receipt must parse");
        assert_eq!(receipt.schema_version, 1);
        assert_eq!(receipt.harness, Harness::GrokBuild);
        assert_eq!(receipt.harness_version, "0.2.114");
        assert_eq!(
            receipt.observed_default_harness_version.as_deref(),
            Some("0.2.117")
        );
        assert_eq!(receipt.observed_default_matches_validated(), Some(false));
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
    }

    #[test]
    fn grok_0105_receipt_is_typed_complete_and_passes_every_required_check() {
        let receipt = grok_0105_conformance_receipt().expect("embedded receipt must parse");
        assert_eq!(receipt.schema_version, 1);
        assert_eq!(receipt.harness, Harness::GrokBuild);
        assert_eq!(receipt.harness_version, "1.0.5");
        assert_eq!(
            receipt.observed_default_harness_version.as_deref(),
            Some("1.0.5")
        );
        assert_eq!(receipt.observed_default_matches_validated(), Some(true));
        assert_eq!(receipt.validated_at, "2026-08-25");
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
        let mut receipt = codex_0149_conformance_receipt().unwrap();
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

    #[test]
    fn opencode_receipt_requires_explicit_route_and_round_trips_identity() {
        let mut receipt = codex_0149_conformance_receipt().unwrap();
        receipt.harness = Harness::OpenCode;
        assert!(!receipt.validates_pin());

        receipt.route = Some(ServingRoute::Hosted);
        receipt.serving_identity = Some(ServingIdentity {
            provider: "alibaba".to_string(),
            model: "qwen3.8-max".to_string(),
            endpoint: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1".to_string(),
        });
        assert!(receipt.has_explicit_serving_identity());
        assert!(receipt.validates_pin());

        let encoded = serde_json::to_string(&receipt).unwrap();
        assert!(encoded.contains("\"route\":\"hosted\""));
        assert!(encoded.contains("\"serving_identity\""));
        assert!(!encoded.contains("DASHSCOPE_API_KEY"));
        let decoded: HarnessConformanceReceipt = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.route, Some(ServingRoute::Hosted));
        assert_eq!(decoded.serving_identity, receipt.serving_identity);
    }
}
