//! Bounded, read-only ambient recall contracts.
//!
//! This module deliberately owns neither ingestion nor model/session launch.
//! It turns an already-authorized factory event into a stable query, asks
//! namespace-aware retrievers for compact candidates, and rejects anything
//! outside the caller's project/team/private boundary before ranking.  Source
//! bodies stay in their authoritative stores.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Marker set by every CAS-owned nested model invocation (cas-fa38).
pub(crate) const INTERNAL_LLM_ENV: &str = crate::internal_llm::INTERNAL_LLM_ENV;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecallRole {
    Worker,
    Supervisor,
}

impl RecallRole {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "worker" => Some(Self::Worker),
            "supervisor" => Some(Self::Supervisor),
            _ => None,
        }
    }

    pub(crate) fn policy(self) -> RolePolicy {
        match self {
            Self::Worker => RolePolicy {
                default_tokens: 1_200,
                ceiling_tokens: 2_000,
                emergency_tokens: 4_000,
                candidate_cap: 48,
                injection_cap: 8,
                query_tokens: 512,
            },
            Self::Supervisor => RolePolicy {
                default_tokens: 1_800,
                ceiling_tokens: 3_000,
                emergency_tokens: 5_000,
                candidate_cap: 72,
                injection_cap: 12,
                query_tokens: 768,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RolePolicy {
    pub(crate) default_tokens: usize,
    pub(crate) ceiling_tokens: usize,
    pub(crate) emergency_tokens: usize,
    pub(crate) candidate_cap: usize,
    pub(crate) injection_cap: usize,
    pub(crate) query_tokens: usize,
}

/// Authoritative identity snapshot from the outer factory hook.
///
/// Nested model calls are rejected via `internal_llm`; they must never clone a
/// worker identity merely because the parent environment still contains one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecallIdentity {
    pub(crate) session_id: String,
    pub(crate) agent_name: String,
    pub(crate) factory_session: String,
    pub(crate) role: RecallRole,
    pub(crate) project_id: String,
    pub(crate) team_id: Option<String>,
    pub(crate) internal_llm: bool,
}

impl RecallIdentity {
    pub(crate) fn is_eligible(&self) -> bool {
        !self.internal_llm
            && !self.session_id.trim().is_empty()
            && !self.agent_name.trim().is_empty()
            && !self.factory_session.trim().is_empty()
            && !self.project_id.trim().is_empty()
    }

    pub(crate) fn scope_gate(&self) -> ScopeGate {
        ScopeGate {
            project_id: self.project_id.clone(),
            team_id: self.team_id.clone(),
            private_owner: self.agent_name.clone(),
            allow_global: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub(crate) enum EvidenceScope {
    Global,
    Project(String),
    Team(String),
    Private(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeGate {
    pub(crate) project_id: String,
    pub(crate) team_id: Option<String>,
    pub(crate) private_owner: String,
    pub(crate) allow_global: bool,
}

impl ScopeGate {
    pub(crate) fn allows(&self, scope: &EvidenceScope) -> bool {
        match scope {
            EvidenceScope::Global => self.allow_global,
            EvidenceScope::Project(id) => id == &self.project_id,
            EvidenceScope::Team(id) => self.team_id.as_ref() == Some(id),
            EvidenceScope::Private(owner) => owner == &self.private_owner,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceSurface {
    Memory,
    Guidance,
    Knowledge,
    Task,
    Rule,
    Skill,
    Spec,
    History,
    Code,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EvidenceProvenance {
    pub(crate) source: String,
    pub(crate) locator: String,
    pub(crate) observed_at: Option<DateTime<Utc>>,
    pub(crate) revision: String,
}

/// Compact first-stage evidence. There is intentionally no body field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EvidenceCandidate {
    pub(crate) evidence_id: String,
    pub(crate) surface: EvidenceSurface,
    pub(crate) scope: EvidenceScope,
    pub(crate) snippet: String,
    pub(crate) why_relevant: String,
    pub(crate) provenance: EvidenceProvenance,
    pub(crate) relevance: f64,
    pub(crate) lexical_score: f64,
    pub(crate) semantic_score: Option<f64>,
    pub(crate) structural_score: f64,
    pub(crate) role_score: f64,
    pub(crate) binding: bool,
    pub(crate) stale: bool,
    pub(crate) conflict_key: Option<String>,
    pub(crate) body_available: bool,
}

impl EvidenceCandidate {
    fn canonical_key(&self) -> String {
        format!("{}@{}", self.evidence_id, self.provenance.revision)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RecallRequest {
    pub(crate) prompt: String,
    pub(crate) task_id: Option<String>,
    pub(crate) task_title: Option<String>,
    pub(crate) task_labels: Vec<String>,
    pub(crate) files: Vec<String>,
    pub(crate) symbols: Vec<String>,
    pub(crate) recent_decisions: Vec<String>,
    pub(crate) seen_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecallQuery {
    pub(crate) canonical: String,
    pub(crate) role: RecallRole,
    pub(crate) task_id: Option<String>,
    pub(crate) files: Vec<String>,
    pub(crate) symbols: Vec<String>,
}

impl RecallQuery {
    pub(crate) fn build(identity: &RecallIdentity, request: &RecallRequest) -> Option<Self> {
        if !identity.is_eligible() {
            return None;
        }
        let mut files = stable_values(&request.files, 16);
        let mut symbols = stable_values(&request.symbols, 16);
        let labels = stable_values(&request.task_labels, 12);
        let decisions = stable_values(&request.recent_decisions, 4);
        let seen = stable_values(&request.seen_evidence, 32);
        // Keep ownership deterministic even when callers reuse their buffers.
        files.shrink_to_fit();
        symbols.shrink_to_fit();

        let mut lines = vec![
            format!("role={:?}", identity.role).to_ascii_lowercase(),
            format!("project={}", clean_scalar(&identity.project_id, 160)),
        ];
        if let Some(task_id) = request.task_id.as_deref() {
            lines.push(format!("task={}", clean_scalar(task_id, 80)));
        }
        if let Some(title) = request.task_title.as_deref() {
            lines.push(format!("task_title={}", clean_scalar(title, 240)));
        }
        if !labels.is_empty() {
            lines.push(format!("labels={}", labels.join(",")));
        }
        if !files.is_empty() {
            lines.push(format!("files={}", files.join(",")));
        }
        if !symbols.is_empty() {
            lines.push(format!("symbols={}", symbols.join(",")));
        }
        if !decisions.is_empty() {
            lines.push(format!("decisions={}", decisions.join(" | ")));
        }
        if !seen.is_empty() {
            lines.push(format!("already_seen={}", seen.join(",")));
        }
        let prompt = redact_prompt(&request.prompt);
        if !prompt.is_empty() {
            lines.push(format!("request={prompt}"));
        }

        let max_chars = identity.role.policy().query_tokens.saturating_mul(4);
        let canonical = truncate_utf8(&lines.join("\n"), max_chars);
        Some(Self {
            canonical,
            role: identity.role,
            task_id: request.task_id.clone(),
            files,
            symbols,
        })
    }
}

/// A retriever must apply `scope` in its lookup predicate, before similarity.
/// The runtime repeats the check before fusion as defense in depth.
pub(crate) trait RecallRetriever {
    fn retrieve(
        &self,
        query: &RecallQuery,
        scope: &ScopeGate,
        limit: usize,
    ) -> Vec<EvidenceCandidate>;
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecallCandidates {
    pub(crate) candidates: Vec<EvidenceCandidate>,
    pub(crate) rejected_scope: usize,
}

pub(crate) fn retrieve_candidates(
    identity: &RecallIdentity,
    request: &RecallRequest,
    retrievers: &[&dyn RecallRetriever],
) -> Option<RecallCandidates> {
    let query = RecallQuery::build(identity, request)?;
    let policy = identity.role.policy();
    let scope = identity.scope_gate();
    let per_source = (policy.candidate_cap / retrievers.len().max(1)).max(4);
    let mut candidates = Vec::new();
    let mut rejected_scope = 0;
    for retriever in retrievers {
        for candidate in retriever.retrieve(&query, &scope, per_source) {
            if scope.allows(&candidate.scope) {
                candidates.push(candidate);
            } else {
                rejected_scope += 1;
            }
        }
    }

    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.canonical_key()));
    candidates.sort_by(|a, b| {
        b.binding
            .cmp(&a.binding)
            .then_with(|| a.stale.cmp(&b.stale))
            .then_with(|| b.relevance.total_cmp(&a.relevance))
            .then_with(|| a.evidence_id.cmp(&b.evidence_id))
    });
    candidates.truncate(policy.candidate_cap);
    Some(RecallCandidates {
        candidates,
        rejected_scope,
    })
}

fn stable_values(values: &[String], cap: usize) -> Vec<String> {
    let mut values: Vec<String> = values
        .iter()
        .map(|value| clean_scalar(value, 240))
        .filter(|value| !value.is_empty())
        .collect();
    values.sort();
    values.dedup();
    values.truncate(cap);
    values
}

fn clean_scalar(value: &str, max_chars: usize) -> String {
    truncate_utf8(
        &value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(['\n', '\r', '\0'], " "),
        max_chars,
    )
}

fn redact_prompt(prompt: &str) -> String {
    let mut kept = Vec::new();
    let mut fenced = false;
    for line in prompt.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || trimmed.len() > 320 {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if [
            "api_key",
            "apikey",
            "password",
            "secret",
            "authorization:",
            "bearer ",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        {
            kept.push("[redacted]".to_string());
        } else if !trimmed.is_empty() {
            kept.push(trimmed.to_string());
        }
    }
    clean_scalar(&kept.join(" "), 1_600)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn identity(role: RecallRole) -> RecallIdentity {
        RecallIdentity {
            session_id: "session-1".into(),
            agent_name: "worker-one".into(),
            factory_session: "factory-1".into(),
            role,
            project_id: "project-a".into(),
            team_id: Some("team-a".into()),
            internal_llm: false,
        }
    }

    fn candidate(id: &str, scope: EvidenceScope) -> EvidenceCandidate {
        EvidenceCandidate {
            evidence_id: id.into(),
            surface: EvidenceSurface::Memory,
            scope,
            snippet: "compact fact".into(),
            why_relevant: "exact task term".into(),
            provenance: EvidenceProvenance {
                source: "sqlite".into(),
                locator: id.into(),
                observed_at: None,
                revision: "r1".into(),
            },
            relevance: 0.8,
            lexical_score: 0.8,
            semantic_score: None,
            structural_score: 0.0,
            role_score: 0.0,
            binding: false,
            stale: false,
            conflict_key: None,
            body_available: true,
        }
    }

    struct FixedRetriever {
        calls: Cell<usize>,
        rows: Vec<EvidenceCandidate>,
    }

    impl RecallRetriever for FixedRetriever {
        fn retrieve(
            &self,
            _query: &RecallQuery,
            _scope: &ScopeGate,
            _limit: usize,
        ) -> Vec<EvidenceCandidate> {
            self.calls.set(self.calls.get() + 1);
            self.rows.clone()
        }
    }

    #[test]
    fn role_policies_bind_the_study_defaults_and_hard_caps() {
        let worker = RecallRole::Worker.policy();
        let supervisor = RecallRole::Supervisor.policy();
        assert_eq!(
            (worker.default_tokens, worker.ceiling_tokens),
            (1_200, 2_000)
        );
        assert_eq!(
            (supervisor.default_tokens, supervisor.ceiling_tokens),
            (1_800, 3_000)
        );
        assert_eq!(
            (worker.emergency_tokens, supervisor.emergency_tokens),
            (4_000, 5_000)
        );
        assert_eq!((worker.injection_cap, supervisor.injection_cap), (8, 12));
    }

    #[test]
    fn query_is_stable_redacted_and_excludes_quoted_bulk() {
        let mut request = RecallRequest {
            prompt: "Fix parser\napi_key=do-not-leak\n```\n300k body\n```\nthen test".into(),
            files: vec!["z.rs".into(), "a.rs".into(), "a.rs".into()],
            symbols: vec!["parse_z".into(), "parse_a".into()],
            ..Default::default()
        };
        let first = RecallQuery::build(&identity(RecallRole::Worker), &request).unwrap();
        request.files.reverse();
        request.symbols.reverse();
        let second = RecallQuery::build(&identity(RecallRole::Worker), &request).unwrap();
        assert_eq!(first.canonical, second.canonical);
        assert!(first.canonical.contains("files=a.rs,z.rs"));
        assert!(first.canonical.contains("[redacted]"));
        assert!(!first.canonical.contains("do-not-leak"));
        assert!(!first.canonical.contains("300k body"));
        assert!(first.canonical.len() <= 512 * 4);
    }

    #[test]
    fn scope_gate_rejects_cross_project_team_and_private_rows() {
        let retriever = FixedRetriever {
            calls: Cell::new(0),
            rows: vec![
                candidate("project-ok", EvidenceScope::Project("project-a".into())),
                candidate("team-ok", EvidenceScope::Team("team-a".into())),
                candidate("private-ok", EvidenceScope::Private("worker-one".into())),
                candidate("global-ok", EvidenceScope::Global),
                candidate("project-leak", EvidenceScope::Project("project-b".into())),
                candidate("team-leak", EvidenceScope::Team("team-b".into())),
                candidate("private-leak", EvidenceScope::Private("worker-two".into())),
            ],
        };
        let result = retrieve_candidates(
            &identity(RecallRole::Worker),
            &RecallRequest {
                prompt: "parser".into(),
                ..Default::default()
            },
            &[&retriever],
        )
        .unwrap();
        let ids: Vec<&str> = result
            .candidates
            .iter()
            .map(|row| row.evidence_id.as_str())
            .collect();
        assert_eq!(result.rejected_scope, 3);
        assert_eq!(
            ids,
            vec!["global-ok", "private-ok", "project-ok", "team-ok"]
        );
    }

    #[test]
    fn nested_internal_model_identity_never_reaches_retrievers() {
        let retriever = FixedRetriever {
            calls: Cell::new(0),
            rows: vec![],
        };
        let mut nested = identity(RecallRole::Worker);
        nested.internal_llm = true;
        assert!(
            retrieve_candidates(
                &nested,
                &RecallRequest {
                    prompt: "Analyze this user prompt".into(),
                    ..Default::default()
                },
                &[&retriever],
            )
            .is_none()
        );
        assert_eq!(retriever.calls.get(), 0);
        assert_eq!(INTERNAL_LLM_ENV, "CAS_INTERNAL_LLM");
    }

    #[test]
    fn first_stage_schema_is_evidence_bearing_but_has_no_body() {
        let encoded = serde_json::to_value(candidate("memory-1", EvidenceScope::Global)).unwrap();
        assert_eq!(encoded["provenance"]["source"], "sqlite");
        assert_eq!(encoded["provenance"]["revision"], "r1");
        assert_eq!(encoded["body_available"], true);
        assert!(encoded.get("body").is_none());
    }
}
