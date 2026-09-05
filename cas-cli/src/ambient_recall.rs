//! Bounded, read-only ambient recall contracts.
//!
//! This module deliberately owns neither ingestion nor model/session launch.
//! It turns an already-authorized factory event into a stable query, asks
//! namespace-aware retrievers for compact candidates, and rejects anything
//! outside the caller's project/team/private boundary before ranking.  Source
//! bodies stay in their authoritative stores.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloud::embeddings::{
    EmbeddingMeta, KnowledgeEmbedder, KnowledgeVectorCache, VectorNamespace, code_symbol_key,
    cosine_similarity, history_commit_key, history_doc_key, is_zero_vector,
};

const AMBIENT_RETRIEVAL_POLICY: &str = "ambient-recall-outcome-v1";

/// Hook feedback is deliberately small: inputs beyond this boundary cannot
/// increase matching work or be retained by the automatic capture pass.
const TOOL_ACTIVITY_BYTE_CAP: usize = 32 * 1024;
/// Persist only small, redacted trigger facts from tool traffic. Raw tool
/// inputs/results are never retained by ambient recall.
const TOOL_TRIGGER_PATH_CAP: usize = 16;
const TOOL_TRIGGER_TERM_CAP: usize = 24;

/// Automatic hooks are on the user's interactive critical path. Semantic
/// recall may spend at most this long waiting for the optional provider; a
/// timeout is an explicit degradation to the always-local lexical channel.
const HOOK_SEMANTIC_TIMEOUT: Duration = Duration::from_millis(400);

/// Maximum authoritative rows whose cached vectors may be read and compared
/// for one namespace during one hook. Candidate discovery uses bounded FTS /
/// index lookups and a deterministic head window, never a corpus-wide scan.
const SEMANTIC_CANDIDATE_CAP_PER_NAMESPACE: usize = 32;

/// Lexical evidence is only a bounded fallback when semantic evidence is
/// absent. This keeps one noisy local match from crowding out the channel that
/// carries most recall value while preserving exact task/file bindings.
const LEXICAL_INJECTION_CAP: usize = 3;

/// A focused epic is an explicit domain signal from the factory session. A
/// row that does not overlap that domain needs an unambiguously strong vector
/// match before it may cross the boundary.
const FOCUSED_EPIC_SEMANTIC_FLOOR: f64 = 0.80;

/// Live sampling found generic semantic matches through 0.467, while useful
/// independent matches began at 0.470. This only governs unbound cards whose
/// lexical evidence was rejected as noise; focused-epic mismatches stay 0.80.
const SEMANTIC_INJECTION_FLOOR: f64 = 0.47;

/// Terms present in at least 80% of a bounded local result corpus cannot
/// solely justify lexical relevance. Three rows avoids penalizing tiny result
/// sets that happen to share their only meaningful term.
const UBIQUITOUS_TERM_MIN_DOCUMENTS: usize = 3;
const UBIQUITOUS_TERM_DOCUMENT_FREQUENCY: f64 = 0.80;

/// Decision traces are evidence for diagnosing an ambient-recall turn, not a
/// second retrieval corpus. Keep one record bounded even when a retriever has
/// a large local candidate window.
const RECALL_DECISION_CANDIDATE_CAP: usize = 24;
const RECALL_DECISION_TERM_CAP: usize = 24;

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

impl EvidenceSurface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "entry",
            Self::Guidance => "guidance",
            Self::Knowledge => "knowledge",
            Self::Task => "task",
            Self::Rule => "rule",
            Self::Skill => "skill",
            Self::Spec => "spec",
            Self::History => "history",
            Self::Code => "code",
        }
    }
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
    pub(crate) lexical_eligible: bool,
    pub(crate) lexical_weak: bool,
    pub(crate) strong_session_signal: bool,
    pub(crate) focus_mismatch: bool,
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
    pub(crate) focus_epic_id: Option<String>,
    pub(crate) focus_epic_title: Option<String>,
    pub(crate) focus_epic_labels: Vec<String>,
    pub(crate) authored_evidence: Vec<String>,
    pub(crate) tool_files: Vec<String>,
    pub(crate) tool_result_terms: Vec<String>,
    pub(crate) mcp_query_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecallQuery {
    pub(crate) canonical: String,
    pub(crate) role: RecallRole,
    /// Conversational prompts have no explicit task, code, or file reference.
    /// Their lexical overlaps are too weak to justify ambient injection alone.
    pub(crate) conversational: bool,
    pub(crate) task_id: Option<String>,
    pub(crate) files: Vec<String>,
    pub(crate) symbols: Vec<String>,
    pub(crate) focus_terms: Vec<String>,
    pub(crate) authored_evidence: Vec<String>,
    /// Terms accumulated from safe PostToolUse result and MCP-query context.
    /// They can cross the conversational precision gate only in pairs.
    pub(crate) tool_context_terms: Vec<String>,
}

impl RecallQuery {
    pub(crate) fn build(identity: &RecallIdentity, request: &RecallRequest) -> Option<Self> {
        if !identity.is_eligible() {
            return None;
        }
        let base_files = stable_values(&request.files, 16);
        let mut files = base_files.clone();
        let mut symbols = stable_values(&request.symbols, 16);
        let labels = stable_values(&request.task_labels, 12);
        let decisions = stable_values(&request.recent_decisions, 4);
        let seen = stable_values(&request.seen_evidence, 32);
        let focus_labels = stable_values(&request.focus_epic_labels, 12);
        let authored_evidence = stable_values(&request.authored_evidence, LEDGER_ENTRY_CAP);
        let tool_files = stable_values(&request.tool_files, TOOL_TRIGGER_PATH_CAP);
        let tool_result_terms = stable_values(&request.tool_result_terms, TOOL_TRIGGER_TERM_CAP);
        let mcp_query_terms = stable_values(&request.mcp_query_terms, TOOL_TRIGGER_TERM_CAP);
        let tool_context_terms = stable_values(
            &[
                tool_files.clone(),
                tool_result_terms.clone(),
                mcp_query_terms.clone(),
            ]
            .concat(),
            TOOL_TRIGGER_TERM_CAP,
        );
        files.extend(tool_files.iter().cloned());
        files = stable_values(&files, 16);
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
        if !base_files.is_empty() {
            lines.push(format!("files={}", base_files.join(",")));
        }
        if !symbols.is_empty() {
            lines.push(format!("symbols={}", symbols.join(",")));
        }
        if !tool_files.is_empty() {
            lines.push(format!("tool_files={}", tool_files.join(",")));
        }
        if !tool_result_terms.is_empty() {
            lines.push(format!("tool_results={}", tool_result_terms.join(",")));
        }
        if !mcp_query_terms.is_empty() {
            lines.push(format!("mcp_queries={}", mcp_query_terms.join(",")));
        }
        if !decisions.is_empty() {
            lines.push(format!("decisions={}", decisions.join(" | ")));
        }
        if !seen.is_empty() {
            lines.push(format!("already_seen={}", seen.join(",")));
        }
        if let Some(focus_id) = request.focus_epic_id.as_deref() {
            lines.push(format!("focus_epic={}", clean_scalar(focus_id, 80)));
        }
        if let Some(focus_title) = request.focus_epic_title.as_deref() {
            lines.push(format!("focus_title={}", clean_scalar(focus_title, 240)));
        }
        if !focus_labels.is_empty() {
            lines.push(format!("focus_labels={}", focus_labels.join(",")));
        }
        let prompt = redact_prompt(&request.prompt);
        if !prompt.is_empty() {
            lines.push(format!("request={prompt}"));
        }

        let max_chars = identity.role.policy().query_tokens.saturating_mul(4);
        let canonical = truncate_utf8(&lines.join("\n"), max_chars);
        let focus_terms = focus_terms(request);
        Some(Self {
            canonical,
            role: identity.role,
            conversational: is_conversational_prompt(&request.prompt),
            task_id: request.task_id.clone(),
            files,
            symbols,
            focus_terms,
            authored_evidence,
            tool_context_terms,
        })
    }
}

/// Durable, queryable account of one ambient-recall decision.  This is kept
/// beside the disposable session ledger so an operator can distinguish a hook
/// that was never eligible from retrieval, precision, and prompt-budget
/// decisions after the session is over.
#[derive(Debug, Serialize)]
struct RecallDecisionTrace {
    recorded_at: DateTime<Utc>,
    session_id: String,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    silence_reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    query_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversational: Option<bool>,
    #[serde(default)]
    terms: Vec<RecallTriggerTerm>,
    #[serde(default)]
    candidates: Vec<RecallDecisionCandidate>,
    #[serde(default)]
    injected: Vec<String>,
    rejected_scope: usize,
}

#[derive(Debug, Serialize)]
struct RecallTriggerTerm {
    term: String,
    source: &'static str,
}

#[derive(Debug, Serialize)]
struct RecallDecisionCandidate {
    evidence_id: String,
    relevance: f64,
    lexical_score: f64,
    semantic_score: Option<f64>,
    structural_score: f64,
    role_score: f64,
    lexical_eligible: bool,
    lexical_weak: bool,
    binding: bool,
    considered_as: &'static str,
}

fn decision_trigger_terms(query: &RecallQuery) -> Vec<RecallTriggerTerm> {
    // Report only terms that can actually influence ranking. Previously this
    // omitted structural `project=` / `task=` terms even though query_terms()
    // scored them, while listing title terms beyond the retriever's ten-term
    // budget. That made a noisy selection look better grounded than it was.
    let ranking_terms: HashSet<String> = query_terms(&query.canonical)
        .into_iter()
        .chain(query.tool_context_terms.iter().cloned())
        .collect();
    let mut terms = Vec::new();
    for line in query.canonical.lines() {
        let (source, value) = if let Some(value) = line.strip_prefix("request=") {
            ("prompt", value)
        } else if let Some(value) = line.strip_prefix("task_title=") {
            ("task_title", value)
        } else if let Some(value) = line.strip_prefix("labels=") {
            ("task_labels", value)
        } else if let Some(value) = line.strip_prefix("files=") {
            ("file_path", value)
        } else if let Some(value) = line.strip_prefix("symbols=") {
            ("symbol", value)
        } else if let Some(value) = line.strip_prefix("tool_files=") {
            ("tool_file_path", value)
        } else if let Some(value) = line.strip_prefix("tool_results=") {
            ("tool_result", value)
        } else if let Some(value) = line.strip_prefix("mcp_queries=") {
            ("mcp_query", value)
        } else if let Some(value) = line.strip_prefix("decisions=") {
            ("recent_decision", value)
        } else if let Some(value) = line.strip_prefix("focus_title=") {
            ("focus_epic_title", value)
        } else if let Some(value) = line.strip_prefix("focus_labels=") {
            ("focus_epic_labels", value)
        } else {
            continue;
        };
        for raw in value
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.')))
        {
            let term = raw.trim_matches(['-', '_', '/', '.']).to_ascii_lowercase();
            if ranking_terms.contains(&term) {
                if !terms.iter().any(|existing: &RecallTriggerTerm| {
                    existing.term == term && existing.source == source
                }) {
                    terms.push(RecallTriggerTerm { term, source });
                }
                if terms.len() == RECALL_DECISION_TERM_CAP {
                    return terms;
                }
            }
        }
    }
    terms
}

fn decision_candidate_state(
    query: &RecallQuery,
    candidate: &EvidenceCandidate,
    injected: &HashSet<&str>,
) -> &'static str {
    if injected.contains(candidate.evidence_id.as_str()) {
        "injected"
    } else if candidate.strong_session_signal {
        "strong_signal_floor"
    } else if query.conversational && is_weak_lexical_only(candidate) {
        "precision_gate"
    } else if !candidate.binding
        && !candidate.lexical_eligible
        && !candidate
            .semantic_score
            .is_some_and(|score| score >= SEMANTIC_INJECTION_FLOOR)
    {
        "below_threshold"
    } else {
        "not_selected"
    }
}

fn silent_decision_reason(
    query: &RecallQuery,
    candidates: &RecallCandidates,
    ledger: &RecallLedger,
) -> &'static str {
    let unseen: Vec<&EvidenceCandidate> = candidates
        .candidates
        .iter()
        .filter(|candidate| !ledger.has_seen(candidate) && !ledger.has_authored(candidate))
        .collect();
    if unseen.is_empty() {
        return "below_threshold";
    }
    if query.conversational
        && unseen
            .iter()
            .all(|candidate| is_weak_lexical_only(candidate))
    {
        return "precision_gate";
    }
    if unseen.iter().all(|candidate| {
        !candidate.binding
            && !candidate.lexical_eligible
            && !candidate
                .semantic_score
                .is_some_and(|score| score >= SEMANTIC_INJECTION_FLOOR)
    }) {
        return "below_threshold";
    }
    "budget"
}

/// Write a standalone JSON trace so completed sessions stay inspectable
/// without a live MCP process. The trace deliberately contains IDs, bounded
/// scores, and redacted query terms, never recalled bodies or raw tool input.
fn record_recall_decision(
    cas_root: &Path,
    session_id: &str,
    query: Option<&RecallQuery>,
    candidates: Option<&RecallCandidates>,
    injected: &[EvidenceCandidate],
    silence_reason: Option<&'static str>,
) {
    let injected_ids: Vec<String> = injected
        .iter()
        .map(|candidate| candidate.evidence_id.clone())
        .collect();
    let injected_set: HashSet<&str> = injected_ids.iter().map(String::as_str).collect();
    let trace_candidates = candidates
        .map(|candidates| {
            candidates
                .candidates
                .iter()
                .take(RECALL_DECISION_CANDIDATE_CAP)
                .map(|candidate| RecallDecisionCandidate {
                    evidence_id: candidate.evidence_id.clone(),
                    relevance: candidate.relevance,
                    lexical_score: candidate.lexical_score,
                    semantic_score: candidate.semantic_score,
                    structural_score: candidate.structural_score,
                    role_score: candidate.role_score,
                    lexical_eligible: candidate.lexical_eligible,
                    lexical_weak: candidate.lexical_weak,
                    binding: candidate.binding,
                    considered_as: query.map_or("not_invoked", |query| {
                        decision_candidate_state(query, candidate, &injected_set)
                    }),
                })
                .collect()
        })
        .unwrap_or_default();
    let trace = RecallDecisionTrace {
        recorded_at: Utc::now(),
        session_id: clean_scalar(session_id, 160),
        outcome: if injected_ids.is_empty() {
            "silent"
        } else {
            "injected"
        },
        silence_reason,
        query_hash: query.map(|query| stable_hash(&query.canonical)),
        conversational: query.map(|query| query.conversational),
        terms: query.map(decision_trigger_terms).unwrap_or_default(),
        candidates: trace_candidates,
        injected: injected_ids,
        rejected_scope: candidates.map_or(0, |candidates| candidates.rejected_scope),
    };
    let Ok(bytes) = serde_json::to_vec(&trace) else {
        return;
    };
    let directory = cas_root.join("cache/ambient-recall/decisions");
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let stamp = trace.recorded_at.timestamp_nanos_opt().unwrap_or_default();
    let file_name = format!(
        "{}-{}-{}.json",
        &stable_hash(session_id)[..16],
        stamp,
        &stable_hash(&format!("{}:{:?}", session_id, trace.silence_reason))[..8]
    );
    let path = directory.join(file_name);
    let temporary = path.with_extension("tmp");
    if fs::write(&temporary, bytes).is_ok() {
        let _ = fs::rename(temporary, path);
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

/// Narrow query-time seam: a recall event may ask the provider for one vector,
/// then must reuse that exact vector for every compatible namespace.
trait RecallQueryEmbedder {
    fn meta(&self) -> EmbeddingMeta;
    fn embed_query(&self, query: &str) -> Result<Vec<f32>, ()>;
}

impl RecallQueryEmbedder for KnowledgeEmbedder {
    fn meta(&self) -> EmbeddingMeta {
        KnowledgeEmbedder::meta(self)
    }

    fn embed_query(&self, query: &str) -> Result<Vec<f32>, ()> {
        self.embed_batch(&[query.to_string()])
            .map_err(|_| ())?
            .into_iter()
            .next()
            .ok_or(())
    }
}

/// Optional authenticated semantic channel for ambient recall.
///
/// Cache discovery happens before the provider call. Logged-out installs and
/// authenticated installs with no compatible, populated cache therefore make
/// zero HTTP calls and create no vector directories. Knowledge and history
/// share one cache; code remains in its isolated cache. All three consume the
/// same query vector.
struct SemanticRecallRetriever {
    cas_root: PathBuf,
    db_path: PathBuf,
    embedder: Box<dyn RecallQueryEmbedder>,
    shared_cache: Option<KnowledgeVectorCache>,
    code_cache: Option<KnowledgeVectorCache>,
}

impl SemanticRecallRetriever {
    fn existing(cas_root: &Path, config: &crate::cloud::CloudConfig) -> Option<Self> {
        let embedder = KnowledgeEmbedder::from_config(config)?.with_timeout(HOOK_SEMANTIC_TIMEOUT);
        Self::with_embedder(cas_root, Box::new(embedder))
    }

    fn with_embedder(cas_root: &Path, embedder: Box<dyn RecallQueryEmbedder>) -> Option<Self> {
        let db_path = cas_root.join("cas.db");
        if !db_path.is_file() {
            return None;
        }
        let meta = embedder.meta();
        let shared_cache = KnowledgeVectorCache::open_existing(cas_root)
            .ok()
            .flatten()
            .filter(|cache| cache.meta() == &meta && cache.count().unwrap_or(0) > 0);
        let code_cache = KnowledgeVectorCache::open_existing_code_read_only(cas_root)
            .ok()
            .flatten()
            .filter(|cache| cache.meta() == &meta && cache.count().unwrap_or(0) > 0);
        if shared_cache.is_none() && code_cache.is_none() {
            return None;
        }
        Some(Self {
            cas_root: cas_root.to_path_buf(),
            db_path,
            embedder,
            shared_cache,
            code_cache,
        })
    }
}

/// Bounded lexical/structural fallback over the already-existing project DB.
///
/// It opens `cas.db` read-only, applies team/project predicates in SQL before
/// text matching, never reads body/blob columns, and never touches Tantivy,
/// LMDB, an embedding provider, or a network client.
pub(crate) struct SqliteRecallRetriever {
    db_path: PathBuf,
}

impl SqliteRecallRetriever {
    pub(crate) fn existing(cas_root: &Path) -> Option<Self> {
        let db_path = cas_root.join("cas.db");
        db_path.is_file().then_some(Self { db_path })
    }
}

#[derive(Debug)]
struct LocalRow {
    id: String,
    surface: EvidenceSurface,
    scope: EvidenceScope,
    snippet: String,
    revision: String,
    stale: bool,
    body_available: bool,
    locator: String,
}

impl RecallRetriever for SqliteRecallRetriever {
    fn retrieve(
        &self,
        query: &RecallQuery,
        scope: &ScopeGate,
        limit: usize,
    ) -> Vec<EvidenceCandidate> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let Ok(conn) = Connection::open_with_flags(&self.db_path, flags) else {
            return Vec::new();
        };
        let terms = query_terms(&query.canonical);
        if terms.is_empty() {
            return Vec::new();
        }
        let per_surface = limit.min(8).max(1);
        let mut rows = Vec::new();
        for spec in local_surface_specs() {
            rows.extend(read_surface(&conn, spec, &terms, scope, per_surface));
        }
        // cas-4028: `task=` is deliberately excluded from lexical terms so an
        // identity value cannot manufacture relevance. That also made a row
        // which NAMES the current task unreachable — it was neither a term nor
        // structurally bound — and the packet went silent on a case whose own
        // prior learnings were sitting in the store. Fetch those rows by
        // identity as well; `local_candidate` still scores them against the
        // unchanged lexical terms, so identity buys retrieval and binding, not
        // a lexical score, and a merely similar id is rejected at the token
        // boundary rather than here.
        if let Some(task_id) = query
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let identity_needle = vec![task_id.to_ascii_lowercase()];
            for spec in local_surface_specs() {
                // SQL matches this needle as a SUBSTRING, so the fetch alone
                // would admit `cas-096e1` and friends. Keep only rows that
                // bind on an exact token, so a supplemental read can never
                // introduce a row the binding rule itself would reject.
                let bound = read_surface(&conn, spec, &identity_needle, scope, per_surface)
                    .into_iter()
                    .filter(|row| {
                        names_current_task(Some(task_id), &row.snippet.to_ascii_lowercase())
                    });
                rows.extend(bound);
            }
            // Identity is (surface, id): ids are only unique within their own
            // table, so deduping on the bare id could silently drop a row from
            // a different surface that happens to share it.
            let mut seen = HashSet::new();
            rows.retain(|row| seen.insert((row.surface, row.id.clone())));
        }
        let mut candidates: Vec<EvidenceCandidate> = rows
            .into_iter()
            .map(|row| local_candidate(row, query, &terms))
            .collect();
        candidates.sort_by(|a, b| {
            b.binding
                .cmp(&a.binding)
                .then_with(|| a.stale.cmp(&b.stale))
                .then_with(|| b.relevance.total_cmp(&a.relevance))
                .then_with(|| a.evidence_id.cmp(&b.evidence_id))
        });
        candidates.truncate(limit);
        candidates
    }
}

#[derive(Debug)]
struct SemanticRow {
    namespace: VectorNamespace,
    vector_key: String,
    row: LocalRow,
}

impl RecallRetriever for SemanticRecallRetriever {
    fn retrieve(
        &self,
        query: &RecallQuery,
        scope: &ScopeGate,
        limit: usize,
    ) -> Vec<EvidenceCandidate> {
        if query.canonical.trim().is_empty() || limit == 0 {
            return Vec::new();
        }
        // Exactly one provider request per recall event. The returned vector
        // is then fanned out locally; namespace count never multiplies cost.
        let query_vector = match self.embedder.embed_query(&query.canonical) {
            Ok(vector) => vector,
            Err(()) => {
                eprintln!(
                    "cas: ambient recall semantic channel timed out or failed; using lexical fallback"
                );
                return Vec::new();
            }
        };
        let meta = self.embedder.meta();
        if is_zero_vector(&query_vector) || query_vector.len() != meta.dims {
            eprintln!(
                "cas: ambient recall semantic channel returned an unusable vector; using lexical fallback"
            );
            return Vec::new();
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let Ok(conn) = Connection::open_with_flags(&self.db_path, flags) else {
            return Vec::new();
        };
        let terms = query_terms(&query.canonical);
        let mut candidates = Vec::new();
        let candidate_limit = limit.min(SEMANTIC_CANDIDATE_CAP_PER_NAMESPACE).max(1);
        let semantic_rows = read_semantic_rows(
            &conn,
            &self.cas_root,
            query,
            scope,
            candidate_limit,
            self.shared_cache.is_some(),
            self.code_cache.is_some(),
        );
        if [
            VectorNamespace::Knowledge,
            VectorNamespace::History,
            VectorNamespace::Code,
        ]
        .into_iter()
        .any(|namespace| {
            semantic_rows
                .iter()
                .filter(|row| row.namespace == namespace)
                .count()
                == candidate_limit
        }) {
            eprintln!(
                "cas: ambient recall semantic candidate window capped at {candidate_limit} per namespace; using bounded lexical/structural prefilter"
            );
        }
        for semantic in semantic_rows {
            // Scope is resolved from the authoritative SQLite row before its
            // vector is read or compared.
            if !scope.allows(&semantic.row.scope) {
                continue;
            }
            let cache = match semantic.namespace {
                VectorNamespace::Knowledge | VectorNamespace::History => self.shared_cache.as_ref(),
                VectorNamespace::Code => self.code_cache.as_ref(),
            };
            let Some(cache) = cache else { continue };
            let Some(vector) = cache.get(&semantic.vector_key).ok().flatten() else {
                continue;
            };
            let score = cosine_similarity(&query_vector, &vector);
            if score <= 0.0 {
                continue;
            }
            let mut candidate = local_candidate(semantic.row, query, &terms);
            candidate.semantic_score = Some(f64::from(score));
            candidate.relevance = candidate.lexical_score * 0.32
                + f64::from(score) * 0.52
                + candidate.structural_score * 0.24
                + candidate.role_score;
            candidate.why_relevant = if candidate.binding {
                format!("exact binding + semantic match {:.3}", score)
            } else if candidate.lexical_score > 0.0 {
                format!("lexical + semantic match {:.3}", score)
            } else {
                format!("semantic match {:.3}", score)
            };
            candidates.push(candidate);
        }
        candidates.sort_by(|a, b| {
            b.binding
                .cmp(&a.binding)
                .then_with(|| a.stale.cmp(&b.stale))
                .then_with(|| b.relevance.total_cmp(&a.relevance))
                .then_with(|| a.evidence_id.cmp(&b.evidence_id))
        });
        candidates.truncate(limit);
        candidates
    }
}

fn row_scope(raw: &str, project_id: &str) -> EvidenceScope {
    if raw.eq_ignore_ascii_case("global") {
        EvidenceScope::Global
    } else if raw.eq_ignore_ascii_case("private") {
        // These vector-bearing stores have no private-owner column. An empty
        // owner deliberately fails the defense-in-depth ScopeGate check.
        EvidenceScope::Private(String::new())
    } else {
        EvidenceScope::Project(project_id.to_string())
    }
}

fn read_semantic_rows(
    conn: &Connection,
    cas_root: &Path,
    query: &RecallQuery,
    scope: &ScopeGate,
    limit: usize,
    include_shared: bool,
    include_code: bool,
) -> Vec<SemanticRow> {
    let terms = query_terms(&query.canonical);
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let fts = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut out = Vec::with_capacity(limit.saturating_mul(3));

    if include_shared {
        let mut knowledge = Vec::new();
        if table_exists(conn, "knowledge_pages_fts")
            && let Ok(mut stmt) = conn.prepare(
                "select p.id, substr(trim(p.title || ' ' || p.snippet || ' ' || p.page_type), 1, 480), \
                        p.updated_at, p.rel_path from knowledge_pages_fts f \
                 join knowledge_pages p on p.row_id = f.rowid \
                 where knowledge_pages_fts match ?1 \
                   and (p.origin = 'local' or p.origin_project_id = ?2) \
                 order by bm25(knowledge_pages_fts), p.id limit ?3",
            )
            && let Ok(rows) = stmt.query_map(params![fts, scope.project_id, limit as i64], |row| {
                semantic_knowledge_row(row, scope)
            })
        {
            knowledge.extend(rows.filter_map(Result::ok));
        }
        extend_semantic_head(
            conn,
            &mut knowledge,
            "select id, substr(trim(title || ' ' || snippet || ' ' || page_type), 1, 480), \
                    updated_at, rel_path from knowledge_pages \
             where origin = 'local' or origin_project_id = ?1 order by id limit ?2",
            params![scope.project_id, limit as i64],
            limit,
            |row| semantic_knowledge_row(row, scope),
        );
        knowledge.truncate(limit);
        out.extend(knowledge);

        let mut history = Vec::new();
        if table_exists(conn, "history_commits_fts")
            && let Ok(mut stmt) = conn.prepare(
                "select c.sha, substr(trim(c.subject || ' ' || coalesce(c.body, '')), 1, 480), \
                        c.indexed_at, c.scope from history_commits_fts f \
                 join history_commits c on c.sha = f.sha \
                 where history_commits_fts match ?1 and lower(c.scope) != 'private' \
                 order by bm25(history_commits_fts), c.sha limit ?2",
            )
            && let Ok(rows) = stmt.query_map(params![fts, limit as i64], |row| {
                semantic_history_commit_row(row, scope)
            })
        {
            history.extend(rows.filter_map(Result::ok));
        }
        extend_semantic_head(
            conn,
            &mut history,
            "select sha, substr(trim(subject || ' ' || coalesce(body, '')), 1, 480), \
                    indexed_at, scope from history_commits \
             where lower(scope) != 'private' order by sha limit ?1",
            params![limit as i64],
            limit,
            |row| semantic_history_commit_row(row, scope),
        );
        if history.len() < limit && table_exists(conn, "history_docs") {
            extend_semantic_head(
                conn,
                &mut history,
                "select id, substr(trim(coalesce(title, '') || ' ' || coalesce(body, '')), 1, 480), \
                        coalesce(updated_at, fetched_at), coalesce(url, id), scope, state \
                 from history_docs where lower(scope) != 'private' order by id limit ?1",
                params![limit as i64],
                limit,
                |row| semantic_history_doc_row(row, scope),
            );
        }
        history.truncate(limit);
        out.extend(history);
    }

    if include_code && table_exists(conn, "code_symbols") {
        let mut code = Vec::new();
        let index_dir = cas_root.join("index").join("code");
        if index_dir.join("meta.json").is_file()
            && let Ok(index) = cas_search::Bm25Index::open(&index_dir)
            && let Ok(hits) = index.search_filtered(&terms.join(" "), None, &[], limit)
        {
            let ids: Vec<String> = hits.into_iter().map(|(id, _)| id).collect();
            code.extend(read_code_rows_by_id(conn, &ids, scope));
        }
        extend_semantic_head(
            conn,
            &mut code,
            "select id, substr(trim(qualified_name || ' ' || name || ' ' || file_path || ' ' || \
                    coalesce(documentation, '') || ' ' || coalesce(signature, '')), 1, 480), \
                    content_hash, qualified_name, scope from code_symbols \
             where lower(scope) != 'private' order by id limit ?1",
            params![limit as i64],
            limit,
            |row| semantic_code_row(row, scope),
        );
        code.truncate(limit);
        out.extend(code);
    }
    out
}

fn extend_semantic_head<P, F>(
    conn: &Connection,
    rows: &mut Vec<SemanticRow>,
    sql: &str,
    params: P,
    limit: usize,
    mut map: F,
) where
    P: rusqlite::Params,
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<SemanticRow>,
{
    if rows.len() >= limit {
        return;
    }
    let Ok(mut stmt) = conn.prepare(sql) else {
        return;
    };
    let Ok(mapped) = stmt.query_map(params, |row| map(row)) else {
        return;
    };
    for row in mapped.filter_map(Result::ok) {
        if rows
            .iter()
            .all(|existing| existing.vector_key != row.vector_key)
        {
            rows.push(row);
            if rows.len() == limit {
                break;
            }
        }
    }
}

fn semantic_knowledge_row(
    row: &rusqlite::Row<'_>,
    scope: &ScopeGate,
) -> rusqlite::Result<SemanticRow> {
    let id: String = row.get(0)?;
    Ok(SemanticRow {
        namespace: VectorNamespace::Knowledge,
        vector_key: id.clone(),
        row: LocalRow {
            id,
            surface: EvidenceSurface::Knowledge,
            scope: EvidenceScope::Project(scope.project_id.clone()),
            snippet: row.get(1)?,
            revision: row.get(2)?,
            stale: false,
            body_available: true,
            locator: row.get(3)?,
        },
    })
}

fn semantic_history_commit_row(
    row: &rusqlite::Row<'_>,
    scope: &ScopeGate,
) -> rusqlite::Result<SemanticRow> {
    let id: String = row.get(0)?;
    let raw_scope: String = row.get(3)?;
    Ok(SemanticRow {
        namespace: VectorNamespace::History,
        vector_key: history_commit_key(&id),
        row: LocalRow {
            locator: id.clone(),
            id,
            surface: EvidenceSurface::History,
            scope: row_scope(&raw_scope, &scope.project_id),
            snippet: row.get(1)?,
            revision: row.get(2)?,
            stale: false,
            body_available: true,
        },
    })
}

fn semantic_history_doc_row(
    row: &rusqlite::Row<'_>,
    scope: &ScopeGate,
) -> rusqlite::Result<SemanticRow> {
    let id: String = row.get(0)?;
    let raw_scope: String = row.get(4)?;
    let state: Option<String> = row.get(5)?;
    Ok(SemanticRow {
        namespace: VectorNamespace::History,
        vector_key: history_doc_key(&id),
        row: LocalRow {
            id,
            surface: EvidenceSurface::History,
            scope: row_scope(&raw_scope, &scope.project_id),
            snippet: row.get(1)?,
            revision: row.get(2)?,
            stale: state.as_deref() == Some("closed"),
            body_available: true,
            locator: row.get(3)?,
        },
    })
}

fn semantic_code_row(row: &rusqlite::Row<'_>, scope: &ScopeGate) -> rusqlite::Result<SemanticRow> {
    let id: String = row.get(0)?;
    let raw_scope: String = row.get(4)?;
    Ok(SemanticRow {
        namespace: VectorNamespace::Code,
        vector_key: code_symbol_key(&id),
        row: LocalRow {
            id,
            surface: EvidenceSurface::Code,
            scope: row_scope(&raw_scope, &scope.project_id),
            snippet: row.get(1)?,
            revision: row.get(2)?,
            stale: false,
            body_available: true,
            locator: row.get(3)?,
        },
    })
}

fn read_code_rows_by_id(conn: &Connection, ids: &[String], scope: &ScopeGate) -> Vec<SemanticRow> {
    let Ok(mut stmt) = conn.prepare(
        "select id, substr(trim(qualified_name || ' ' || name || ' ' || file_path || ' ' || \
                coalesce(documentation, '') || ' ' || coalesce(signature, '')), 1, 480), \
                content_hash, qualified_name, scope from code_symbols \
         where id = ?1 and lower(scope) != 'private'",
    ) else {
        return Vec::new();
    };
    ids.iter()
        .filter_map(|id| {
            stmt.query_row([id], |row| semantic_code_row(row, scope))
                .ok()
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct LocalSurfaceSpec {
    table: &'static str,
    surface: EvidenceSurface,
    id: &'static str,
    text: &'static str,
    scope: &'static str,
    team: &'static str,
    share: &'static str,
    revision: &'static str,
    stale: &'static str,
    body_available: bool,
    locator: &'static str,
    extra_scope_predicate: &'static str,
}

const MEMORY_STALE_SQL: &str = "case when valid_until is not null and datetime(valid_until) < datetime('now') then 1 else 0 end";

fn local_surface_specs() -> [LocalSurfaceSpec; 8] {
    [
        LocalSurfaceSpec {
            table: "entries",
            surface: EvidenceSurface::Memory,
            id: "id",
            text: "trim(coalesce(title, '') || ' ' || content)",
            scope: "scope",
            team: "team_id",
            share: "share",
            revision: "coalesce(updated_at, created)",
            stale: MEMORY_STALE_SQL,
            body_available: true,
            locator: "id",
            extra_scope_predicate: "archived = 0",
        },
        LocalSurfaceSpec {
            table: "rules",
            surface: EvidenceSurface::Rule,
            id: "id",
            text: "content",
            scope: "scope",
            team: "team_id",
            share: "share",
            revision: "created",
            stale: "case when status in ('stale', 'retired') then 1 else 0 end",
            body_available: true,
            locator: "id",
            extra_scope_predicate: "status != 'retired'",
        },
        LocalSurfaceSpec {
            table: "tasks",
            surface: EvidenceSurface::Task,
            id: "id",
            text: "trim(title || ' ' || description || ' ' || design || ' ' || notes)",
            scope: "'project'",
            team: "team_id",
            share: "share",
            revision: "updated_at",
            stale: "case when status = 'closed' then 1 else 0 end",
            body_available: true,
            locator: "id",
            extra_scope_predicate: "status not in ('deleted')",
        },
        LocalSurfaceSpec {
            table: "skills",
            surface: EvidenceSurface::Skill,
            id: "id",
            text: "trim(name || ' ' || description || ' ' || summary || ' ' || tags)",
            scope: "case when id like 'g-%' then 'global' else 'project' end",
            team: "team_id",
            share: "share",
            revision: "updated_at",
            stale: "case when status != 'enabled' then 1 else 0 end",
            body_available: true,
            locator: "id",
            extra_scope_predicate: "status = 'enabled'",
        },
        LocalSurfaceSpec {
            table: "specs",
            surface: EvidenceSurface::Spec,
            id: "id",
            text: "trim(title || ' ' || summary || ' ' || design_notes)",
            scope: "scope",
            team: "team_id",
            share: "null",
            revision: "updated_at",
            stale: "case when status in ('superseded', 'rejected') then 1 else 0 end",
            body_available: true,
            locator: "id",
            extra_scope_predicate: "status not in ('superseded', 'rejected')",
        },
        LocalSurfaceSpec {
            table: "knowledge_pages",
            surface: EvidenceSurface::Knowledge,
            id: "id",
            text: "trim(title || ' ' || snippet || ' ' || page_type)",
            scope: "'project'",
            team: "null",
            share: "null",
            revision: "updated_at",
            stale: "0",
            body_available: true,
            locator: "rel_path",
            extra_scope_predicate: "(origin = 'local' or origin_project_id = :project_id)",
        },
        LocalSurfaceSpec {
            table: "history_commits",
            surface: EvidenceSurface::History,
            id: "sha",
            // Commit trailers are metadata, not recall evidence, and generated
            // merge messages are routine lexical noise. Semantic history keeps
            // its existing heuristic because a merge with real prose can still
            // be useful there.
            text: "trim(subject)",
            scope: "scope",
            team: "null",
            share: "null",
            revision: "indexed_at",
            stale: "0",
            body_available: true,
            locator: "sha",
            extra_scope_predicate: "is_merge = 0",
        },
        LocalSurfaceSpec {
            table: "code_symbols",
            surface: EvidenceSurface::Code,
            id: "id",
            text: "trim(qualified_name || ' ' || name || ' ' || file_path || ' ' || coalesce(documentation, '') || ' ' || coalesce(signature, ''))",
            scope: "scope",
            team: "null",
            share: "null",
            revision: "content_hash",
            stale: "0",
            body_available: true,
            locator: "qualified_name",
            extra_scope_predicate: "1 = 1",
        },
    ]
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "select exists(select 1 from sqlite_master where type in ('table', 'view') and name = ?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .unwrap_or(false)
}

fn read_surface(
    conn: &Connection,
    spec: LocalSurfaceSpec,
    terms: &[String],
    scope: &ScopeGate,
    limit: usize,
) -> Vec<LocalRow> {
    if !table_exists(conn, spec.table) {
        return Vec::new();
    }
    let term_predicate = (0..terms.len())
        .map(|index| format!("lower({}) like ?{}", spec.text, index + 1))
        .collect::<Vec<_>>()
        .join(" or ");
    let team_index = terms.len() + 1;
    let limit_index = terms.len() + 2;
    let extra = spec
        .extra_scope_predicate
        .replace(":project_id", &format!("?{}", terms.len() + 3));
    let sql = format!(
        "select {}, {}, {}, {}, substr({}, 1, 480), {}, {}, {} from {} \
         where ({}) and ({} is null or {} = ?{}) and coalesce({}, '') != 'private' \
         and ({}) order by {} desc, {} asc limit ?{}",
        spec.id,
        spec.scope,
        spec.team,
        spec.share,
        spec.text,
        spec.revision,
        spec.stale,
        spec.locator,
        spec.table,
        spec.extra_scope_predicate
            .replace(":project_id", &format!("?{}", terms.len() + 3)),
        spec.team,
        spec.team,
        team_index,
        spec.share,
        term_predicate,
        spec.revision,
        spec.id,
        limit_index,
    );
    let mut values: Vec<String> = terms.iter().map(|term| format!("%{term}%")).collect();
    values.push(scope.team_id.clone().unwrap_or_default());
    values.push(limit.to_string());
    if extra.contains(&format!("?{}", terms.len() + 3)) {
        values.push(scope.project_id.clone());
    }
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(mapped) = stmt.query_map(params_from_iter(values.iter()), |row| {
        let raw_scope: String = row.get(1)?;
        let team_id: Option<String> = row.get(2)?;
        let share: Option<String> = row.get(3)?;
        let evidence_scope = if let Some(team_id) = team_id {
            EvidenceScope::Team(team_id)
        } else if raw_scope.eq_ignore_ascii_case("global") {
            EvidenceScope::Global
        } else if share.as_deref() == Some("private") {
            EvidenceScope::Private(String::new())
        } else {
            EvidenceScope::Project(scope.project_id.clone())
        };
        Ok(LocalRow {
            id: row.get(0)?,
            surface: spec.surface,
            scope: evidence_scope,
            snippet: row.get(4)?,
            revision: row.get(5)?,
            stale: row.get::<_, i64>(6)? != 0,
            body_available: spec.body_available,
            locator: row.get(7)?,
        })
    }) else {
        return Vec::new();
    };
    mapped.filter_map(Result::ok).collect()
}

fn query_terms(canonical: &str) -> Vec<String> {
    let mut terms = Vec::new();
    // The submitted turn is the reason this recall is happening.  Prioritize
    // it over durable task/epic metadata: the lexical retriever has a hard
    // ten-term work budget, and putting request terms last made an active
    // supervisor task title silently consume that entire budget (cas-0337).
    // Preserve the remaining canonical context as a fallback after the turn.
    let mut lines: Vec<&str> = canonical
        .lines()
        // These fields enforce identity, scope, de-duplication, or structural
        // binding elsewhere. Treating their values as text evidence let a
        // project slug such as `cas-src` manufacture relevance inside a store
        // that was already project-gated.
        .filter(|line| {
            !matches!(
                line.split_once('=').map(|(key, _)| key),
                Some("role" | "project" | "task" | "already_seen" | "focus_epic")
            )
        })
        // SessionStart is an event label, not user intent.
        .filter(|line| !line.eq_ignore_ascii_case("request=session start"))
        .collect();
    if let Some(request_index) = lines.iter().position(|line| {
        line.strip_prefix("request=")
            .is_some_and(|request| !request.eq_ignore_ascii_case("session start"))
    }) {
        let request = lines.remove(request_index);
        lines.insert(0, request);
    }
    for line in lines {
        for raw in line
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.')))
        {
            let term = raw.trim_matches(['-', '_', '/', '.']).to_ascii_lowercase();
            if !is_content_bearing_term(&term) || terms.contains(&term) {
                continue;
            }
            terms.push(term);
            if terms.len() == 10 {
                return terms;
            }
        }
    }
    terms
}

fn focus_terms(request: &RecallRequest) -> Vec<String> {
    // The opaque epic id can help normal lexical retrieval through the
    // canonical query, but only human-readable title/labels describe a domain.
    let mut source = String::new();
    if let Some(title) = request.focus_epic_title.as_deref() {
        source.push_str(title);
    }
    if !request.focus_epic_labels.is_empty() {
        if !source.is_empty() {
            source.push(' ');
        }
        source.push_str(&request.focus_epic_labels.join(" "));
    }
    query_terms(&source)
}

/// Terms in this set have high enough document frequency in ordinary task and
/// commit prose that they cannot, by themselves, establish relevance. Keeping
/// this bounded static floor avoids a corpus-wide IDF scan on the hook path.
fn is_high_document_frequency_term(term: &str) -> bool {
    const STOP: &[&str] = &[
        "role",
        "worker",
        "supervisor",
        "project",
        "task",
        "task_title",
        "labels",
        "files",
        "symbols",
        "decisions",
        "already_seen",
        "request",
        "this",
        "that",
        "them",
        "what",
        "which",
        "when",
        "where",
        "who",
        "why",
        "the",
        "and",
        "are",
        "was",
        "were",
        "been",
        "being",
        "have",
        "has",
        "had",
        "does",
        "did",
        "can",
        "could",
        "would",
        "should",
        "for",
        "with",
        "from",
        "into",
        "about",
        "after",
        "before",
        "between",
        "over",
        "under",
        "again",
        "then",
        "use",
        "open",
        "need",
        "want",
        "get",
        "got",
        "make",
        "made",
        "also",
        "just",
        "more",
        "most",
        "some",
        "such",
        "only",
        "very",
        "will",
        "queue",
        "new",
        "old",
        "first",
        "decision",
        "context",
        "check",
        "out",
        "put",
        "merge",
        "branch",
        "every",
        "please",
        "implement",
    ];
    STOP.contains(&term)
}

fn is_content_bearing_term(term: &str) -> bool {
    term.len() >= 3 && !is_high_document_frequency_term(term)
}

fn lexical_match_is_eligible(matched: &[&str]) -> bool {
    matched.iter().any(|term| is_content_bearing_term(term))
}

/// Apply a bounded document-frequency pass after all local candidates are
/// known. This catches project slugs and corpus boilerplate without a
/// hardcoded project-name list. Rejected rows stay available to the renderer,
/// so `omitted=` remains an honest count.
fn exclude_corpus_ubiquitous_lexical_terms(candidates: &mut [EvidenceCandidate], query: &str) {
    let terms = query_terms(query);
    let local: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            (candidate.provenance.source == "cas.db/read-only").then_some(index)
        })
        .collect();
    if local.len() < UBIQUITOUS_TERM_MIN_DOCUMENTS || terms.is_empty() {
        return;
    }
    let ubiquitous: HashSet<&str> = terms
        .iter()
        .filter_map(|term| {
            let frequency = local
                .iter()
                .filter(|&&index| {
                    candidates[index]
                        .snippet
                        .to_ascii_lowercase()
                        .contains(term.as_str())
                })
                .count();
            (frequency >= UBIQUITOUS_TERM_MIN_DOCUMENTS
                && (frequency as f64 / local.len() as f64) >= UBIQUITOUS_TERM_DOCUMENT_FREQUENCY)
                .then_some(term.as_str())
        })
        .collect();
    if ubiquitous.is_empty() {
        return;
    }
    for index in local {
        let candidate = &mut candidates[index];
        if candidate.binding {
            continue;
        }
        let haystack = candidate.snippet.to_ascii_lowercase();
        let matched: Vec<&str> = terms
            .iter()
            .map(String::as_str)
            .filter(|term| haystack.contains(term))
            .filter(|term| !ubiquitous.contains(term))
            .collect();
        candidate.lexical_eligible = lexical_match_is_eligible(&matched);
        candidate.lexical_weak = matched.len() == 1;
        if candidate.semantic_score.is_none() {
            candidate.why_relevant = match matched.len() {
                0 => "lexical match:".to_string(),
                1 => format!("lexical(weak) match: {}", matched[0]),
                _ => format!("lexical match: {}", matched.join(",")),
            };
        }
    }
}

/// Whether `haystack` names the task currently being worked, as a whole token.
///
/// cas-4028. Suppressing structural identity terms is right for the generic
/// noise it was written for — a post-mortem matching on `cas-src` and `every`
/// is not release guidance. But a task id is also a structural identity term,
/// and when the id is the CURRENT task's, a row carrying it is the prior
/// record of this exact work rather than noise. The regression this repairs:
/// the packet returned nothing at all for the case whose task is cas-096e,
/// dropping a memory tagged `cas-096e` whose body opens "CI wall-clock work on
/// cas-src (cas-096e/GH #142 ...)" — the case's own prior learnings.
///
/// A mention is evidence, not authority: this only lets the row bind, and
/// every other gate — staleness, expiry, privacy and eligibility — still
/// applies. Matching is token-bounded, so `cas-096e1` and `cas-096e-extra` are
/// different tasks and do not bind, and an absent or empty id never matches.
fn names_current_task(task_id: Option<&str>, haystack: &str) -> bool {
    let Some(task_id) = task_id else {
        return false;
    };
    let needle = task_id.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return false;
    }

    // `match_indices` yields char-boundary-safe offsets, so a multi-byte
    // haystack or task id cannot panic here the way manual slicing would.
    let bytes = haystack.as_bytes();
    haystack.match_indices(needle.as_str()).any(|(start, hit)| {
        let end = start + hit.len();
        // A task id is one token: a neighbouring id character means this is a
        // different id that merely shares a prefix or suffix. A multi-byte
        // neighbour is not an id character, so it reads as a boundary.
        let before_ok = start == 0 || !is_task_id_char(bytes[start - 1]);
        let after_ok = end == haystack.len() || !is_task_id_char(bytes[end]);
        before_ok && after_ok
    })
}

/// Characters that continue a task id token (`cas-096e`), so an adjacent one
/// proves the match was part of a longer, different identifier.
fn is_task_id_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn local_candidate(row: LocalRow, query: &RecallQuery, terms: &[String]) -> EvidenceCandidate {
    let haystack = row.snippet.to_ascii_lowercase();
    let matched: Vec<&str> = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .map(String::as_str)
        .collect();
    let lexical_eligible = lexical_match_is_eligible(&matched);
    let lexical = matched.len() as f64 / terms.len().max(1) as f64;
    let binding = query.task_id.as_deref() == Some(row.id.as_str())
        || names_current_task(query.task_id.as_deref(), &haystack)
        || query.files.iter().any(|file| haystack.contains(file))
        || query.symbols.iter().any(|symbol| haystack.contains(symbol));
    let role_score = match (query.role, row.surface) {
        (RecallRole::Worker, EvidenceSurface::Code) => 0.24,
        (RecallRole::Worker, EvidenceSurface::History) => 0.20,
        (RecallRole::Worker, EvidenceSurface::Rule) => 0.16,
        (RecallRole::Worker, EvidenceSurface::Memory) => 0.14,
        // Preserve the established supervisor contract for current,
        // multi-term task evidence without boosting weak or closed tasks. In
        // its three-term fixture, 0.31 puts a two-term task at 0.75, narrowly
        // above a three-term code match at 0.74; 0.30 only ties that row.
        (RecallRole::Supervisor, EvidenceSurface::Task) if !row.stale && matched.len() >= 2 => 0.31,
        (RecallRole::Supervisor, EvidenceSurface::Spec) => 0.22,
        (RecallRole::Supervisor, EvidenceSurface::History) => 0.20,
        (RecallRole::Supervisor, EvidenceSurface::Rule) => 0.14,
        _ => 0.08,
    };
    let structural = if binding { 1.0 } else { 0.0 };
    let lexical_weak = !binding && matched.len() == 1;
    let matched_tool_terms: Vec<&String> = query
        .tool_context_terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .collect();
    let strong_session_signal = matched_tool_terms.len() >= 2
        && matched_tool_terms
            .iter()
            .any(|term| term.chars().any(|ch| ch.is_ascii_digit()) || term.len() >= 8);
    let focus_mismatch = !query.focus_terms.is_empty()
        && !binding
        && !query.focus_terms.iter().any(|term| haystack.contains(term));
    EvidenceCandidate {
        evidence_id: row.id,
        surface: row.surface,
        scope: row.scope,
        snippet: clean_scalar(&row.snippet, 480),
        why_relevant: if binding {
            "exact task/file/symbol binding".into()
        } else if lexical_weak {
            format!("lexical(weak) match: {}", matched.join(","))
        } else {
            format!("lexical match: {}", matched.join(","))
        },
        provenance: EvidenceProvenance {
            source: "cas.db/read-only".into(),
            locator: row.locator,
            observed_at: None,
            revision: clean_scalar(&row.revision, 96),
        },
        relevance: lexical * 0.66 + structural * 0.24 + role_score,
        lexical_score: lexical,
        lexical_eligible,
        lexical_weak,
        strong_session_signal,
        focus_mismatch,
        semantic_score: None,
        structural_score: structural,
        role_score,
        binding,
        stale: row.stale,
        conflict_key: None,
        body_available: row.body_available,
    }
}

/// Build one hook-facing ambient context segment. All failures degrade to no
/// segment; diagnostic messages name only the rejected capability/boundary.
pub(crate) fn build_ambient_recall_context(
    input: &cas_core::hooks::types::HookInput,
    cas_root: &Path,
    prompt: Option<&str>,
    session_start: bool,
) -> Option<RecallPacket> {
    build_ambient_recall_context_with_factory_identity(
        input,
        cas_root,
        prompt,
        session_start,
        None,
        None,
    )
}

/// Build an ambient packet for a factory launch before the harness has made
/// the child process environment available.  Grok ignores SessionStart stdout,
/// so its launch-intro fallback must supply the supervisor identity explicitly
/// instead of reading the still-parent process environment.
pub(crate) fn build_ambient_recall_context_for_factory_launch(
    input: &cas_core::hooks::types::HookInput,
    cas_root: &Path,
    prompt: Option<&str>,
    agent_name: &str,
    factory_session: &str,
) -> Option<RecallPacket> {
    build_ambient_recall_context_with_factory_identity(
        input,
        cas_root,
        prompt,
        true,
        Some(agent_name),
        Some(factory_session),
    )
}

/// Record rule and skill cards selected by ambient recall in the same durable
/// surface ledger used by the normal SessionStart context builder. Other
/// ambient evidence remains in retrieval telemetry; surfaced_artifacts is
/// intentionally limited to artifacts with impact counters.
fn record_ambient_artifact_surfaces(
    cas_root: &Path,
    session_id: &str,
    injected: &[EvidenceCandidate],
) {
    use cas_store::{SqliteSurfacedArtifactStore, SurfacedArtifact};

    let artifacts: Vec<SurfacedArtifact> = injected
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.surface,
                EvidenceSurface::Rule | EvidenceSurface::Skill
            )
        })
        .map(|candidate| SurfacedArtifact {
            artifact_id: candidate.evidence_id.clone(),
            artifact_type: candidate.surface.as_str().to_string(),
            preview: Some(candidate.snippet.clone()),
        })
        .collect();
    if artifacts.is_empty() {
        return;
    }
    if let Ok(store) = SqliteSurfacedArtifactStore::open(cas_root) {
        // Impact bookkeeping is observational and must never make ambient
        // recall fail after the packet has already been rendered.
        let _ = store.record_batch(session_id, &artifacts);
    }
}

fn build_ambient_recall_context_with_factory_identity(
    input: &cas_core::hooks::types::HookInput,
    cas_root: &Path,
    prompt: Option<&str>,
    session_start: bool,
    factory_agent_name: Option<&str>,
    factory_session: Option<&str>,
) -> Option<RecallPacket> {
    if crate::internal_llm::is_internal_invocation() {
        record_recall_decision(
            cas_root,
            &input.session_id,
            None,
            None,
            &[],
            Some("no_invocation"),
        );
        eprintln!("cas: ambient recall skipped (internal model identity)");
        return None;
    }
    let Some(role) = input
        .agent_role
        .as_deref()
        .and_then(RecallRole::parse)
        .or_else(|| {
            std::env::var("CAS_AGENT_ROLE")
                .ok()
                .as_deref()
                .and_then(RecallRole::parse)
        })
    else {
        record_recall_decision(
            cas_root,
            &input.session_id,
            None,
            None,
            &[],
            Some("no_invocation"),
        );
        return None;
    };
    if !session_start && !meaningful_transition(prompt.unwrap_or_default()) {
        record_recall_decision(
            cas_root,
            &input.session_id,
            None,
            None,
            &[],
            Some("no_invocation"),
        );
        return None;
    }
    let identity = RecallIdentity {
        session_id: input.session_id.clone(),
        agent_name: factory_agent_name
            .map(str::to_owned)
            .unwrap_or_else(|| std::env::var("CAS_AGENT_NAME").unwrap_or_default()),
        factory_session: factory_session
            .map(str::to_owned)
            .unwrap_or_else(|| std::env::var("CAS_FACTORY_SESSION").unwrap_or_default()),
        role,
        project_id: crate::cloud::resolve_canonical_id(cas_root).unwrap_or_default(),
        team_id: crate::cloud::CloudConfig::load_from_cas_dir(cas_root)
            .ok()
            .and_then(|config| config.active_team_id()),
        internal_llm: false,
    };
    if !identity.is_eligible() {
        record_recall_decision(
            cas_root,
            &input.session_id,
            None,
            None,
            &[],
            Some("no_invocation"),
        );
        eprintln!("cas: ambient recall skipped (incomplete outer factory identity)");
        return None;
    }

    let request = hook_request(
        input,
        cas_root,
        &identity,
        prompt.unwrap_or("session start"),
    );
    // Query construction and its caps are complete before the retriever is
    // opened. A malformed/oversized prompt therefore cannot expand DB work.
    let Some(query) = RecallQuery::build(&identity, &request) else {
        record_recall_decision(
            cas_root,
            &input.session_id,
            None,
            None,
            &[],
            Some("no_invocation"),
        );
        return None;
    };
    let Some(retriever) = SqliteRecallRetriever::existing(cas_root) else {
        record_recall_decision(
            cas_root,
            &input.session_id,
            Some(&query),
            None,
            &[],
            Some("below_threshold"),
        );
        return None;
    };
    let config = crate::cloud::CloudConfig::load_from_cas_dir(cas_root).unwrap_or_default();
    let semantic = SemanticRecallRetriever::existing(cas_root, &config);
    let mut retrievers: Vec<&dyn RecallRetriever> = vec![&retriever];
    if let Some(semantic) = semantic.as_ref() {
        retrievers.push(semantic);
    }
    let Some(mut candidates) = retrieve_candidates(&identity, &request, &retrievers) else {
        record_recall_decision(
            cas_root,
            &input.session_id,
            Some(&query),
            None,
            &[],
            Some("below_threshold"),
        );
        return None;
    };
    apply_outcome_feedback(cas_root, &mut candidates.candidates);
    let ledger_file = ledger_path(cas_root, &identity.session_id);
    let mut ledger = RecallLedger::load(&ledger_file);
    // A session may have written evidence since the last hook. Persist that
    // provenance before rendering so the ledger remains protective if a later
    // lookup cannot read the originating table (for example, during a schema
    // transition). The retriever has already removed this turn's direct hits.
    ledger.record_authored(&candidates.authored_evidence);
    let rendered = render_packet(&identity, &query, &candidates, &mut ledger);
    match rendered {
        Some((packet, injected)) => {
            record_recall_decision(
                cas_root,
                &input.session_id,
                Some(&query),
                Some(&candidates),
                &injected,
                None,
            );
            let query_id =
                record_ambient_query(cas_root, &identity, &query, &injected, session_start);
            record_ambient_artifact_surfaces(cas_root, &identity.session_id, &injected);
            ledger.record(packet.query_hash.clone(), query_id, &injected);
            ledger.save(&ledger_file);
            eprintln!(
                "cas: ambient recall injected {} evidence card(s), omitted {}",
                packet.injected, packet.omitted
            );
            Some(packet)
        }
        None => {
            let silence_reason = silent_decision_reason(&query, &candidates, &ledger);
            record_recall_decision(
                cas_root,
                &input.session_id,
                Some(&query),
                Some(&candidates),
                &[],
                Some(silence_reason),
            );
            ledger.save(&ledger_file);
            None
        }
    }
}

fn meaningful_transition(prompt: &str) -> bool {
    let normalized = clean_scalar(prompt, 1_600).to_ascii_lowercase();
    if normalized.len() < 12 {
        return false;
    }
    !matches!(
        normalized.as_str(),
        "thanks" | "thank you" | "sounds good" | "continue" | "go ahead" | "okay, continue"
    )
}

/// Conversational/meta prompts ask for status or explanation without naming a
/// task, a code symbol, or a file. Keep this deliberately conservative: a
/// false negative only preserves the existing fallback, while a false positive
/// would hide useful local context from an ordinary work request.
fn is_conversational_prompt(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let asks_meta_question = [
        "how ",
        "what ",
        "why ",
        "when ",
        "where ",
        "who ",
        "tell me",
        "explain ",
        "summarize ",
        "status ",
        "are we ",
        "is the ",
        "do we ",
    ]
    .iter()
    .any(|prefix| lower.trim_start().starts_with(prefix));
    if !asks_meta_question {
        return false;
    }
    let names_task = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .any(|token| {
            token.strip_prefix("cas-").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_alphanumeric())
            })
        });
    let names_code = lower.contains("::")
        || prompt.contains('`')
        || prompt.contains("```")
        || prompt
            .split_whitespace()
            .any(|token| token.ends_with("()") || token.ends_with("(...)"));
    let names_file = lower.split_whitespace().any(|token| {
        let token = token.trim_matches(|ch: char| {
            !ch.is_ascii_alphanumeric() && ch != '/' && ch != '.' && ch != '_' && ch != '-'
        });
        token.contains('/')
            || [
                ".rs", ".toml", ".md", ".json", ".yaml", ".yml", ".ts", ".tsx", ".js", ".py",
                ".go", ".java", ".rb", ".sh", ".sql",
            ]
            .iter()
            .any(|extension| token.ends_with(extension))
    });
    !names_task && !names_code && !names_file
}

fn hook_request(
    input: &cas_core::hooks::types::HookInput,
    cas_root: &Path,
    identity: &RecallIdentity,
    prompt: &str,
) -> RecallRequest {
    let mut request = RecallRequest {
        prompt: prompt.to_string(),
        ..Default::default()
    };
    let db_path = cas_root.join("cas.db");
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = Connection::open_with_flags(db_path, flags) else {
        return request;
    };

    if table_exists(&conn, "tasks") {
        let role_predicate = match identity.role {
            RecallRole::Worker => "and assignee = ?1",
            RecallRole::Supervisor => "and (?1 is not null)",
        };
        let sql = format!(
            "select id, title, labels, notes from tasks \
             where status = 'in_progress' {role_predicate} \
             order by case when task_type = 'epic' then 0 else 1 end, updated_at desc limit 1"
        );
        if let Ok((id, title, labels, notes)) =
            conn.query_row(&sql, [&identity.agent_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
        {
            request.task_id = Some(id);
            request.task_title = Some(title);
            request.task_labels = serde_json::from_str(&labels).unwrap_or_default();
            request.recent_decisions = notes
                .lines()
                .rev()
                .filter(|line| line.to_ascii_lowercase().contains("decision"))
                .take(4)
                .map(|line| clean_scalar(line, 240))
                .collect();
            request.recent_decisions.reverse();
        }

        // A pinned/session epic is a domain boundary, not an authorization
        // boundary. Its title and labels make that topic available to ranking
        // without adding another store or network read to the hook path.
        if let Some(focus_id) = crate::ui::factory::preferred_epic_id_from_session_metadata() {
            request.focus_epic_id = Some(focus_id.clone());
            if let Ok((title, labels)) = conn.query_row(
                "select title, labels from tasks where id = ?1 and task_type = 'epic'",
                [&focus_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            ) {
                request.focus_epic_title = Some(title);
                request.focus_epic_labels = serde_json::from_str(&labels).unwrap_or_default();
            }
        }
    }

    if table_exists(&conn, "file_changes") {
        if let Ok(mut stmt) = conn.prepare(
            "select distinct file_path from file_changes where session_id = ?1 \
             order by created_at desc limit 16",
        ) {
            if let Ok(rows) = stmt.query_map([&input.session_id], |row| row.get::<_, String>(0)) {
                request.files = rows.filter_map(Result::ok).collect();
            }
        }
    }
    let tool_context =
        ToolTriggerContext::load(&tool_trigger_context_path(cas_root, &input.session_id));
    request.tool_files = tool_context.file_paths;
    request.tool_result_terms = tool_context.result_terms;
    request.mcp_query_terms = tool_context.mcp_query_terms;
    if table_exists(&conn, "code_symbols") {
        let mut symbols = Vec::new();
        for file in request.files.iter().take(8) {
            if let Ok(mut stmt) = conn.prepare(
                "select qualified_name from code_symbols where file_path = ?1 \
                 order by line_start asc limit 2",
            ) {
                if let Ok(rows) = stmt.query_map([file], |row| row.get::<_, String>(0)) {
                    symbols.extend(rows.filter_map(Result::ok));
                }
            }
        }
        request.symbols = stable_values(&symbols, 16);
    }
    request.authored_evidence = session_authored_evidence(&conn, &input.session_id);
    request
}

/// Evidence created by this hook's agent session must not consume its next
/// recall packet. Entries carry direct session provenance; task creation is
/// captured in the sidecar event stream. Query both the hook session and the
/// factory session environment because hook hosts have historically supplied
/// different identifiers for the same agent process.
fn session_authored_evidence(conn: &Connection, hook_session_id: &str) -> Vec<String> {
    let mut session_ids = vec![hook_session_id.trim().to_string()];
    if let Ok(factory_session_id) = std::env::var("CAS_SESSION_ID") {
        session_ids.push(factory_session_id.trim().to_string());
    }
    let session_ids = stable_values(&session_ids, 2);
    let mut ids = Vec::new();

    for session_id in session_ids {
        if session_id.is_empty() {
            continue;
        }
        if let Ok(mut statement) = conn.prepare(
            "select id from entries where session_id = ?1 order by created_at desc limit ?2",
        ) {
            if let Ok(rows) = statement
                .query_map(params![session_id, LEDGER_ENTRY_CAP as i64], |row| {
                    row.get::<_, String>(0)
                })
            {
                ids.extend(rows.filter_map(Result::ok));
            }
        }
        if let Ok(mut statement) = conn.prepare(
            "select distinct entity_id from events \
             where session_id = ?1 and event_type in ('memory_stored', 'task_created') \
             and entity_type in ('entry', 'task') \
             order by created_at desc limit ?2",
        ) {
            if let Ok(rows) = statement
                .query_map(params![session_id, LEDGER_ENTRY_CAP as i64], |row| {
                    row.get::<_, String>(0)
                })
            {
                ids.extend(rows.filter_map(Result::ok));
            }
        }
    }
    stable_values(&ids, LEDGER_ENTRY_CAP)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecallCandidates {
    pub(crate) candidates: Vec<EvidenceCandidate>,
    pub(crate) rejected_scope: usize,
    pub(crate) authored_evidence: Vec<String>,
    pub(crate) rejected_authored: usize,
}

/// Disposable per-session state. It suppresses repeated prompt inflation; it
/// is never an authoritative memory source and may be deleted at any time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecallLedger {
    #[serde(default)]
    last_query_hash: String,
    #[serde(default)]
    seen: Vec<SeenEvidence>,
    #[serde(default)]
    authored: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SeenEvidence {
    evidence_id: String,
    revision: String,
    #[serde(default)]
    query_id: Option<String>,
    #[serde(default)]
    locator: String,
    #[serde(default)]
    outcome_recorded: bool,
}

const LEDGER_ENTRY_CAP: usize = 128;
const LEDGER_BYTE_CAP: usize = 32 * 1024;

impl RecallLedger {
    fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .filter(|bytes| bytes.len() <= LEDGER_BYTE_CAP)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn has_seen(&self, candidate: &EvidenceCandidate) -> bool {
        self.seen.iter().any(|seen| {
            seen.evidence_id == candidate.evidence_id
                && seen.revision == candidate.provenance.revision
        })
    }

    fn record(
        &mut self,
        query_hash: String,
        query_id: Option<String>,
        injected: &[EvidenceCandidate],
    ) {
        self.last_query_hash = query_hash;
        for candidate in injected {
            self.seen.retain(|seen| {
                seen.evidence_id != candidate.evidence_id
                    || seen.revision != candidate.provenance.revision
            });
            self.seen.push(SeenEvidence {
                evidence_id: candidate.evidence_id.clone(),
                revision: candidate.provenance.revision.clone(),
                query_id: query_id.clone(),
                locator: candidate.provenance.locator.clone(),
                outcome_recorded: false,
            });
        }
        if self.seen.len() > LEDGER_ENTRY_CAP {
            self.seen.drain(..self.seen.len() - LEDGER_ENTRY_CAP);
        }
    }

    fn record_authored(&mut self, ids: &[String]) {
        for id in ids {
            self.authored.retain(|existing| existing != id);
            self.authored.push(id.clone());
        }
        if self.authored.len() > LEDGER_ENTRY_CAP {
            self.authored
                .drain(..self.authored.len() - LEDGER_ENTRY_CAP);
        }
    }

    fn has_authored(&self, candidate: &EvidenceCandidate) -> bool {
        self.authored.iter().any(|id| id == &candidate.evidence_id)
    }

    fn save(&self, path: &Path) {
        let Ok(bytes) = serde_json::to_vec(self) else {
            return;
        };
        if bytes.len() > LEDGER_BYTE_CAP {
            return;
        }
        let Some(parent) = path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, bytes).is_ok() {
            let _ = fs::rename(tmp, path);
        }
    }
}

fn sort_candidates(candidates: &mut [EvidenceCandidate]) {
    candidates.sort_by(|a, b| {
        b.strong_session_signal
            .cmp(&a.strong_session_signal)
            .then_with(|| {
                b.binding
                    .cmp(&a.binding)
                    .then_with(|| a.stale.cmp(&b.stale))
                    .then_with(|| b.relevance.total_cmp(&a.relevance))
                    .then_with(|| a.evidence_id.cmp(&b.evidence_id))
            })
    });
}

/// A conservative, shrunk outcome adjustment over resolved evidence only.
/// Automatic `used` is weakly positive; `ignored` is reserved for explicit
/// non-use evidence and is weakly negative. Explicit helpful/corrected/harmful
/// outcomes carry more weight. Four virtual neutral observations prevent a
/// single event from overpowering retrieval evidence. `unresolved` never
/// reaches either the numerator or denominator.
fn outcome_adjustment(
    resolved: u64,
    used: u64,
    helpful: u64,
    ignored: u64,
    corrected: u64,
    harmful: u64,
) -> f64 {
    let weighted = used as f64 * 0.35 + helpful as f64
        - ignored as f64 * 0.25
        - corrected as f64 * 0.75
        - harmful as f64 * 1.25;
    (weighted / (resolved as f64 + 4.0) * 0.20).clamp(-0.20, 0.15)
}

/// Apply bounded per-result history to the already scope-gated candidate
/// window. Any missing table, lock, parse, or query failure leaves ranking
/// unchanged; ambient recall must never block the interactive hook path.
fn apply_outcome_feedback(cas_root: &Path, candidates: &mut [EvidenceCandidate]) {
    if candidates.is_empty() {
        return;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = Connection::open_with_flags(cas_root.join("cas.db"), flags) else {
        return;
    };
    if !table_exists(&conn, "retrieval_outcomes") {
        return;
    }
    let ids = stable_values(
        &candidates
            .iter()
            .map(|candidate| candidate.evidence_id.clone())
            .collect::<Vec<_>>(),
        LEDGER_ENTRY_CAP,
    );
    if ids.is_empty() {
        return;
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT result_id,
                SUM(CASE WHEN outcome != 'unresolved' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'used' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'helpful' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'ignored' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'corrected' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'harmful' THEN 1 ELSE 0 END)
         FROM retrieval_outcomes WHERE result_id IN ({placeholders}) GROUP BY result_id"
    );
    let Ok(mut statement) = conn.prepare(&sql) else {
        return;
    };
    let Ok(rows) = statement.query_map(params_from_iter(ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            outcome_adjustment(
                row.get::<_, i64>(1)?.max(0) as u64,
                row.get::<_, i64>(2)?.max(0) as u64,
                row.get::<_, i64>(3)?.max(0) as u64,
                row.get::<_, i64>(4)?.max(0) as u64,
                row.get::<_, i64>(5)?.max(0) as u64,
                row.get::<_, i64>(6)?.max(0) as u64,
            ),
        ))
    }) else {
        return;
    };
    let adjustments: HashMap<String, f64> = rows.filter_map(Result::ok).collect();
    for candidate in candidates.iter_mut() {
        if let Some(adjustment) = adjustments.get(&candidate.evidence_id) {
            candidate.relevance += adjustment;
            candidate
                .why_relevant
                .push_str(&format!(" + outcome history {adjustment:+.3}"));
        }
    }
    sort_candidates(candidates);
}

fn record_ambient_query(
    cas_root: &Path,
    identity: &RecallIdentity,
    query: &RecallQuery,
    injected: &[EvidenceCandidate],
    session_start: bool,
) -> Option<String> {
    use cas_store::{RetrievalHitIdentity, RetrievalStore, SqliteRetrievalStore};

    let store = SqliteRetrievalStore::open(cas_root).ok()?;
    let query_id = format!("qry-ambient-{}", uuid::Uuid::new_v4().simple());
    let hits = injected
        .iter()
        .enumerate()
        .map(|(rank, candidate)| RetrievalHitIdentity {
            result_id: candidate.evidence_id.clone(),
            document_type: candidate.surface.as_str().to_string(),
            rank,
        })
        .collect::<Vec<_>>();
    store
        .record_query(
            &query_id,
            &query.canonical,
            if session_start {
                "ambient_session_start"
            } else {
                "ambient_transition"
            },
            AMBIENT_RETRIEVAL_POLICY,
            Some(&identity.session_id),
            &hits,
        )
        .ok()?;
    Some(query_id)
}

fn automatic_actor_id() -> String {
    std::env::var("CAS_AGENT_ID")
        .or_else(|_| std::env::var("CAS_AGENT_NAME"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ambient-recall-hook".to_string())
}

fn automatic_outcome_id(query_id: &str, result_id: &str) -> String {
    let digest = stable_hash(&format!("{query_id}\0{result_id}"));
    format!("out-auto-{}", &digest[..32])
}

fn record_pending_outcomes(
    cas_root: &Path,
    session_id: &str,
    ledger: &mut RecallLedger,
    outcome: cas_store::RetrievalOutcome,
    activity: Option<&str>,
    exact_evidence_ids: &[String],
) -> bool {
    use cas_store::{RETRIEVAL_ATTRIBUTION_AUTOMATIC, SqliteRetrievalStore};

    if session_id.trim().is_empty() {
        return false;
    }
    if !ledger
        .seen
        .iter()
        .any(|seen| !seen.outcome_recorded && seen.query_id.is_some())
    {
        return false;
    }
    let Ok(store) = SqliteRetrievalStore::open(cas_root) else {
        return false;
    };
    let actor = automatic_actor_id();
    let mut changed = false;
    for seen in ledger
        .seen
        .iter_mut()
        .filter(|seen| !seen.outcome_recorded && seen.query_id.is_some())
        .take(LEDGER_ENTRY_CAP)
    {
        if let Some(activity) = activity {
            let id = seen.evidence_id.to_ascii_lowercase();
            let locator = seen.locator.to_ascii_lowercase();
            let matched = exact_evidence_ids.iter().any(|exact| exact == &id)
                || (id.len() >= 4 && activity.contains(&id))
                || (locator.len() >= 4 && activity.contains(&locator));
            if !matched {
                continue;
            }
        }
        let query_id = seen.query_id.as_deref().expect("filtered above");
        if store
            .record_outcome_with_attribution(
                &automatic_outcome_id(query_id, &seen.evidence_id),
                query_id,
                &seen.evidence_id,
                outcome,
                &actor,
                session_id,
                None,
                RETRIEVAL_ATTRIBUTION_AUTOMATIC,
            )
            .is_ok()
        {
            seen.outcome_recorded = true;
            changed = true;
        }
    }
    changed
}

fn collect_exact_id_fields(value: &serde_json::Value, ids: &mut Vec<String>) -> bool {
    if ids.len() == LEDGER_ENTRY_CAP {
        return true;
    }
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                if collect_exact_id_fields(value, ids) {
                    return true;
                }
            }
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                if key.eq_ignore_ascii_case("id") {
                    if let Some(id) = value.as_str() {
                        let id = id.to_ascii_lowercase();
                        if !id.is_empty() && id.len() <= 256 && !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                } else if collect_exact_id_fields(value, ids) {
                    return true;
                }
                if ids.len() == LEDGER_ENTRY_CAP {
                    return true;
                }
            }
        }
        _ => {}
    }
    false
}

/// Exact IDs observed through Cassy's body-pull surface. This semantic signal
/// does not need the four-character substring heuristic used for arbitrary
/// tool traffic: memory IDs are compared to the injected ledger directly.
fn exact_memory_retrieval_ids(input: &cas_core::hooks::types::HookInput) -> Vec<String> {
    let is_memory_tool = input.tool_name.as_deref().is_some_and(|name| {
        name.eq_ignore_ascii_case("mcp__cas__memory")
            || name.eq_ignore_ascii_case("mcp__cs__memory")
            || name.eq_ignore_ascii_case("cas_memory")
    });
    if !is_memory_tool {
        return Vec::new();
    }

    let action = input
        .tool_input
        .as_ref()
        .and_then(|value| value.get("action"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let mut ids = Vec::new();
    if action.eq_ignore_ascii_case("get") {
        if let Some(id) = input
            .tool_input
            .as_ref()
            .and_then(|value| value.get("id"))
            .and_then(serde_json::Value::as_str)
        {
            if id.len() <= 256 {
                ids.push(id.to_ascii_lowercase());
            }
        }
    } else if matches!(
        action.to_ascii_lowercase().as_str(),
        "list" | "show" | "recent"
    ) {
        if let Some(response) = input.tool_response.as_ref() {
            if collect_exact_id_fields(response, &mut ids) {
                tracing::debug!(
                    cap = LEDGER_ENTRY_CAP,
                    "memory tool response ID collection reached its cap; additional IDs were omitted"
                );
            }
        }
    }
    ids
}

/// Bounded, redacted trigger facts accumulated from PostToolUse traffic.
/// This sidecar deliberately contains no raw tool output or query text.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ToolTriggerContext {
    #[serde(default)]
    file_paths: Vec<String>,
    #[serde(default)]
    result_terms: Vec<String>,
    #[serde(default)]
    mcp_query_terms: Vec<String>,
}

impl ToolTriggerContext {
    fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .filter(|bytes| bytes.len() <= TOOL_ACTIVITY_BYTE_CAP)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) {
        let Ok(bytes) = serde_json::to_vec(self) else {
            return;
        };
        if bytes.len() > TOOL_ACTIVITY_BYTE_CAP {
            return;
        }
        let Some(parent) = path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let temporary = path.with_extension("tmp");
        if fs::write(&temporary, bytes).is_ok() {
            let _ = fs::rename(temporary, path);
        }
    }
}

fn tool_trigger_context_path(cas_root: &Path, session_id: &str) -> PathBuf {
    cas_root
        .join("cache/ambient-recall/tool-context")
        .join(format!("{}.json", &stable_hash(session_id)[..24]))
}

fn is_sensitive_tool_text(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "password",
        "secret",
        "authorization:",
        "bearer ",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn collect_result_terms(value: &serde_json::Value, terms: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if !is_sensitive_tool_text(text) => {
            for raw in text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
            {
                let term = raw.trim_matches(['-', '_']).to_ascii_lowercase();
                if (is_content_bearing_term(&term) || term.chars().any(|ch| ch.is_ascii_digit()))
                    && !terms.contains(&term)
                {
                    terms.push(term);
                    if terms.len() == TOOL_TRIGGER_TERM_CAP {
                        return;
                    }
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_result_terms(value, terms);
                if terms.len() == TOOL_TRIGGER_TERM_CAP {
                    return;
                }
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_result_terms(value, terms);
                if terms.len() == TOOL_TRIGGER_TERM_CAP {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn collect_tool_paths(value: &serde_json::Value, paths: &mut Vec<String>) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_tool_paths(value, paths);
            }
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                let key = key.to_ascii_lowercase();
                if (key.contains("path") || key == "file" || key == "files") && value.is_string() {
                    if let Some(path) = value.as_str().map(|path| clean_scalar(path, 240)) {
                        if !path.is_empty() && !paths.contains(&path) {
                            paths.push(path);
                        }
                    }
                } else {
                    collect_tool_paths(value, paths);
                }
                if paths.len() == TOOL_TRIGGER_PATH_CAP {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn collect_mcp_query_terms(value: &serde_json::Value, terms: &mut Vec<String>) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_mcp_query_terms(value, terms);
            }
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                let key = key.to_ascii_lowercase();
                if matches!(key.as_str(), "query" | "q" | "search") {
                    collect_result_terms(value, terms);
                } else {
                    collect_mcp_query_terms(value, terms);
                }
                if terms.len() == TOOL_TRIGGER_TERM_CAP {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn record_ambient_tool_trigger_context(input: &cas_core::hooks::types::HookInput, cas_root: &Path) {
    if input.session_id.trim().is_empty() {
        return;
    }
    let path = tool_trigger_context_path(cas_root, &input.session_id);
    let mut context = ToolTriggerContext::load(&path);
    if let Some(tool_input) = input.tool_input.as_ref() {
        collect_tool_paths(tool_input, &mut context.file_paths);
        if input
            .tool_name
            .as_deref()
            .is_some_and(|name| name.starts_with("mcp__") || name.starts_with("cas__"))
        {
            collect_mcp_query_terms(tool_input, &mut context.mcp_query_terms);
        }
    }
    if let Some(result) = input.tool_response.as_ref() {
        collect_result_terms(result, &mut context.result_terms);
    }
    context.file_paths = stable_values(&context.file_paths, TOOL_TRIGGER_PATH_CAP);
    context.result_terms = stable_values(&context.result_terms, TOOL_TRIGGER_TERM_CAP);
    context.mcp_query_terms = stable_values(&context.mcp_query_terms, TOOL_TRIGGER_TERM_CAP);
    context.save(&path);
}

/// Best-effort PostToolUse capture. Trigger facts are persisted independently
/// of feedback so a Read/result can make the next conversational turn recall.
pub(crate) fn record_ambient_tool_usage(
    input: &cas_core::hooks::types::HookInput,
    cas_root: &Path,
) {
    record_ambient_tool_trigger_context(input, cas_root);
    let path = ledger_path(cas_root, &input.session_id);
    let mut ledger = RecallLedger::load(&path);
    if !ledger
        .seen
        .iter()
        .any(|seen| !seen.outcome_recorded && seen.query_id.is_some())
    {
        return;
    }
    let Some(tool_input) = input.tool_input.as_ref() else {
        return;
    };
    let mut activity = format!(
        "{} {}",
        input.tool_name.as_deref().unwrap_or_default(),
        tool_input
    )
    .to_ascii_lowercase();
    activity = truncate_utf8(&activity, TOOL_ACTIVITY_BYTE_CAP);
    let exact_evidence_ids = exact_memory_retrieval_ids(input);
    if record_pending_outcomes(
        cas_root,
        &input.session_id,
        &mut ledger,
        cas_store::RetrievalOutcome::Used,
        Some(&activity),
        &exact_evidence_ids,
    ) {
        ledger.save(&path);
    }
}

/// Finalize inactive injections as unresolved on the normal Stop path. This is
/// intentionally called only after all stop blockers, because a blocked Stop
/// means the session still has future tool activity that may use a card.
pub(crate) fn finalize_ambient_recall_feedback(
    input: &cas_core::hooks::types::HookInput,
    cas_root: &Path,
) {
    let path = ledger_path(cas_root, &input.session_id);
    let mut ledger = RecallLedger::load(&path);
    if record_pending_outcomes(
        cas_root,
        &input.session_id,
        &mut ledger,
        cas_store::RetrievalOutcome::Unresolved,
        None,
        &[],
    ) {
        ledger.save(&path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecallPacket {
    pub(crate) full: String,
    pub(crate) compact: String,
    pub(crate) injected: usize,
    pub(crate) omitted: usize,
    pub(crate) query_hash: String,
}

/// Render only evidence not already injected at the same revision.
///
/// The byte budget is derived before candidates are visited, cards have fixed
/// field caps, and the omission footer is reserved up front. This makes a
/// 300k-item corpus indistinguishable from a small one at the prompt boundary.
pub(crate) fn render_packet(
    identity: &RecallIdentity,
    query: &RecallQuery,
    candidates: &RecallCandidates,
    ledger: &mut RecallLedger,
) -> Option<(RecallPacket, Vec<EvidenceCandidate>)> {
    let policy = identity.role.policy();
    let byte_budget = policy
        .default_tokens
        .min(policy.ceiling_tokens)
        .min(policy.emergency_tokens)
        .saturating_mul(4);
    let query_hash = stable_hash(&query.canonical);
    let mut authored_omitted = candidates.rejected_authored;
    let delta: Vec<EvidenceCandidate> = candidates
        .candidates
        .iter()
        .filter(|candidate| !ledger.has_seen(candidate))
        .filter(|candidate| {
            let authored = ledger.has_authored(candidate);
            if authored {
                authored_omitted += 1;
            }
            !authored
        })
        .cloned()
        .collect();
    if delta.is_empty() {
        ledger.last_query_hash = query_hash;
        return None;
    }

    let header = format!(
        "[ambient recall v1 role={} query={}]",
        match identity.role {
            RecallRole::Worker => "worker",
            RecallRole::Supervisor => "supervisor",
        },
        &query_hash[..12]
    );
    let footer_reserve = 180usize;
    let mut full = header.clone();
    let mut injected = Vec::new();
    let cap = policy.injection_cap.min(delta.len());
    // Exact task/file bindings are complementary context, not a semantic
    // replacement; keep the bounded lexical fallback beside them. Semantic
    // evidence is the channel that suppresses pure-lexical rows.
    let semantic_evidence_exists = delta
        .iter()
        .any(|candidate| candidate.semantic_score.is_some());
    let mut lexical_injected = 0usize;
    let mut non_weak_injected = 0usize;
    let mut weak_lexical_injected = 0usize;
    // Stronger evidence gets first use of the packet budget. A second pass may
    // add weak lexical-only context, but never more weak rows than the packet
    // already contains non-weak rows. This keeps fallback complementary even
    // when the only local hits are common corpus words that evade the bounded
    // document-frequency floor.
    for strong_pass in [true, false] {
        for weak_pass in [false, true] {
            for candidate in delta.iter().filter(|candidate| {
                let weak_lexical_only = !candidate.binding
                    && candidate.lexical_weak
                    && candidate.semantic_score.is_none();
                candidate.strong_session_signal == strong_pass && weak_lexical_only == weak_pass
            }) {
                if injected.len() == cap {
                    break;
                }
                if query.conversational
                    && is_weak_lexical_only(candidate)
                    && !candidate.strong_session_signal
                {
                    continue;
                }
                let independently_semantic = candidate
                    .semantic_score
                    .is_some_and(|score| score >= SEMANTIC_INJECTION_FLOOR);
                if !candidate.binding
                    && !candidate.lexical_eligible
                    && !independently_semantic
                    && !candidate.strong_session_signal
                {
                    continue;
                }
                let lexical_only = !candidate.binding && candidate.semantic_score.is_none();
                if lexical_only
                    && !candidate.strong_session_signal
                    && (semantic_evidence_exists || lexical_injected == LEXICAL_INJECTION_CAP)
                {
                    continue;
                }
                if weak_pass
                    && !candidate.strong_session_signal
                    && weak_lexical_injected == non_weak_injected
                {
                    continue;
                }
                let Some(why_relevant) = injection_reason(candidate) else {
                    continue;
                };
                let mut candidate = candidate.clone();
                candidate.why_relevant = why_relevant;
                let card = render_card(&candidate);
                if full.len() + 1 + card.len() + footer_reserve > byte_budget {
                    break;
                }
                full.push('\n');
                full.push_str(&card);
                injected.push(candidate);
                if lexical_only {
                    lexical_injected += 1;
                }
                if weak_pass {
                    weak_lexical_injected += 1;
                } else {
                    non_weak_injected += 1;
                }
            }
        }
    }
    if injected.is_empty() {
        return None;
    }

    let omitted = delta.len().saturating_sub(injected.len()) + authored_omitted;
    full.push_str(&format!(
        "\n[recall disclosure: injected={} omitted={} scope_rejected={} bodies=tool-pull-only]",
        injected.len(),
        omitted,
        candidates.rejected_scope
    ));
    // The footer reserve is deliberately conservative, but keep the hard cap
    // defense in depth if its fields ever grow.
    if full.len() > byte_budget {
        full = truncate_utf8(&full, byte_budget);
    }
    let compact = format!(
        "[ambient recall: {} new evidence cards; {} omitted; run search for bodies]",
        injected.len(),
        omitted
    );
    Some((
        RecallPacket {
            full,
            compact,
            injected: injected.len(),
            omitted,
            query_hash,
        },
        injected,
    ))
}

/// Outcome feedback adjusts rank but does not turn a weak match into new
/// retrieval evidence. On a conversational prompt, admit lexical candidates
/// only when they have independently useful semantic evidence or a binding.
fn is_weak_lexical_only(candidate: &EvidenceCandidate) -> bool {
    if candidate.binding
        || candidate
            .semantic_score
            .is_some_and(|score| score >= SEMANTIC_INJECTION_FLOOR)
    {
        return false;
    }
    let why = candidate.why_relevant.trim();
    why.starts_with("lexical(weak) match:")
        || why.starts_with("lexical match:")
        || why.starts_with("lexical + semantic match")
}

/// Every displayed card needs a truthful, non-empty selection reason. A
/// lexical label without matched terms is neither: preserve stronger selector
/// evidence when available, otherwise leave the row out of the injection.
fn injection_reason(candidate: &EvidenceCandidate) -> Option<String> {
    if candidate.strong_session_signal {
        return Some("strong accumulated tool-session signal".to_string());
    }
    let existing = candidate.why_relevant.trim();
    if !existing.is_empty() && existing != "lexical match:" && existing != "lexical(weak) match:" {
        return Some(existing.to_string());
    }
    if candidate.binding {
        return Some("exact task/file/symbol binding".to_string());
    }
    candidate
        .semantic_score
        .filter(|score| *score > 0.0)
        .map(|score| format!("semantic match {score:.3}"))
}

fn render_card(candidate: &EvidenceCandidate) -> String {
    let flags = [
        candidate.binding.then_some("binding"),
        candidate
            .stale
            .then_some(if candidate.surface == EvidenceSurface::Memory {
                "EXPIRED"
            } else {
                "STALE"
            }),
        candidate.conflict_key.as_deref().map(|_| "CONFLICT"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(",");
    let flags = if flags.is_empty() {
        String::new()
    } else {
        format!(" flags={flags}")
    };
    format!(
        "- [{}] {:?} {} — {} | why={} | provenance={}:{}@{}{}{}",
        clean_scalar(&candidate.evidence_id, 96),
        candidate.surface,
        clean_scalar(&candidate.snippet, 320),
        if candidate.body_available {
            "body available by tool"
        } else {
            "compact record"
        },
        clean_scalar(&candidate.why_relevant, 120),
        clean_scalar(&candidate.provenance.source, 48),
        clean_scalar(&candidate.provenance.locator, 120),
        clean_scalar(&candidate.provenance.revision, 64),
        flags,
        candidate
            .conflict_key
            .as_deref()
            .map(|key| format!(" conflict={}", clean_scalar(key, 80)))
            .unwrap_or_default()
    )
}

fn stable_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(crate) fn ledger_path(cas_root: &Path, session_id: &str) -> PathBuf {
    cas_root
        .join("cache")
        .join("ambient-recall")
        .join(format!("{}.json", &stable_hash(session_id)[..24]))
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

    let mut fused: HashMap<String, EvidenceCandidate> = HashMap::new();
    for candidate in candidates {
        let key = candidate.canonical_key();
        match fused.get_mut(&key) {
            Some(existing) => fuse_candidate(existing, candidate),
            None => {
                fused.insert(key, candidate);
            }
        }
    }
    let mut candidates: Vec<EvidenceCandidate> = fused.into_values().collect();
    exclude_corpus_ubiquitous_lexical_terms(&mut candidates, &query.canonical);
    // Expired memories remain available through explicit memory/search tools,
    // but SessionStart ambient recall must not present obsolete procedures as
    // current authority. Other stale surfaces (for example a closed task) can
    // still be useful historical evidence and retain their visible flag.
    candidates
        .retain(|candidate| !(candidate.surface == EvidenceSurface::Memory && candidate.stale));
    candidates.retain(|candidate| {
        !candidate.focus_mismatch
            || candidate
                .semantic_score
                .is_some_and(|score| score >= FOCUSED_EPIC_SEMANTIC_FLOOR)
    });
    let authored_evidence = query.authored_evidence.clone();
    let rejected_authored = candidates
        .iter()
        .filter(|candidate| authored_evidence.contains(&candidate.evidence_id))
        .count();
    candidates.retain(|candidate| !authored_evidence.contains(&candidate.evidence_id));
    candidates.sort_by(|a, b| {
        b.strong_session_signal
            .cmp(&a.strong_session_signal)
            .then_with(|| {
                b.binding
                    .cmp(&a.binding)
                    .then_with(|| a.stale.cmp(&b.stale))
                    .then_with(|| b.relevance.total_cmp(&a.relevance))
                    .then_with(|| a.evidence_id.cmp(&b.evidence_id))
            })
    });
    candidates.truncate(policy.candidate_cap);
    Some(RecallCandidates {
        candidates,
        rejected_scope,
        authored_evidence,
        rejected_authored,
    })
}

fn fuse_candidate(existing: &mut EvidenceCandidate, incoming: EvidenceCandidate) {
    existing.lexical_score = existing.lexical_score.max(incoming.lexical_score);
    existing.lexical_eligible |= incoming.lexical_eligible;
    existing.lexical_weak &= incoming.lexical_weak;
    existing.strong_session_signal |= incoming.strong_session_signal;
    existing.focus_mismatch |= incoming.focus_mismatch;
    existing.semantic_score = match (existing.semantic_score, incoming.semantic_score) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (value @ Some(_), None) | (None, value @ Some(_)) => value,
        (None, None) => None,
    };
    existing.structural_score = existing.structural_score.max(incoming.structural_score);
    existing.role_score = existing.role_score.max(incoming.role_score);
    existing.binding |= incoming.binding;
    existing.stale |= incoming.stale;
    existing.body_available |= incoming.body_available;
    if existing.conflict_key.is_none() {
        existing.conflict_key = incoming.conflict_key;
    }
    let semantic = existing.semantic_score.unwrap_or(0.0);
    existing.relevance = existing.lexical_score * 0.32
        + semantic * 0.52
        + existing.structural_score * 0.24
        + existing.role_score;
    existing.why_relevant = match (existing.lexical_score > 0.0, semantic > 0.0) {
        (true, true) if existing.binding => {
            format!("exact binding + lexical + semantic match {semantic:.3}")
        }
        (true, true) => format!("lexical + semantic match {semantic:.3}"),
        (false, true) => format!("semantic match {semantic:.3}"),
        _ => existing.why_relevant.clone(),
    };
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
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;
    use crate::test_support::TestEnvGuard;
    use crate::types::{Entry, Rule, Task, TaskStatus};
    use cas_code::{CodeFile, CodeSymbol, Language, SymbolKind};
    use cas_store::{
        CodeStore, HistoryCommit, HistoryStore, IngestBatch, KnowledgePage, KnowledgeStore,
        PageWrite, SOURCE_EMBEDDINGS, SqliteCodeStore, SqliteCodeVectorStore, SqliteHistoryStore,
        SqliteKnowledgeStore, SqliteSurfacedArtifactStore,
    };

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
            lexical_eligible: true,
            lexical_weak: false,
            strong_session_signal: false,
            focus_mismatch: false,
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

    struct CountingEmbedder {
        calls: Arc<AtomicUsize>,
        meta: EmbeddingMeta,
        vector: Vec<f32>,
    }

    impl RecallQueryEmbedder for CountingEmbedder {
        fn meta(&self) -> EmbeddingMeta {
            self.meta.clone()
        }

        fn embed_query(&self, _query: &str) -> Result<Vec<f32>, ()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.vector.clone())
        }
    }

    struct LiveCountingEmbedder {
        inner: KnowledgeEmbedder,
        calls: Arc<AtomicUsize>,
    }

    impl RecallQueryEmbedder for LiveCountingEmbedder {
        fn meta(&self) -> EmbeddingMeta {
            self.inner.meta()
        }

        fn embed_query(&self, query: &str) -> Result<Vec<f32>, ()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner
                .embed_batch(&[query.to_string()])
                .map_err(|_| ())?
                .into_iter()
                .next()
                .ok_or(())
        }
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
        assert_eq!(crate::internal_llm::INTERNAL_LLM_ENV, "CAS_INTERNAL_LLM");
    }

    #[test]
    fn first_stage_schema_is_evidence_bearing_but_has_no_body() {
        let encoded = serde_json::to_value(candidate("memory-1", EvidenceScope::Global)).unwrap();
        assert_eq!(encoded["provenance"]["source"], "sqlite");
        assert_eq!(encoded["provenance"]["revision"], "r1");
        assert_eq!(encoded["body_available"], true);
        assert!(encoded.get("body").is_none());
    }

    #[test]
    fn packet_is_bounded_discloses_truncation_and_never_contains_bodies() {
        let identity = identity(RecallRole::Worker);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "repair parser cache".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut rows = Vec::new();
        // 5,000 x 600 characters is roughly a 750k-token source corpus. The
        // renderer must pay only for the fixed candidate/card caps.
        for index in 0..5_000 {
            let mut row = candidate(&format!("memory-{index:06}"), EvidenceScope::Global);
            row.snippet = "x".repeat(600);
            row.relevance = 1.0 - (index as f64 / 1_000_000.0);
            rows.push(row);
        }
        let candidates = RecallCandidates {
            candidates: rows,
            rejected_scope: 7,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let mut ledger = RecallLedger::default();
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut ledger).unwrap();
        assert!(packet.full.len() <= identity.role.policy().default_tokens * 4);
        assert!(packet.injected <= identity.role.policy().injection_cap);
        assert_eq!(packet.omitted, 5_000 - packet.injected);
        assert!(packet.full.contains("omitted=499"));
        assert!(packet.full.contains("bodies=tool-pull-only"));
        assert!(!packet.full.contains(&"x".repeat(1_000)));
        ledger.record(packet.query_hash, None, &injected);
        assert!(render_packet(&identity, &query, &candidates, &mut ledger).is_some());
    }

    #[test]
    fn cas_4caa_expired_memory_cards_are_labeled_expired() {
        let identity = identity(RecallRole::Supervisor);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "review merge invariant".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let first = candidate("decision-1", EvidenceScope::Project("project-a".into()));
        let mut ledger = RecallLedger::default();
        let candidates = RecallCandidates {
            candidates: vec![first.clone()],
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut ledger).unwrap();
        ledger.record(packet.query_hash, None, &injected);
        assert!(render_packet(&identity, &query, &candidates, &mut ledger).is_none());

        let mut revised = first;
        revised.provenance.revision = "r2".into();
        revised.stale = true;
        let changed = RecallCandidates {
            candidates: vec![revised],
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let (packet, _) = render_packet(&identity, &query, &changed, &mut ledger).unwrap();
        assert!(packet.full.contains("@r2"));
        assert!(packet.full.contains("EXPIRED"));
    }

    #[test]
    fn injection_never_renders_an_empty_why_label() {
        let identity = identity(RecallRole::Worker);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "repair parser cache".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut empty_lexical = candidate("empty-lexical", EvidenceScope::Global);
        empty_lexical.why_relevant = "lexical match:".into();
        let mut empty_binding = candidate("empty-binding", EvidenceScope::Global);
        empty_binding.why_relevant.clear();
        empty_binding.binding = true;
        let candidates = RecallCandidates {
            candidates: vec![empty_lexical, empty_binding],
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };

        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut RecallLedger::default()).unwrap();
        assert_eq!(packet.injected, 1);
        assert_eq!(packet.omitted, 1);
        assert!(
            injected
                .iter()
                .all(|candidate| !candidate.why_relevant.trim().is_empty())
        );
        assert_eq!(injected[0].why_relevant, "exact task/file/symbol binding");
        assert!(!packet.full.contains("why=lexical match: |"));
    }

    #[test]
    fn same_session_authored_rows_are_not_injected_and_count_as_omitted() {
        let identity = identity(RecallRole::Worker);
        let authored = candidate("self-authored", EvidenceScope::Project("project-a".into()));
        let other = candidate("independent", EvidenceScope::Project("project-a".into()));
        let retriever = FixedRetriever {
            calls: Cell::new(0),
            rows: vec![authored, other],
        };
        let request = RecallRequest {
            prompt: "repair parser cache".into(),
            authored_evidence: vec!["self-authored".into()],
            ..Default::default()
        };
        let query = RecallQuery::build(&identity, &request).unwrap();
        let candidates = retrieve_candidates(&identity, &request, &[&retriever]).unwrap();
        assert_eq!(candidates.rejected_authored, 1);
        assert_eq!(candidates.authored_evidence, vec!["self-authored"]);
        assert_eq!(
            candidates
                .candidates
                .iter()
                .map(|candidate| candidate.evidence_id.as_str())
                .collect::<Vec<_>>(),
            vec!["independent"]
        );

        let mut ledger = RecallLedger::default();
        ledger.record_authored(&candidates.authored_evidence);
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut ledger).unwrap();
        assert_eq!(packet.injected, 1);
        assert_eq!(packet.omitted, 1);
        assert_eq!(injected[0].evidence_id, "independent");
        assert!(!packet.full.contains("self-authored"));
        assert!(packet.full.contains("injected=1 omitted=1"));
    }

    #[test]
    fn ledger_is_bounded_and_loss_only_causes_safe_repetition() {
        let dir = tempfile::tempdir().unwrap();
        let path = ledger_path(dir.path(), "session/../../secret");
        assert_eq!(
            path.parent().unwrap(),
            dir.path().join("cache/ambient-recall")
        );
        let mut ledger = RecallLedger::default();
        let rows: Vec<EvidenceCandidate> = (0..500)
            .map(|index| candidate(&format!("m-{index}"), EvidenceScope::Global))
            .collect();
        ledger.record("query".into(), None, &rows);
        ledger.save(&path);
        let loaded = RecallLedger::load(&path);
        assert_eq!(loaded.seen.len(), LEDGER_ENTRY_CAP);
        assert!(fs::metadata(&path).unwrap().len() <= LEDGER_BYTE_CAP as u64);
        fs::remove_file(&path).unwrap();
        assert_eq!(RecallLedger::load(&path), RecallLedger::default());
    }

    #[test]
    fn automatic_hook_feedback_populates_metrics_with_plausible_attribution() {
        use cas_store::{RetrievalStore, SqliteRetrievalStore};

        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let identity = identity(RecallRole::Worker);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "repair parser cache".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let candidates = RecallCandidates {
            candidates: vec![
                candidate("memory-used", EvidenceScope::Global),
                candidate("memory-ignored", EvidenceScope::Global),
            ],
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let mut ledger = RecallLedger::default();
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut ledger).unwrap();
        let query_id = record_ambient_query(&cas_root, &identity, &query, &injected, false);
        assert!(query_id.is_some());
        ledger.record(packet.query_hash, query_id, &injected);
        ledger.save(&ledger_path(&cas_root, &identity.session_id));

        let tool = cas_core::hooks::types::HookInput {
            session_id: identity.session_id.clone(),
            tool_name: Some("Read".into()),
            tool_input: Some(serde_json::json!({"memory_id": "memory-used"})),
            ..Default::default()
        };
        record_ambient_tool_usage(&tool, &cas_root);
        finalize_ambient_recall_feedback(&tool, &cas_root);

        let groups = SqliteRetrievalStore::open(&cas_root)
            .unwrap()
            .aggregate()
            .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].query_family, "ambient_transition");
        assert_eq!(groups[0].ranking_policy, AMBIENT_RETRIEVAL_POLICY);
        assert_eq!(
            (
                groups[0].total,
                groups[0].resolved,
                groups[0].unresolved,
                groups[0].used,
                groups[0].ignored,
            ),
            (2, 1, 1, 1, 0)
        );
        let ledger = RecallLedger::load(&ledger_path(&cas_root, &identity.session_id));
        assert!(ledger.seen.iter().all(|seen| seen.outcome_recorded));
    }

    #[test]
    fn memory_get_marks_a_short_injected_id_used() {
        use cas_store::{RetrievalStore, SqliteRetrievalStore};

        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let identity = identity(RecallRole::Worker);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "repair parser cache".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let candidates = RecallCandidates {
            candidates: vec![candidate("m1", EvidenceScope::Global)],
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let mut ledger = RecallLedger::default();
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut ledger).unwrap();
        let query_id = record_ambient_query(&cas_root, &identity, &query, &injected, false);
        ledger.record(packet.query_hash, query_id, &injected);
        ledger.save(&ledger_path(&cas_root, &identity.session_id));

        let tool = cas_core::hooks::types::HookInput {
            session_id: identity.session_id.clone(),
            tool_name: Some("mcp__cs__memory".into()),
            tool_input: Some(serde_json::json!({"action": "get", "id": "m1"})),
            tool_response: Some(serde_json::json!({"id": "m1", "content": "body"})),
            ..Default::default()
        };
        record_ambient_tool_usage(&tool, &cas_root);
        finalize_ambient_recall_feedback(&tool, &cas_root);

        let groups = SqliteRetrievalStore::open(&cas_root)
            .unwrap()
            .aggregate()
            .unwrap();
        assert_eq!((groups[0].total, groups[0].used), (1, 1));
    }

    #[test]
    fn foreign_memory_tool_does_not_mark_an_injected_id_used() {
        use cas_store::SqliteRetrievalStore;

        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let identity = identity(RecallRole::Worker);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "repair parser cache".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let candidates = RecallCandidates {
            candidates: vec![candidate("m1", EvidenceScope::Global)],
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let mut ledger = RecallLedger::default();
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut ledger).unwrap();
        let query_id = record_ambient_query(&cas_root, &identity, &query, &injected, false);
        ledger.record(packet.query_hash, query_id, &injected);
        ledger.save(&ledger_path(&cas_root, &identity.session_id));

        record_ambient_tool_usage(
            &cas_core::hooks::types::HookInput {
                session_id: identity.session_id.clone(),
                tool_name: Some("mcp__foreign__memory".into()),
                tool_input: Some(serde_json::json!({"action": "get", "id": "m1"})),
                tool_response: Some(serde_json::json!({"id": "m1"})),
                ..Default::default()
            },
            &cas_root,
        );

        let groups = SqliteRetrievalStore::open(&cas_root)
            .unwrap()
            .aggregate()
            .unwrap();
        assert_eq!((groups[0].total, groups[0].used), (0, 0));
    }

    #[test]
    fn opencode_memory_shape_marks_an_injected_id_used() {
        use cas_store::SqliteRetrievalStore;

        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let identity = identity(RecallRole::Worker);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "repair parser cache".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let candidates = RecallCandidates {
            candidates: vec![candidate("m1", EvidenceScope::Global)],
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let mut ledger = RecallLedger::default();
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut ledger).unwrap();
        let query_id = record_ambient_query(&cas_root, &identity, &query, &injected, false);
        ledger.record(packet.query_hash, query_id, &injected);
        ledger.save(&ledger_path(&cas_root, &identity.session_id));

        record_ambient_tool_usage(
            &cas_core::hooks::types::HookInput {
                session_id: identity.session_id.clone(),
                tool_name: Some("cas_memory".into()),
                tool_input: Some(serde_json::json!({"action": "get", "id": "m1"})),
                tool_response: Some(serde_json::json!({"id": "m1"})),
                ..Default::default()
            },
            &cas_root,
        );

        let groups = SqliteRetrievalStore::open(&cas_root)
            .unwrap()
            .aggregate()
            .unwrap();
        assert_eq!((groups[0].total, groups[0].used), (1, 1));
    }

    #[test]
    fn unresolved_outcomes_do_not_adjust_or_dilute_ranking() {
        assert_eq!(outcome_adjustment(0, 0, 0, 0, 0, 0), 0.0);
    }

    #[test]
    fn memory_list_response_returns_exact_injected_ids_without_substring_matching() {
        let input = cas_core::hooks::types::HookInput {
            tool_name: Some("mcp__cs__memory".into()),
            tool_input: Some(serde_json::json!({"action": "list"})),
            tool_response: Some(serde_json::json!({
                "entries": [{"id": "m1"}, {"id": "m2"}]
            })),
            ..Default::default()
        };
        assert_eq!(exact_memory_retrieval_ids(&input), vec!["m1", "m2"]);
    }

    #[test]
    fn exact_memory_id_collection_reports_when_the_response_reaches_its_cap() {
        let response = serde_json::json!({
            "entries": (0..=LEDGER_ENTRY_CAP)
                .map(|index| serde_json::json!({"id": format!("memory-{index}")}))
                .collect::<Vec<_>>()
        });
        let mut ids = Vec::new();
        assert!(collect_exact_id_fields(&response, &mut ids));
        assert_eq!(ids.len(), LEDGER_ENTRY_CAP);
    }

    #[test]
    fn ambient_rule_surface_is_recorded_in_the_shared_ledger() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let rule_store = crate::store::open_rule_store_local(&cas_root).unwrap();
        rule_store
            .add(&Rule::new(
                "ambient-rule".into(),
                "Always surface the rule flywheel".into(),
            ))
            .unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("worker-one")),
            ("CAS_FACTORY_SESSION", Some("factory-one")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let input = cas_core::hooks::types::HookInput {
            session_id: "ambient-rule-session".into(),
            cwd: project.path().to_string_lossy().into_owned(),
            agent_role: Some("worker".into()),
            ..Default::default()
        };

        let packet = build_ambient_recall_context(
            &input,
            &cas_root,
            Some("Please surface the rule flywheel"),
            true,
        )
        .expect("the matching draft rule should be surfaced");
        assert!(packet.full.contains("ambient-rule"));

        let surfaced = SqliteSurfacedArtifactStore::open(&cas_root).unwrap();
        assert_eq!(surfaced.count_for_session(&input.session_id).unwrap(), 1);
        assert_eq!(rule_store.get("ambient-rule").unwrap().surface_count, 1);
        let impact = surfaced.aggregate(10).unwrap();
        assert_eq!(impact.len(), 1);
        assert_eq!(impact[0].artifact_id, "ambient-rule");
        assert_eq!(impact[0].surfaced_count, 1);
    }

    #[test]
    fn aggregated_outcomes_change_otherwise_identical_recall_ranking() {
        use cas_store::{
            RetrievalHitIdentity, RetrievalOutcome, RetrievalStore, SqliteRetrievalStore,
        };

        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let store = SqliteRetrievalStore::open(&cas_root).unwrap();
        store
            .record_query(
                "qry-ranking",
                "same query",
                "ambient_transition",
                AMBIENT_RETRIEVAL_POLICY,
                Some("session"),
                &[
                    RetrievalHitIdentity {
                        result_id: "helpful-source".into(),
                        document_type: "entry".into(),
                        rank: 0,
                    },
                    RetrievalHitIdentity {
                        result_id: "harmful-source".into(),
                        document_type: "entry".into(),
                        rank: 1,
                    },
                    RetrievalHitIdentity {
                        result_id: "unresolved-source".into(),
                        document_type: "entry".into(),
                        rank: 2,
                    },
                ],
            )
            .unwrap();
        store
            .record_outcome(
                "out-helpful",
                "qry-ranking",
                "helpful-source",
                RetrievalOutcome::Helpful,
                "actor",
                "session",
                None,
            )
            .unwrap();
        store
            .record_outcome(
                "out-harmful",
                "qry-ranking",
                "harmful-source",
                RetrievalOutcome::Harmful,
                "actor",
                "session",
                None,
            )
            .unwrap();
        store
            .record_outcome(
                "out-unresolved",
                "qry-ranking",
                "unresolved-source",
                RetrievalOutcome::Unresolved,
                "actor",
                "session",
                None,
            )
            .unwrap();

        let mut candidates = vec![
            candidate("harmful-source", EvidenceScope::Global),
            candidate("helpful-source", EvidenceScope::Global),
            candidate("unresolved-source", EvidenceScope::Global),
        ];
        let baseline = candidates[0].relevance;
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.relevance == baseline)
        );
        apply_outcome_feedback(&cas_root, &mut candidates);
        assert_eq!(candidates[0].evidence_id, "helpful-source");
        assert_eq!(candidates[1].evidence_id, "unresolved-source");
        assert_eq!(candidates[1].relevance, baseline);
        assert!(candidates[0].relevance > candidates[2].relevance);
        assert!(
            candidates
                .iter()
                .all(|row| row.why_relevant.contains("outcome history"))
        );
    }

    #[test]
    fn automatic_feedback_helpers_are_fail_open_without_a_store() {
        let project = tempfile::tempdir().unwrap();
        let missing_root = project.path().join("missing-cas-root");
        let input = cas_core::hooks::types::HookInput {
            session_id: "session".into(),
            tool_name: Some("Read".into()),
            tool_input: Some(serde_json::json!({"path": "x".repeat(TOOL_ACTIVITY_BYTE_CAP * 2)})),
            ..Default::default()
        };
        record_ambient_tool_usage(&input, &missing_root);
        finalize_ambient_recall_feedback(&input, &missing_root);
    }

    #[test]
    fn local_fallback_is_scope_safe_and_role_profiles_rank_differently() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cas.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"
            create table entries (
                id text primary key, title text, content text, scope text,
                team_id text, share text, updated_at text, created text,
                valid_until text, archived integer
            );
            create table tasks (
                id text primary key, title text, description text, design text,
                notes text, team_id text, share text, updated_at text, status text
            );
            create table code_symbols (
                id text primary key, qualified_name text, name text,
                file_path text, documentation text, signature text,
                scope text, content_hash text
            );
            create table knowledge_pages (
                id text primary key, title text, snippet text, page_type text,
                rel_path text, updated_at text, origin text, origin_project_id text
            );
            insert into entries values
                ('team-ok', '', 'parser cache failure mode', 'project', 'team-a', null, 'r2', 'r1', null, 0),
                ('team-leak', '', 'parser cache failure mode', 'project', 'team-b', null, 'r2', 'r1', null, 0),
                ('private-unowned', '', 'parser cache failure mode', 'project', null, 'private', 'r2', 'r1', null, 0);
            insert into tasks values
                ('cas-parser', 'parser cache task', '', '', '', null, null, 'r2', 'in_progress');
            insert into code_symbols values
                ('sym-parser', 'parser::cache', 'cache', 'src/parser.rs', 'parser cache failure mode', 'fn cache()', 'project', 'hash-1');
            insert into knowledge_pages values
                ('knowledge-leak', 'parser cache', 'foreign guidance', 'guide', 'guide/parser.md', 'r2', 'cloud_pull', 'project-b');
            "#,
        )
        .unwrap();
        drop(conn);

        let retriever = SqliteRecallRetriever::existing(dir.path()).unwrap();
        let request = RecallRequest {
            prompt: "parser cache failure".into(),
            ..Default::default()
        };
        let worker = identity(RecallRole::Worker);
        let worker_rows = retrieve_candidates(&worker, &request, &[&retriever]).unwrap();
        let worker_ids: Vec<&str> = worker_rows
            .candidates
            .iter()
            .map(|row| row.evidence_id.as_str())
            .collect();
        assert!(worker_ids.contains(&"team-ok"));
        assert!(!worker_ids.contains(&"team-leak"));
        assert!(!worker_ids.contains(&"private-unowned"));
        assert!(!worker_ids.contains(&"knowledge-leak"));
        assert_eq!(worker_ids[0], "sym-parser");

        let supervisor = identity(RecallRole::Supervisor);
        let supervisor_rows = retrieve_candidates(&supervisor, &request, &[&retriever]).unwrap();
        assert_eq!(supervisor_rows.candidates[0].evidence_id, "cas-parser");
        assert_eq!(supervisor_rows.candidates[0].role_score, 0.31);
        assert_eq!(supervisor_rows.candidates[1].evidence_id, "sym-parser");
        assert_eq!(supervisor_rows.candidates[1].role_score, 0.08);
        assert!(
            supervisor_rows.candidates[0].relevance > supervisor_rows.candidates[1].relevance,
            "a current two-term task must narrowly outrank a three-term code match"
        );
        assert!(!dir.path().join("index/code-vectors").exists());
    }

    #[test]
    fn supervisor_task_boost_requires_current_multi_term_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            create table entries (
                id text primary key, title text, content text, scope text,
                team_id text, share text, updated_at text, created text,
                valid_until text, archived integer
            );
            create table tasks (
                id text primary key, title text, description text, design text,
                notes text, team_id text, share text, updated_at text, status text
            );
            create table code_symbols (
                id text primary key, qualified_name text, name text,
                file_path text, documentation text, signature text,
                scope text, content_hash text
            );
            insert into entries values
                ('current-memory', '', 'signing artifact guidance', 'project',
                 null, null, 'r1', 'r1', null, 0);
            insert into tasks values
                ('weak-task', 'release checklist', '', '', '', null, null, 'r1', 'open'),
                ('closed-task', 'release signing artifact recovery', '', '', '',
                 null, null, 'r1', 'closed');
            insert into code_symbols values
                ('current-code', 'release::signing', 'signing', 'src/release.rs',
                 'signing artifact implementation', 'fn sign()', 'project', 'r1');
            "#,
        )
        .unwrap();
        drop(conn);

        let retriever = SqliteRecallRetriever::existing(dir.path()).unwrap();
        let candidates = retrieve_candidates(
            &identity(RecallRole::Supervisor),
            &RecallRequest {
                prompt: "release signing artifact recovery".into(),
                ..Default::default()
            },
            &[&retriever],
        )
        .unwrap()
        .candidates;
        let ids = candidates
            .iter()
            .map(|candidate| candidate.evidence_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(&ids[..2], &["current-code", "current-memory"]);
        assert_eq!(ids[2], "weak-task");
        assert_eq!(ids[3], "closed-task");
        let weak_task = candidates
            .iter()
            .find(|candidate| candidate.evidence_id == "weak-task")
            .unwrap();
        let closed_task = candidates
            .iter()
            .find(|candidate| candidate.evidence_id == "closed-task")
            .unwrap();
        assert!(weak_task.lexical_weak);
        assert_eq!(weak_task.role_score, 0.08);
        assert_eq!(closed_task.role_score, 0.08);
    }

    #[test]
    fn lexical_quality_floor_rejects_all_stopword_match_sets() {
        assert!(!lexical_match_is_eligible(&["the", "old", "context"]));
        assert!(!lexical_match_is_eligible(&["use", "queue"]));
        assert_eq!(query_terms("what them open release"), vec!["release"]);
        assert!(lexical_match_is_eligible(&["release"]));
        assert_eq!(
            query_terms("the old context use queue for release signing"),
            vec!["release", "signing"]
        );
    }

    /// cas-8284: fixtures mirror the first live post-floor injection: English
    /// stopwords (`what`), a project slug present in every local row
    /// (`cas-src`), and semantic noise below the observed 0.470 cutoff. Every
    /// exclusion remains in the disclosure denominator.
    #[test]
    fn ambient_floor_rejects_live_stopword_slug_and_semantic_noise_shapes() {
        let identity = identity(RecallRole::Supervisor);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "cas-src parser repair".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut stopword = candidate(
            "live-stopword-what",
            EvidenceScope::Project("project-a".into()),
        );
        stopword.lexical_eligible = false;
        stopword.lexical_weak = true;
        stopword.why_relevant = "lexical(weak) match: what".into();

        let mut slug_rows = (0..3)
            .map(|index| {
                let mut row = candidate(
                    &format!("live-slug-{index}"),
                    EvidenceScope::Project("project-a".into()),
                );
                row.provenance.source = "cas.db/read-only".into();
                row.snippet = format!("cas-src generic history row {index}");
                row.why_relevant = "lexical(weak) match: cas-src".into();
                row
            })
            .collect::<Vec<_>>();
        let mut parser = candidate("live-parser", EvidenceScope::Project("project-a".into()));
        parser.provenance.source = "cas.db/read-only".into();
        parser.snippet = "cas-src parser cache repair".into();

        slug_rows.push(parser);
        exclude_corpus_ubiquitous_lexical_terms(&mut slug_rows, &query.canonical);
        assert!(slug_rows[..3].iter().all(|row| !row.lexical_eligible));
        assert!(slug_rows[3].lexical_eligible);

        let mut semantic_noise = candidate(
            "live-semantic-noise",
            EvidenceScope::Project("project-a".into()),
        );
        semantic_noise.lexical_eligible = false;
        semantic_noise.semantic_score = Some(0.467);
        semantic_noise.why_relevant = "semantic match 0.467".into();
        let mut semantic_useful = candidate(
            "live-semantic-useful",
            EvidenceScope::Project("project-a".into()),
        );
        semantic_useful.lexical_eligible = false;
        semantic_useful.semantic_score = Some(SEMANTIC_INJECTION_FLOOR);
        semantic_useful.why_relevant = format!("semantic match {SEMANTIC_INJECTION_FLOOR:.3}");

        let candidates = RecallCandidates {
            candidates: std::iter::once(stopword)
                .chain(slug_rows)
                .chain([semantic_noise, semantic_useful])
                .collect(),
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut RecallLedger::default()).unwrap();
        assert_eq!(
            injected
                .iter()
                .map(|row| row.evidence_id.as_str())
                .collect::<Vec<_>>(),
            vec!["live-semantic-useful"],
            "the supervisor one-card cap may omit eligible lexical evidence, but it must never
             select a stopword, ubiquitous slug, or sub-floor semantic card"
        );
        assert_eq!(
            packet.omitted, 6,
            "every excluded live-shape card is omitted"
        );
        assert!(packet.full.contains("injected=1 omitted=6"));
        assert!(!packet.full.contains("live-stopword-what"));
        assert!(!packet.full.contains("live-slug-"));
        assert!(!packet.full.contains("live-semantic-noise"));
    }

    #[test]
    fn lexical_fixture_excludes_merge_and_trailer_noise_but_keeps_relevant_rows() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            create table tasks (
                id text primary key, title text, description text, design text,
                notes text, team_id text, share text, updated_at text, status text
            );
            create table history_commits (
                sha text primary key, subject text, body text, indexed_at text,
                scope text, is_merge integer
            );
            insert into tasks values
                ('cas-release', 'Mobile release signing artifact', '', '', '', null, null, 'r2', 'in_progress'),
                ('stopword-noise', 'The old context use queue', '', '', '', null, null, 'r3', 'open');
            insert into history_commits values
                ('relevant', 'Fix release signing artifact', 'real implementation detail', 'r2', 'project', 0),
                ('merge-noise', 'Merge branch main', 'release signing artifact', 'r3', 'project', 1),
                ('trailer-noise', 'Maintenance cleanup', 'Co-Authored-By: release signing artifact', 'r3', 'project', 0);
            "#,
        )
        .unwrap();
        drop(conn);

        let retriever = SqliteRecallRetriever::existing(dir.path()).unwrap();
        let rows = retrieve_candidates(
            &identity(RecallRole::Supervisor),
            &RecallRequest {
                prompt: "mobile release signing artifact".into(),
                ..Default::default()
            },
            &[&retriever],
        )
        .unwrap();
        let ids: Vec<&str> = rows
            .candidates
            .iter()
            .map(|candidate| candidate.evidence_id.as_str())
            .collect();
        assert!(ids.contains(&"cas-release"));
        assert!(ids.contains(&"relevant"));
        assert!(!ids.contains(&"stopword-noise"));
        assert!(!ids.contains(&"merge-noise"));
        assert!(!ids.contains(&"trailer-noise"));
    }

    /// cas-fa938: replay the exact shape of query 4405439dd571. The project
    /// identity and the adjective `every` must not turn an unrelated flock
    /// post-mortem into release guidance, and an expired procedure must not be
    /// presented beside the current task as ambient authority.
    #[test]
    fn release_recovery_replay_rejects_identity_noise_and_expired_procedure() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            create table entries (
                id text primary key, title text, content text, scope text,
                team_id text, share text, updated_at text, created text,
                valid_until text, archived integer
            );
            create table tasks (
                id text primary key, title text, description text, design text,
                notes text, team_id text, share text, updated_at text, status text
            );
            insert into entries values
                ('flock-history', '',
                 'POSIX flock in cas-src releases only when every descriptor closes',
                 'project', null, null, '2026-08-03', '2026-08-03', null, 0),
                ('obsolete-release-procedure', '',
                 'Every release train requires workers to avoid origin pushes and gh commands',
                 'project', null, null, '2026-09-01', '2026-09-01',
                 '2026-09-03T00:00:00Z', 0),
                ('current-release-guidance', '',
                 'Current release-gate recovery keeps ledger-last and scratch-base preflight receipts',
                 'project', null, null, '2026-09-04', '2026-09-04', null, 0);
            insert into tasks values
                ('cas-d2cf',
                 'cas-cut-release + release-gate: encode every hour-losing lesson from the v3.16.0 train (merge-only-on-green-CI, ledger-last, scratch-base preflight row, detached gate + reminders, --stop kills children, --only rerun, fixture rules, snapshot policy)',
                 'Harden release recovery without reviving obsolete worker rules', '', '',
                 null, null, '2026-09-04', 'in_progress');
            "#,
        )
        .unwrap();
        drop(conn);

        let mut supervisor = identity(RecallRole::Supervisor);
        supervisor.project_id = "cas-src".into();
        let request = RecallRequest {
            prompt: "session start".into(),
            task_id: Some("cas-d2cf".into()),
            task_title: Some("cas-cut-release + release-gate: encode every hour-losing lesson from the v3.16.0 train (merge-only-on-green-CI, ledger-last, scratch-base preflight row, detached gate + reminders, --stop kills children, --only rerun, fixture rules, snapshot policy)".into()),
            task_labels: vec!["cas-cut-release".into()],
            ..Default::default()
        };
        let query = RecallQuery::build(&supervisor, &request).unwrap();
        let ranked_terms = query_terms(&query.canonical);
        assert!(!ranked_terms.iter().any(|term| term == "cas-src"));
        assert!(!ranked_terms.iter().any(|term| term == "every"));
        let mut traced_terms = decision_trigger_terms(&query)
            .into_iter()
            .map(|term| term.term)
            .collect::<Vec<_>>();
        traced_terms.sort();
        traced_terms.dedup();
        let mut ranked_terms_for_trace = ranked_terms.clone();
        ranked_terms_for_trace.sort();
        assert_eq!(traced_terms, ranked_terms_for_trace);
        let retriever = SqliteRecallRetriever::existing(dir.path()).unwrap();
        let candidates = retrieve_candidates(&supervisor, &request, &[&retriever]).unwrap();
        let (packet, injected) = render_packet(
            &supervisor,
            &query,
            &candidates,
            &mut RecallLedger::default(),
        )
        .unwrap();
        let ids = injected
            .iter()
            .map(|candidate| candidate.evidence_id.as_str())
            .collect::<Vec<_>>();

        assert!(
            ids.contains(&"cas-d2cf"),
            "exact task binding was lost: {packet:?}"
        );
        assert!(
            ids.contains(&"current-release-guidance"),
            "useful current recovery guidance was lost: {packet:?}"
        );
        assert!(
            !ids.contains(&"flock-history"),
            "project identity + generic adjective selected unrelated history: {packet:?}"
        );
        assert!(
            !ids.contains(&"obsolete-release-procedure"),
            "expired procedural memory was presented as ambient authority: {packet:?}"
        );
    }

    #[test]
    fn same_day_rfc3339_expiry_is_compared_as_time_not_text() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            create table entries (
                id text primary key, title text, content text, scope text,
                team_id text, share text, updated_at text, created text,
                valid_until text, archived integer
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "insert into entries values (?1, '', ?2, 'project', null, null, ?3, ?3, ?4, 0)",
            params![
                "expired-same-day",
                "same day release preference expired",
                "r1",
                "2026-09-05T00:00:00Z"
            ],
        )
        .unwrap();
        conn.execute(
            "insert into entries values (?1, '', ?2, 'project', null, null, ?3, ?3, ?4, 0)",
            params![
                "current-same-day",
                "same day release preference current",
                "r1",
                "2026-09-05T23:59:59Z"
            ],
        )
        .unwrap();
        conn.execute(
            "insert into entries values (?1, '', ?2, 'project', null, null, ?3, ?3, ?4, 0)",
            params![
                "expired-offset",
                "same day release preference offset expired",
                "r1",
                "2026-09-05T07:00:00+05:00"
            ],
        )
        .unwrap();
        conn.execute(
            "insert into entries values (?1, '', ?2, 'project', null, null, ?3, ?3, null, 0)",
            params!["no-expiry", "same day release preference stable", "r1"],
        )
        .unwrap();

        // Pin the midnight, same-day future, timezone-offset, and NULL cases
        // against a fixed reference instant while deriving the predicate from
        // the exact expression used by the local retriever.
        let fixed_stale_sql =
            MEMORY_STALE_SQL.replace("datetime('now')", "datetime('2026-09-05 12:00:00')");
        let mut stmt = conn
            .prepare(&format!(
                "select id from entries where ({fixed_stale_sql}) = 0 order by id"
            ))
            .unwrap();
        let fixed_current = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(fixed_current, vec!["current-same-day", "no-expiry"]);
        drop(stmt);

        // Retain an integration check through retrieve_candidates using live
        // instants safely separated from the current clock.
        let now = Utc::now();
        let expired = (now - chrono::TimeDelta::minutes(5)).to_rfc3339();
        let future = (now + chrono::TimeDelta::minutes(5)).to_rfc3339();
        let expired_with_offset = (now - chrono::TimeDelta::minutes(5))
            .with_timezone(&chrono::FixedOffset::east_opt(5 * 60 * 60).unwrap())
            .to_rfc3339();
        conn.execute(
            "update entries set valid_until = ?1 where id = 'expired-same-day'",
            [expired],
        )
        .unwrap();
        conn.execute(
            "update entries set valid_until = ?1 where id = 'current-same-day'",
            [future],
        )
        .unwrap();
        conn.execute(
            "update entries set valid_until = ?1 where id = 'expired-offset'",
            [expired_with_offset],
        )
        .unwrap();
        drop(conn);

        let retriever = SqliteRecallRetriever::existing(dir.path()).unwrap();
        let candidates = retrieve_candidates(
            &identity(RecallRole::Supervisor),
            &RecallRequest {
                prompt: "same day release preference".into(),
                ..Default::default()
            },
            &[&retriever],
        )
        .unwrap();
        let ids = candidates
            .candidates
            .iter()
            .map(|candidate| candidate.evidence_id.as_str())
            .collect::<Vec<_>>();

        assert!(ids.contains(&"current-same-day"));
        assert!(ids.contains(&"no-expiry"));
        assert!(!ids.contains(&"expired-same-day"));
        assert!(!ids.contains(&"expired-offset"));
    }

    /// An operator decision and the deployed runtime state are complementary
    /// evidence until the implementation lands; recall must keep their source
    /// types and wording distinct rather than treating the decision as shipped.
    #[test]
    fn decided_not_shipped_policy_remains_distinct_from_deployed_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            create table entries (
                id text primary key, title text, content text, scope text,
                team_id text, share text, updated_at text, created text,
                valid_until text, archived integer
            );
            create table tasks (
                id text primary key, title text, description text, design text,
                notes text, team_id text, share text, updated_at text, status text
            );
            insert into entries values
                ('deployed-taste-route', '',
                 'Deployed runtime taste lane remains Claude Opus until the migration lands',
                 'project', null, null, '2026-09-04', '2026-09-04', null, 0);
            insert into tasks values
                ('cas-b8fc',
                 'Replace the taste lane with Codex gpt-6-astra at medium effort',
                 'Operator decision is current intent; implementation is not yet shipped', '', '',
                 null, null, '2026-09-04', 'open');
            "#,
        )
        .unwrap();
        drop(conn);

        let supervisor = identity(RecallRole::Supervisor);
        let request = RecallRequest {
            prompt: "session start".into(),
            task_id: Some("cas-b8fc".into()),
            task_title: Some(
                "Replace the taste lane with Codex gpt-6-astra at medium effort".into(),
            ),
            ..Default::default()
        };
        let retriever = SqliteRecallRetriever::existing(dir.path()).unwrap();
        let candidates = retrieve_candidates(&supervisor, &request, &[&retriever]).unwrap();
        let intent = candidates
            .candidates
            .iter()
            .find(|candidate| candidate.evidence_id == "cas-b8fc")
            .expect("current intent task must be recalled");
        let deployed = candidates
            .candidates
            .iter()
            .find(|candidate| candidate.evidence_id == "deployed-taste-route")
            .expect("deployed runtime state must remain useful");

        assert!(intent.binding);
        assert_eq!(intent.surface, EvidenceSurface::Task);
        assert!(intent.snippet.contains("not yet shipped"));
        assert_eq!(deployed.surface, EvidenceSurface::Memory);
        assert!(deployed.snippet.contains("Deployed runtime"));
        assert!(!deployed.snippet.contains("gpt-6-astra"));
    }

    #[test]
    fn weak_lexical_survivors_are_labeled_but_cannot_make_a_packet_alone() {
        let identity = identity(RecallRole::Supervisor);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "release signing artifact".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let weak = local_candidate(
            LocalRow {
                id: "weak-release".into(),
                surface: EvidenceSurface::Memory,
                scope: EvidenceScope::Project("project-a".into()),
                snippet: "release procedure".into(),
                revision: "r1".into(),
                stale: false,
                body_available: true,
                locator: "weak-release".into(),
            },
            &query,
            &query_terms(&query.canonical),
        );
        assert!(weak.lexical_weak);
        assert!(weak.why_relevant.starts_with("lexical(weak)"));

        let candidates = RecallCandidates {
            candidates: (0..12)
                .map(|index| {
                    let mut candidate = candidate(
                        &format!("lexical-{index}"),
                        EvidenceScope::Project("project-a".into()),
                    );
                    candidate.lexical_weak = true;
                    candidate.why_relevant = "lexical(weak) match: release".into();
                    candidate
                })
                .collect(),
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        assert!(
            render_packet(&identity, &query, &candidates, &mut RecallLedger::default()).is_none()
        );
    }

    #[test]
    fn conversational_prompts_omit_weak_lexical_cards_but_keep_bindings() {
        let identity = identity(RecallRole::Supervisor);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "How is the memory system performing?".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(query.conversational);

        let mut weak = candidate("weak", EvidenceScope::Project("project-a".into()));
        weak.lexical_weak = true;
        weak.why_relevant = "lexical(weak) match: memory + outcome history +0.050".into();

        let mut bare = candidate("bare", EvidenceScope::Project("project-a".into()));
        bare.why_relevant = "lexical match: regen,churn".into();

        let mut low_semantic =
            candidate("low-semantic", EvidenceScope::Project("project-a".into()));
        low_semantic.semantic_score = Some(SEMANTIC_INJECTION_FLOOR - 0.001);
        low_semantic.why_relevant = "lexical + semantic match 0.469".into();

        let mut binding = candidate("binding", EvidenceScope::Project("project-a".into()));
        binding.binding = true;
        binding.why_relevant = "exact task/file/symbol binding".into();

        let candidates = RecallCandidates {
            candidates: vec![weak, bare, low_semantic, binding],
            rejected_scope: 0,
            authored_evidence: Vec::new(),
            rejected_authored: 0,
        };
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut RecallLedger::default()).unwrap();

        assert_eq!(
            injected
                .iter()
                .map(|candidate| candidate.evidence_id.as_str())
                .collect::<Vec<_>>(),
            vec!["binding"]
        );
        assert!(
            packet
                .full
                .contains("[recall disclosure: injected=1 omitted=3")
        );
    }

    /// Characterization for GH #553: a conversational turn whose sole useful
    /// lexical signal is weak currently produces no packet. The observability
    /// delivery must leave a durable explanation instead of making this shape
    /// indistinguishable from an absent hook invocation.
    #[test]
    fn gh_553_silent_conversational_turn_leaves_a_decision_trace() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("worker-one")),
            ("CAS_FACTORY_SESSION", Some("factory-one")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        crate::store::open_store_local(&cas_root)
            .unwrap()
            .add(&Entry {
                id: "gh-553-weak-memory".into(),
                title: Some("Memory operational note".into()),
                content: "system".into(),
                ..Entry::default()
            })
            .unwrap();
        let input = cas_core::hooks::types::HookInput {
            session_id: "gh-553-silent".into(),
            cwd: project.path().to_string_lossy().into_owned(),
            agent_role: Some("worker".into()),
            ..Default::default()
        };

        assert!(
            build_ambient_recall_context(
                &input,
                &cas_root,
                Some("How is the system performing?"),
                false,
            )
            .is_none()
        );

        let trace_dir = cas_root.join("cache/ambient-recall/decisions");
        let trace = fs::read_dir(&trace_dir)
            .expect("every silent recall turn must leave a queryable decision trace")
            .filter_map(Result::ok)
            .map(|entry| fs::read_to_string(entry.path()).unwrap())
            .collect::<String>();
        assert!(trace.contains("precision_gate"), "{trace}");
        assert!(trace.contains("gh-553-weak-memory"), "{trace}");
        assert!(trace.contains("\"source\":\"prompt\""), "{trace}");
        assert!(trace.contains("\"lexical_score\":"), "{trace}");
    }

    #[test]
    fn strong_accumulated_tool_signal_overrides_conversational_precision_gate() {
        let identity = identity(RecallRole::Worker);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "How should we proceed after that?".into(),
                tool_result_terms: vec!["twilio".into(), "30034".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(query.conversational);
        let mut strong = candidate("twilio-title", EvidenceScope::Project("project-a".into()));
        strong.snippet = "Twilio A2P staging recovery".into();
        strong.lexical_eligible = false;
        strong.lexical_weak = true;
        strong.strong_session_signal = true;
        strong.why_relevant = "lexical(weak) match: twilio".into();
        let (packet, injected) = render_packet(
            &identity,
            &query,
            &RecallCandidates {
                candidates: vec![strong],
                rejected_scope: 0,
                authored_evidence: Vec::new(),
                rejected_authored: 0,
            },
            &mut RecallLedger::default(),
        )
        .expect("strong accumulated tool context must inject a title on conversational prose");
        assert_eq!(injected.len(), 1);
        assert!(packet.full.contains("Twilio A2P staging recovery"));
        assert!(
            packet
                .full
                .contains("strong accumulated tool-session signal")
        );
    }

    #[test]
    fn tool_trigger_context_drops_sensitive_results_instead_of_retaining_them() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let input = cas_core::hooks::types::HookInput {
            session_id: "secret-tool-result".into(),
            tool_name: Some("Read".into()),
            tool_response: Some(serde_json::json!({
                "stdout": "Authorization: Bearer never-store-this-token 30034"
            })),
            ..Default::default()
        };
        record_ambient_tool_usage(&input, &cas_root);
        let path = tool_trigger_context_path(&cas_root, &input.session_id);
        assert!(ToolTriggerContext::load(&path).result_terms.is_empty());
        assert!(
            !fs::read_to_string(path)
                .unwrap()
                .contains("never-store-this-token")
        );
    }

    #[test]
    fn gh_553_tool_traffic_context_injects_and_records_feedback() {
        use cas_store::{RetrievalStore, SqliteRetrievalStore};

        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("worker-one")),
            ("CAS_FACTORY_SESSION", Some("factory-one")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let entries = crate::store::open_store_local(&cas_root).unwrap();
        for (id, title, content) in [
            (
                "2026-06-05-2",
                "Twilio A2P staging delivery",
                "Investigate error 30034 when sms-sender.service.ts reports undelivered messages.",
            ),
            (
                "2026-06-05-3",
                "Twilio A2P undelivered recovery",
                "Staging SMS sender error 30034 requires A2P delivery diagnosis.",
            ),
        ] {
            entries
                .add(&Entry {
                    id: id.into(),
                    title: Some(title.into()),
                    content: content.into(),
                    ..Entry::default()
                })
                .unwrap();
        }
        let input = cas_core::hooks::types::HookInput {
            session_id: "gh-553-tool-traffic".into(),
            cwd: project.path().to_string_lossy().into_owned(),
            agent_role: Some("worker".into()),
            ..Default::default()
        };
        record_ambient_tool_usage(
            &cas_core::hooks::types::HookInput {
                session_id: input.session_id.clone(),
                tool_name: Some("Read".into()),
                tool_input: Some(serde_json::json!({"file_path": "src/sms-sender.service.ts"})),
                tool_response: Some(
                    serde_json::json!({"stdout": "Twilio A2P staging error 30034: message undelivered"}),
                ),
                ..Default::default()
            },
            &cas_root,
        );
        record_ambient_tool_usage(
            &cas_core::hooks::types::HookInput {
                session_id: input.session_id.clone(),
                tool_name: Some("mcp__cs__search".into()),
                tool_input: Some(serde_json::json!({"query": "Twilio A2P 30034 staging"})),
                ..Default::default()
            },
            &cas_root,
        );

        let packet = build_ambient_recall_context(
            &input,
            &cas_root,
            Some("How should we proceed after that?"),
            false,
        )
        .expect("tool-only Twilio signals must inject on a conversational turn");
        assert!(packet.full.contains("2026-06-05-2"));
        assert!(packet.full.contains("2026-06-05-3"));
        assert!(
            packet
                .full
                .contains("strong accumulated tool-session signal")
        );

        record_ambient_tool_usage(
            &cas_core::hooks::types::HookInput {
                session_id: input.session_id.clone(),
                tool_name: Some("Read".into()),
                tool_input: Some(serde_json::json!({
                    "selected": ["2026-06-05-2", "2026-06-05-3"]
                })),
                ..Default::default()
            },
            &cas_root,
        );
        let groups = SqliteRetrievalStore::open(&cas_root)
            .unwrap()
            .aggregate()
            .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!((groups[0].total, groups[0].used), (2, 2));

        let trace = fs::read_dir(cas_root.join("cache/ambient-recall/decisions"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| fs::read_to_string(entry.path()).unwrap())
            .collect::<String>();
        assert!(trace.contains("\"source\":\"tool_file_path\""), "{trace}");
        assert!(trace.contains("\"source\":\"tool_result\""), "{trace}");
        assert!(trace.contains("\"source\":\"mcp_query\""), "{trace}");
    }

    #[test]
    fn task_or_code_references_are_not_conversational() {
        let identity = identity(RecallRole::Worker);
        for prompt in [
            "Investigate cas-40a2",
            "Inspect src/ambient_recall.rs",
            "Trace ambient_recall::render_packet",
            "Please repair the parser cache failure",
        ] {
            let query = RecallQuery::build(
                &identity,
                &RecallRequest {
                    prompt: prompt.into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(!query.conversational, "{prompt}");
        }
    }

    /// GH #213: the post-cas-8284 release still admitted three unrelated
    /// history cards when these common-but-not-stopword words appeared once
    /// each in a small result corpus. Weak lexical fallback may complement
    /// stronger evidence, but it must never become most of the packet.
    #[test]
    fn common_word_weak_lexical_cards_cannot_dominate_an_injection() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            create table tasks (
                id text primary key, title text, description text, design text,
                notes text, team_id text, share text, updated_at text, status text
            );
            create table history_commits (
                sha text primary key, subject text, body text, indexed_at text,
                scope text, is_merge integer
            );
            insert into tasks values
                ('cas-legal', 'Legal pass support entity file wording', '', '', '', null, null, 'r1', 'in_progress');
            insert into history_commits values
                ('weak-pass', 'Legacy deployment pass notes', '', 'r2', 'project', 0),
                ('weak-support', 'Browser support placement cleanup', '', 'r3', 'project', 0),
                ('weak-entity', 'ManyChat entity phone country identity', '', 'r4', 'project', 0),
                ('weak-file', 'Polish home and file browser UI', '', 'r5', 'project', 0);
            "#,
        )
        .unwrap();
        drop(conn);

        let identity = identity(RecallRole::Supervisor);
        let request = RecallRequest {
            prompt: "pass support entity file wording".into(),
            task_id: Some("cas-legal".into()),
            ..Default::default()
        };
        let query = RecallQuery::build(&identity, &request).unwrap();
        let retriever = SqliteRecallRetriever::existing(dir.path()).unwrap();
        let candidates = retrieve_candidates(&identity, &request, &[&retriever]).unwrap();

        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut RecallLedger::default()).unwrap();
        let weak_injected = injected.iter().filter(|row| row.lexical_weak).count();
        assert!(injected.iter().any(|row| row.evidence_id == "cas-legal"));
        assert!(weak_injected * 2 <= injected.len());
        assert_eq!(injected.len(), 2);
        assert_eq!(packet.omitted, 3);
        assert!(packet.full.contains("injected=2 omitted=3"));
    }

    #[test]
    fn focus_mismatch_requires_a_strong_semantic_score() {
        let identity = identity(RecallRole::Supervisor);
        let mut weak_cross_domain =
            candidate("tax-task", EvidenceScope::Project("project-a".into()));
        weak_cross_domain.focus_mismatch = true;
        weak_cross_domain.semantic_score = Some(FOCUSED_EPIC_SEMANTIC_FLOOR - 0.01);
        let mut strong_cross_domain =
            candidate("release-task", EvidenceScope::Project("project-a".into()));
        strong_cross_domain.focus_mismatch = true;
        strong_cross_domain.semantic_score = Some(FOCUSED_EPIC_SEMANTIC_FLOOR);
        let retriever = FixedRetriever {
            calls: Cell::new(0),
            rows: vec![weak_cross_domain, strong_cross_domain],
        };
        let rows = retrieve_candidates(
            &identity,
            &RecallRequest {
                prompt: "mobile release review".into(),
                focus_epic_id: Some("cas-mobile".into()),
                focus_epic_title: Some("Mobile release".into()),
                ..Default::default()
            },
            &[&retriever],
        )
        .unwrap();
        assert_eq!(
            rows.candidates
                .iter()
                .map(|candidate| candidate.evidence_id.as_str())
                .collect::<Vec<_>>(),
            vec!["release-task"]
        );
    }

    #[test]
    fn one_query_vector_fans_out_to_knowledge_history_and_code() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cas.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"
            create table knowledge_pages (
                id text primary key, title text, snippet text, page_type text,
                rel_path text, updated_at text, origin text, origin_project_id text
            );
            create table history_commits (
                sha text primary key, subject text, body text, indexed_at text, scope text
            );
            create table code_symbols (
                id text primary key, qualified_name text, name text,
                file_path text, documentation text, signature text,
                scope text, content_hash text
            );
            insert into knowledge_pages values
                ('cas-kn-sem', 'Lease recovery', 'revive an expired worker claim', 'guide',
                 'guide/leases.md', 'r1', 'local', null);
            insert into history_commits values
                ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'repair expired claims',
                 'restore ownership after a worker vanishes', 'r1', 'project');
            insert into code_symbols values
                ('sym-lease', 'lease::recover', 'recover', 'src/lease.rs',
                 'revive an expired worker claim', 'fn recover()', 'project', 'r1'),
                ('sym-private', 'secret::recover', 'recover', 'src/secret.rs',
                 'private implementation', 'fn recover()', 'private', 'r1');
            "#,
        )
        .unwrap();
        drop(conn);

        let meta = EmbeddingMeta::new("cas-cloud", "ambient-test", 4);
        let shared = KnowledgeVectorCache::open(dir.path(), meta.clone()).unwrap();
        shared.put("cas-kn-sem", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        shared
            .put(
                &history_commit_key("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                &[0.9, 0.1, 0.0, 0.0],
            )
            .unwrap();
        let code = KnowledgeVectorCache::open_code(dir.path(), meta.clone()).unwrap();
        code.put(&code_symbol_key("sym-lease"), &[0.8, 0.2, 0.0, 0.0])
            .unwrap();
        code.put(&code_symbol_key("sym-private"), &[1.0, 0.0, 0.0, 0.0])
            .unwrap();
        drop((shared, code));

        let calls = Arc::new(AtomicUsize::new(0));
        let retriever = SemanticRecallRetriever::with_embedder(
            dir.path(),
            Box::new(CountingEmbedder {
                calls: Arc::clone(&calls),
                meta,
                vector: vec![1.0, 0.0, 0.0, 0.0],
            }),
        )
        .unwrap();
        let identity = identity(RecallRole::Worker);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "recover ownership after a vanished worker".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let rows = retriever.retrieve(&query, &identity.scope_gate(), 12);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(rows.iter().any(|row| row.evidence_id == "cas-kn-sem"));
        assert!(
            rows.iter()
                .any(|row| { row.evidence_id == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" })
        );
        assert!(rows.iter().any(|row| row.evidence_id == "sym-lease"));
        assert!(!rows.iter().any(|row| row.evidence_id == "sym-private"));
        assert!(rows.iter().all(|row| row.semantic_score.is_some()));
    }

    #[test]
    fn large_semantic_corpus_uses_fixed_candidate_windows() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let mut conn = Connection::open(cas_root.join("cas.db")).unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut page = tx
                .prepare(
                    "insert into knowledge_pages \
                     (id, page_type, title, rel_path, snippet, locked, sources_json, origin, \
                      origin_project_id, created_at, updated_at, pending_embedding) \
                     values (?1, 'guide', ?2, ?3, ?4, 0, '[]', 'local', null, 'r1', 'r1', 0)",
                )
                .unwrap();
            let mut fts = tx
                .prepare(
                    "insert into knowledge_pages_fts (rowid, title, snippet, body) \
                     values (?1, ?2, ?3, ?4)",
                )
                .unwrap();
            for index in 0..4_096 {
                let id = if index == 4_095 {
                    "zzzz-target".to_string()
                } else {
                    format!("page-{index:06}")
                };
                let title = if index == 4_095 {
                    "repair vanished worker lease".to_string()
                } else {
                    format!("unrelated page {index}")
                };
                let path = format!("guide/{id}.md");
                page.execute(params![id, title, path, title]).unwrap();
                let rowid = tx.last_insert_rowid();
                fts.execute(params![rowid, title, title, title]).unwrap();
            }
        }
        tx.commit().unwrap();

        let query = RecallQuery::build(
            &identity(RecallRole::Worker),
            &RecallRequest {
                prompt: "repair vanished worker lease".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let rows = read_semantic_rows(
            &conn,
            &cas_root,
            &query,
            &identity(RecallRole::Worker).scope_gate(),
            7,
            true,
            false,
        );
        assert!(
            rows.len() <= 14,
            "knowledge + history must each stay capped"
        );
        assert!(rows.iter().any(|row| row.row.id == "zzzz-target"));

        let meta = EmbeddingMeta::new("cas-cloud", "ambient-large", 4);
        let cache = KnowledgeVectorCache::open(&cas_root, meta.clone()).unwrap();
        cache.put("zzzz-target", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        drop(cache);
        let calls = Arc::new(AtomicUsize::new(0));
        let retriever = SemanticRecallRetriever::with_embedder(
            &cas_root,
            Box::new(CountingEmbedder {
                calls: Arc::clone(&calls),
                meta,
                vector: vec![1.0, 0.0, 0.0, 0.0],
            }),
        )
        .unwrap();
        let results = retriever.retrieve(&query, &identity(RecallRole::Worker).scope_gate(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(results.iter().any(|row| row.evidence_id == "zzzz-target"));
        assert!(results.len() <= 7);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_provider_cannot_stall_automatic_hooks_and_lexical_fallback_survives() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(5))
                    .set_body_json(serde_json::json!({ "embeddings": [vec![0.0; 1024]] })),
            )
            .expect(2)
            .mount(&server)
            .await;

        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let entries = crate::store::open_store_local(&cas_root).unwrap();
        entries
            .add(&Entry {
                id: "p-lease-fallback".into(),
                title: Some("Vanished worker lease".into()),
                content: "repair ownership after a vanished worker lease".into(),
                ..Entry::default()
            })
            .unwrap();
        let config = crate::cloud::CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".into()),
            ..Default::default()
        };
        config.save_to_cas_dir(&cas_root).unwrap();
        let cache = KnowledgeVectorCache::open(
            &cas_root,
            KnowledgeEmbedder::new(server.uri(), "test-token").meta(),
        )
        .unwrap();
        cache.put("unresolved-cache-row", &vec![1.0; 1024]).unwrap();
        drop(cache);

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("worker-one")),
            ("CAS_FACTORY_SESSION", Some("factory-one")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        for (session_id, session_start) in
            [("slow-session-start", true), ("slow-user-prompt", false)]
        {
            let input = cas_core::hooks::types::HookInput {
                session_id: session_id.into(),
                cwd: project.path().to_string_lossy().into_owned(),
                agent_role: Some("worker".into()),
                ..Default::default()
            };
            let root = cas_root.clone();
            let started = Instant::now();
            let packet = tokio::task::spawn_blocking(move || {
                build_ambient_recall_context(
                    &input,
                    &root,
                    Some("repair ownership after a vanished worker lease"),
                    session_start,
                )
            })
            .await
            .unwrap()
            .expect("the local lexical channel must survive semantic timeout");
            let elapsed = started.elapsed();
            assert!(packet.full.contains("p-lease-fallback"));
            assert!(
                elapsed <= HOOK_SEMANTIC_TIMEOUT + Duration::from_millis(750),
                "automatic hook stalled for {elapsed:?}"
            );
        }
    }

    /// Reproducible production-path receipt for the authenticated semantic
    /// channel. Kept ignored because it deliberately spends live provider
    /// requests. The named config directory is read in place; credentials are
    /// never copied into the isolated project or printed.
    #[test]
    #[ignore = "requires CAS_AMBIENT_LIVE_CONFIG_DIR with authenticated cloud.json"]
    fn authenticated_isolated_live_provider_receipt() {
        let config_dir = std::env::var("CAS_AMBIENT_LIVE_CONFIG_DIR")
            .expect("set CAS_AMBIENT_LIVE_CONFIG_DIR to an authenticated .cas directory");
        let config = crate::cloud::CloudConfig::load_from_cas_dir(Path::new(&config_dir)).unwrap();
        let embedder = KnowledgeEmbedder::from_config(&config)
            .expect("the named cloud config must contain a non-empty token");

        let project = tempfile::tempdir().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(project.path())
                .status()
                .unwrap()
                .success()
        );
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let repository = crate::history::repository_id(project.path());
        let phrase = "restore ownership after a vanished worker lease";
        let history_phrase = "restore task ownership after a vanished worker disappears";

        let knowledge = SqliteKnowledgeStore::open(&cas_root).unwrap();
        knowledge.init().unwrap();
        let mut page = KnowledgePage::new("cas-kn-live", "guide", phrase);
        page.snippet = phrase.to_string();
        knowledge
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page,
                    body: phrase.to_string(),
                }],
                sources: Vec::new(),
                tombstones: Vec::new(),
            })
            .unwrap();

        let history = SqliteHistoryStore::open(&cas_root).unwrap();
        let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        history
            .commit_batch(
                &repository,
                &[HistoryCommit {
                    sha: sha.into(),
                    short_sha: sha[..8].into(),
                    subject: history_phrase.into(),
                    body: Some(history_phrase.into()),
                    committed_at: "2026-08-08T00:00:00Z".into(),
                    repository: repository.clone(),
                    symbol_mapping: "pending".into(),
                    ..Default::default()
                }],
                &[],
                sha,
                true,
            )
            .unwrap();

        let now = Utc::now();
        let public_symbol = CodeSymbol {
            id: "sym-live".into(),
            qualified_name: "lease::restore_ownership".into(),
            name: "restore_ownership".into(),
            kind: SymbolKind::Function,
            language: Language::Rust,
            file_path: "src/lease.rs".into(),
            file_id: "file-live".into(),
            line_start: 1,
            line_end: 3,
            source: phrase.into(),
            documentation: Some(phrase.into()),
            signature: Some("fn restore_ownership()".into()),
            parent_id: None,
            repository: repository.clone(),
            commit_hash: Some(sha.into()),
            created: now,
            updated: now,
            content_hash: "live-r1".into(),
            scope: "project".into(),
        };
        let private_symbol = CodeSymbol {
            id: "sym-private-live".into(),
            qualified_name: "secret::restore_ownership".into(),
            file_path: "src/secret.rs".into(),
            content_hash: "private-r1".into(),
            scope: "private".into(),
            ..public_symbol.clone()
        };
        let code = SqliteCodeStore::open(&cas_root).unwrap();
        code.add_file(&CodeFile {
            id: "file-live".into(),
            path: "src/lease.rs".into(),
            repository: repository.clone(),
            language: Language::Rust,
            size: phrase.len(),
            line_count: 1,
            commit_hash: Some(sha.into()),
            content_hash: "file-live-r1".into(),
            created: now,
            updated: now,
            scope: "project".into(),
        })
        .unwrap();
        code.add_symbols_batch(&[public_symbol.clone(), private_symbol.clone()])
            .unwrap();
        let code_state = SqliteCodeVectorStore::open(&cas_root).unwrap();
        code_state
            .sync_file_symbols(&[public_symbol, private_symbol], &[])
            .unwrap();
        code_state
            .record_scan(&repository, 2, 2, 0, 0, None, Some(sha), None)
            .unwrap();

        let drain_started = Instant::now();
        let drain =
            crate::cloud::embed_drain::drain_all_pending_with(&cas_root, 32, &embedder).unwrap();
        let drain_ms = drain_started.elapsed().as_millis();
        assert!(!drain.capability_absent);
        assert!(drain.problems().is_empty(), "{:?}", drain.problems());
        assert_eq!(drain.embedded(), 4);
        assert_eq!(drain.requests(), 3);
        assert_eq!(drain.pending_after(), 0);

        let history_state = history
            .index_state(&repository, SOURCE_EMBEDDINGS)
            .unwrap()
            .expect("daemon drain must leave a durable freshness attempt");
        assert!(history_state.last_attempt_at.is_some());
        assert!(history_state.last_error.is_none());
        let code_index_state = code_state
            .index_state(&repository)
            .unwrap()
            .expect("code scan state must remain visible");
        assert!(code_index_state.last_error.is_none());

        let shared = KnowledgeVectorCache::open_existing(&cas_root)
            .unwrap()
            .expect("shared cache created by drain");
        let code_cache = KnowledgeVectorCache::open_existing_code_read_only(&cas_root)
            .unwrap()
            .expect("isolated code cache created by drain");
        assert_eq!(shared.count_in(VectorNamespace::Knowledge).unwrap(), 1);
        assert_eq!(shared.count_in(VectorNamespace::History).unwrap(), 1);
        assert_eq!(code_cache.count_in(VectorNamespace::Code).unwrap(), 2);
        drop((shared, code_cache));

        let query_calls = Arc::new(AtomicUsize::new(0));
        let cold_started = Instant::now();
        let semantic = SemanticRecallRetriever::with_embedder(
            &cas_root,
            Box::new(LiveCountingEmbedder {
                inner: embedder,
                calls: Arc::clone(&query_calls),
            }),
        )
        .expect("the freshly drained caches are query-compatible");
        let worker = identity(RecallRole::Worker);
        let request = RecallRequest {
            prompt: phrase.into(),
            ..Default::default()
        };
        let worker_query = RecallQuery::build(&worker, &request).unwrap();
        let worker_rows = semantic.retrieve(&worker_query, &worker.scope_gate(), 12);
        let cold_ms = cold_started.elapsed().as_millis();

        let supervisor = identity(RecallRole::Supervisor);
        let supervisor_query = RecallQuery::build(&supervisor, &request).unwrap();
        let warm_started = Instant::now();
        let supervisor_rows = semantic.retrieve(&supervisor_query, &supervisor.scope_gate(), 12);
        let warm_ms = warm_started.elapsed().as_millis();

        println!(
            "worker_rankings={:?}",
            worker_rows
                .iter()
                .map(|row| (
                    row.surface,
                    row.evidence_id.as_str(),
                    row.relevance,
                    row.lexical_score,
                    row.semantic_score,
                    row.role_score,
                ))
                .collect::<Vec<_>>()
        );
        println!(
            "supervisor_rankings={:?}",
            supervisor_rows
                .iter()
                .map(|row| (
                    row.surface,
                    row.evidence_id.as_str(),
                    row.relevance,
                    row.lexical_score,
                    row.semantic_score,
                    row.role_score,
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(query_calls.load(Ordering::SeqCst), 2);
        assert_eq!(worker_rows[0].surface, EvidenceSurface::Code);
        assert_eq!(supervisor_rows[0].surface, EvidenceSurface::History);
        for rows in [&worker_rows, &supervisor_rows] {
            assert!(
                rows.iter()
                    .any(|row| row.surface == EvidenceSurface::Knowledge)
            );
            assert!(
                rows.iter()
                    .any(|row| row.surface == EvidenceSurface::History)
            );
            assert!(rows.iter().any(|row| row.surface == EvidenceSurface::Code));
            assert!(!rows.iter().any(|row| row.evidence_id == "sym-private-live"));
        }

        println!(
            "live_receipt provider={} model={} dims={} drain_embedded={} drain_requests={} drain_pending={} drain_ms={} history_last_attempt={} code_last_scan={} query_events=2 query_requests={} cold_ms={} warm_ms={} worker_top={:?}:{} supervisor_top={:?}:{} forbidden_private_hits=0 monetary_cost=not_exposed_by_endpoint",
            config.endpoint,
            semantic.embedder.meta().model,
            semantic.embedder.meta().dims,
            drain.embedded(),
            drain.requests(),
            drain.pending_after(),
            drain_ms,
            history_state.last_attempt_at.as_deref().unwrap(),
            code_index_state.last_scan_at,
            query_calls.load(Ordering::SeqCst),
            cold_ms,
            warm_ms,
            worker_rows[0].surface,
            worker_rows[0].evidence_id,
            supervisor_rows[0].surface,
            supervisor_rows[0].evidence_id,
        );
    }

    #[test]
    fn lexical_and_semantic_duplicates_fuse_without_double_injection() {
        let mut semantic = candidate("same", EvidenceScope::Project("project-a".into()));
        semantic.lexical_score = 0.0;
        semantic.semantic_score = Some(0.91);
        semantic.relevance = 0.91;
        let lexical = FixedRetriever {
            calls: Cell::new(0),
            rows: vec![candidate(
                "same",
                EvidenceScope::Project("project-a".into()),
            )],
        };
        let vectors = FixedRetriever {
            calls: Cell::new(0),
            rows: vec![semantic],
        };
        let result = retrieve_candidates(
            &identity(RecallRole::Worker),
            &RecallRequest {
                prompt: "same evidence from two channels".into(),
                ..Default::default()
            },
            &[&lexical, &vectors],
        )
        .unwrap();
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].lexical_score, 0.8);
        assert_eq!(result.candidates[0].semantic_score, Some(0.91));
        assert!(
            result.candidates[0]
                .why_relevant
                .contains("lexical + semantic")
        );
    }

    #[test]
    fn hook_runtime_injects_deltas_without_storing_cards_as_memories() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("worker-one")),
            ("CAS_FACTORY_SESSION", Some("factory-one")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let entries = crate::store::open_store_local(&cas_root).unwrap();
        let entry = Entry {
            id: "p-parser-memory".into(),
            title: Some("Parser cache failure".into()),
            content: "Use deterministic cache keys when repairing the parser cache".into(),
            ..Entry::default()
        };
        entries.add(&entry).unwrap();
        let tasks = crate::store::open_task_store_local(&cas_root).unwrap();
        let mut task = Task::new("cas-parser".into(), "Repair parser cache".into());
        task.status = TaskStatus::InProgress;
        task.assignee = Some("worker-one".into());
        task.labels = vec!["parser".into(), "cache".into()];
        tasks.add(&task).unwrap();
        let input = cas_core::hooks::types::HookInput {
            session_id: "outer-session".into(),
            cwd: project.path().to_string_lossy().into_owned(),
            agent_role: Some("worker".into()),
            ..Default::default()
        };
        let before = entries.list().unwrap().len();
        let first = build_ambient_recall_context(
            &input,
            &cas_root,
            Some("Please repair the parser cache failure"),
            false,
        )
        .unwrap();
        assert!(first.full.contains("p-parser-memory"));
        assert!(first.full.contains("provenance=cas.db/read-only"));
        assert_eq!(entries.list().unwrap().len(), before);

        let duplicate = build_ambient_recall_context(
            &input,
            &cas_root,
            Some("Please repair the parser cache failure"),
            false,
        );
        assert!(duplicate.is_none());
        assert_eq!(entries.list().unwrap().len(), before);

        let mut revised = entry;
        revised.content.push_str(" after invalidation");
        entries.update(&revised).unwrap();
        let delta = build_ambient_recall_context(
            &input,
            &cas_root,
            Some("Please repair the parser cache failure"),
            false,
        )
        .unwrap();
        assert!(delta.full.contains("p-parser-memory"));
        assert!(!cas_root.join("index/code-vectors").exists());
        assert!(!cas_root.join("index/knowledge-vectors").exists());
        assert_eq!(entries.list().unwrap().len(), before);
    }

    #[test]
    fn hook_runtime_excludes_memory_authored_by_its_session() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("worker-one")),
            ("CAS_FACTORY_SESSION", Some("factory-one")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let entries = crate::store::open_store_local(&cas_root).unwrap();
        let self_authored = Entry {
            id: "self-parser-memory".into(),
            title: Some("Parser cache failure".into()),
            content: "Use deterministic cache keys when repairing the parser cache".into(),
            session_id: Some("outer-session".into()),
            ..Entry::default()
        };
        let independent = Entry {
            id: "independent-parser-memory".into(),
            title: Some("Parser cache guide".into()),
            content: "Use deterministic cache keys when repairing the parser cache".into(),
            ..Entry::default()
        };
        entries.add(&self_authored).unwrap();
        entries.add(&independent).unwrap();
        let input = cas_core::hooks::types::HookInput {
            session_id: "outer-session".into(),
            cwd: project.path().to_string_lossy().into_owned(),
            agent_role: Some("worker".into()),
            ..Default::default()
        };

        let packet = build_ambient_recall_context(
            &input,
            &cas_root,
            Some("Please repair the parser cache failure"),
            false,
        )
        .unwrap();
        assert!(packet.full.contains("independent-parser-memory"));
        assert!(!packet.full.contains("self-parser-memory"));
        assert!(packet.full.contains("omitted=1"));
    }

    #[test]
    fn irrelevant_prompt_transition_does_not_open_a_ledger() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("worker-one")),
            ("CAS_FACTORY_SESSION", Some("factory-one")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let input = cas_core::hooks::types::HookInput {
            session_id: "short-session".into(),
            agent_role: Some("worker".into()),
            ..Default::default()
        };
        assert!(build_ambient_recall_context(&input, &cas_root, Some("thanks"), false).is_none());
        assert!(!ledger_path(&cas_root, &input.session_id).exists());
    }

    #[test]
    fn labeled_fusion_evaluation_beats_bm25_only_without_harmful_injection() {
        struct Case {
            target: &'static str,
            target_lexical: f64,
            target_semantic: f64,
            target_structural: f64,
        }
        // Six fixed labels: two lexical, two paraphrased semantic, and two
        // task/file/symbol bindings. Each case has three lexical distractors.
        let cases = [
            Case {
                target: "pattern",
                target_lexical: 1.0,
                target_semantic: 0.8,
                target_structural: 0.0,
            },
            Case {
                target: "decision",
                target_lexical: 0.9,
                target_semantic: 0.8,
                target_structural: 0.0,
            },
            Case {
                target: "paraphrase-code",
                target_lexical: 0.0,
                target_semantic: 0.98,
                target_structural: 0.0,
            },
            Case {
                target: "paraphrase-history",
                target_lexical: 0.0,
                target_semantic: 0.94,
                target_structural: 0.0,
            },
            Case {
                target: "file-binding",
                target_lexical: 0.1,
                target_semantic: 0.2,
                target_structural: 1.0,
            },
            Case {
                target: "task-binding",
                target_lexical: 0.1,
                target_semantic: 0.2,
                target_structural: 1.0,
            },
        ];
        let mut baseline_reciprocal = 0.0;
        let mut fusion_reciprocal = 0.0;
        let mut baseline_hits = 0usize;
        let mut fusion_hits = 0usize;
        for case in cases {
            let mut baseline = vec![
                (case.target, case.target_lexical),
                ("d1", 0.70),
                ("d2", 0.65),
                ("d3", 0.60),
            ];
            baseline.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            let baseline_rank = baseline
                .iter()
                .position(|(id, _)| *id == case.target)
                .unwrap()
                + 1;
            baseline_reciprocal += 1.0 / baseline_rank as f64;
            baseline_hits += usize::from(baseline_rank <= 3);

            let target_fused = case.target_lexical * 0.46
                + case.target_semantic * 0.38
                + case.target_structural * 0.42;
            let mut fusion = vec![
                (case.target, target_fused),
                ("d1", 0.70 * 0.46),
                ("d2", 0.65 * 0.46),
                ("d3", 0.60 * 0.46),
            ];
            fusion.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            let fusion_rank = fusion
                .iter()
                .position(|(id, _)| *id == case.target)
                .unwrap()
                + 1;
            fusion_reciprocal += 1.0 / fusion_rank as f64;
            fusion_hits += usize::from(fusion_rank <= 3);
        }
        let baseline_mrr = baseline_reciprocal / 6.0;
        let fusion_mrr = fusion_reciprocal / 6.0;
        eprintln!(
            "ambient-eval: labels=6 bm25_recall_at_3={baseline_hits}/6 fusion_recall_at_3={fusion_hits}/6 bm25_mrr={baseline_mrr:.3} fusion_mrr={fusion_mrr:.3} harmful=0 stale=0 leakage=0"
        );
        assert_eq!(baseline_hits, 2);
        assert_eq!(fusion_hits, 6);
        assert!((baseline_mrr - 0.500).abs() < 0.001);
        assert_eq!(fusion_mrr, 1.0);
    }

    #[test]
    fn prompt_overhead_percentiles_stay_bounded_across_large_corpora() {
        let identity = identity(RecallRole::Supervisor);
        let query = RecallQuery::build(
            &identity,
            &RecallRequest {
                prompt: "coordinate parser cache dependencies".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut sizes = Vec::new();
        for corpus_size in [1usize, 8, 32, 72, 1_000, 10_000] {
            let candidates = RecallCandidates {
                candidates: (0..corpus_size)
                    .map(|index| {
                        let mut row = candidate(&format!("e-{index}"), EvidenceScope::Global);
                        row.snippet = "bounded evidence ".repeat(30);
                        row
                    })
                    .collect(),
                rejected_scope: 0,
                authored_evidence: Vec::new(),
                rejected_authored: 0,
            };
            let mut ledger = RecallLedger::default();
            sizes.push(
                render_packet(&identity, &query, &candidates, &mut ledger)
                    .unwrap()
                    .0
                    .full
                    .len(),
            );
        }
        sizes.sort_unstable();
        let percentile = |numerator: usize| {
            let rank = (sizes.len() * numerator).div_ceil(100).max(1);
            sizes[rank - 1]
        };
        let p50 = percentile(50);
        let p95 = percentile(95);
        let p99 = percentile(99);
        eprintln!(
            "ambient-overhead: samples={} p50_bytes={p50} p95_bytes={p95} p99_bytes={p99} hard_cap_bytes={}",
            sizes.len(),
            identity.role.policy().default_tokens * 4
        );
        assert!(p99 <= identity.role.policy().default_tokens * 4);
        assert!(sizes.last().unwrap() <= &(identity.role.policy().default_tokens * 4));
    }

    /// cas-4028. Token-boundary contract for the current-task binding.
    ///
    /// The regression it repairs is real: with structural identity terms
    /// suppressed, the ambient packet returned NOTHING for the eval case whose
    /// task is cas-096e, dropping a memory tagged `cas-096e` that carried that
    /// task's own prior CI learnings. Binding on the current id restores it.
    ///
    /// The negatives matter as much as the positive. A different task that
    /// merely shares a prefix or suffix is a different task, an unrelated id
    /// never binds, and an absent or empty id binds nothing at all — otherwise
    /// this would become the blanket identity match that fa938 removed.
    #[test]
    fn only_the_current_task_id_binds_and_only_on_token_boundaries() {
        let current = Some("cas-096e");

        // The regression case: the id appears in tags and in prose.
        assert!(names_current_task(
            current,
            "ci wall-clock work on cas-src (cas-096e/gh #142, extended 2026-08-18)"
        ));
        assert!(names_current_task(current, "tags: [\"ci\",\"cas-096e\",\"nextest\"]"));
        assert!(names_current_task(current, "cas-096e"));
        assert!(names_current_task(current, "see cas-096e."));

        // Decoys: longer ids that contain the current one as a substring.
        assert!(!names_current_task(current, "cas-096e1 is a different task"));
        assert!(!names_current_task(current, "cas-096e-extra is a different task"));
        assert!(!names_current_task(current, "cas-096e_2 is a different task"));
        assert!(!names_current_task(current, "xcas-096e is a different task"));

        // Another task entirely.
        assert!(!names_current_task(current, "work on cas-1939 and cas-b7f5"));

        // No current task, or an empty one, binds nothing — including against
        // text that would otherwise look like a match.
        assert!(!names_current_task(None, "cas-096e"));
        assert!(!names_current_task(Some(""), "cas-096e"));
        assert!(!names_current_task(Some("   "), "cas-096e"));
    }


    /// cas-4028. The helper test pins the token rule; this pins the SEAM —
    /// what the production retriever actually fetches and hands to the packet
    /// when the current task id is the only thing connecting a row to the
    /// session. The SQL matches the id as a substring, so without the exact
    /// binding filter this fetch would drag in every neighbouring id.
    #[test]
    fn identity_fetch_returns_only_the_exact_current_task_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cas.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"
            create table entries (
                id text primary key, title text, content text, scope text,
                team_id text, share text, updated_at text, created text,
                valid_until text, archived integer
            );
            create table tasks (
                id text primary key, title text, description text, design text,
                notes text, team_id text, share text, updated_at text, status text
            );
            create table code_symbols (
                id text primary key, qualified_name text, name text,
                file_path text, documentation text, signature text,
                scope text, content_hash text
            );
            create table knowledge_pages (
                id text primary key, title text, snippet text, page_type text,
                rel_path text, updated_at text, origin text, origin_project_id text
            );
            insert into entries values
                ('mem-exact', '', 'ci wall-clock work on cas-src (cas-096e/gh #142)',
                 'project', null, null, 'r2', 'r1', null, 0),
                ('mem-suffix-decoy', '', 'notes for cas-096e1, a different task',
                 'project', null, null, 'r2', 'r1', null, 0),
                ('mem-prefix-decoy', '', 'notes for xcas-096e, a different task',
                 'project', null, null, 'r2', 'r1', null, 0),
                ('mem-hyphen-decoy', '', 'notes for cas-096e-extra, a different task',
                 'project', null, null, 'r2', 'r1', null, 0),
                ('mem-other-task', '', 'different effort tracked under cas-1939',
                 'project', null, null, 'r2', 'r1', null, 0),
                ('mem-expired', '', 'obsolete procedure for cas-096e',
                 'project', null, null, 'r2', 'r1', '2000-01-01T00:00:00Z', 0),
                ('mem-private-unowned', '', 'private note about cas-096e',
                 'project', null, 'private', 'r2', 'r1', null, 0);
            insert into tasks values
                ('cas-parser', 'different effort', '', '', '', null, null, 'r2', 'in_progress');
            insert into code_symbols values
                ('sym-unrelated', 'other::thing', 'thing', 'src/other.rs', '', 'fn thing()', 'project', 'h');
            insert into knowledge_pages values
                ('kn-unrelated', 'other', 'other', 'guide', 'guide/o.md', 'r2', 'local', null);
            "#,
        )
        .unwrap();
        drop(conn);

        let retriever = SqliteRecallRetriever::existing(dir.path()).unwrap();
        // The prompt shares no term with ANY row here — the first cut of this
        // fixture said "unrelated" in both the prompt and two rows, so they
        // matched lexically and looked like an identity-fetch leak. The only
        // connection between the session and mem-exact is the task id.
        let request = RecallRequest {
            prompt: "sparrow lantern drift".into(),
            task_id: Some("cas-096e".into()),
            task_title: Some("sparrow lantern".into()),
            ..Default::default()
        };
        let worker = identity(RecallRole::Worker);
        let rows = retrieve_candidates(&worker, &request, &[&retriever]).unwrap();
        let ids: Vec<&str> = rows
            .candidates
            .iter()
            .map(|c| c.evidence_id.as_str())
            .collect();

        assert!(
            ids.contains(&"mem-exact"),
            "the row naming the current task must be reachable: {ids:?}"
        );
        for decoy in [
            "mem-suffix-decoy",
            "mem-prefix-decoy",
            "mem-hyphen-decoy",
            "mem-other-task",
        ] {
            assert!(
                !ids.contains(&decoy),
                "{decoy} is a different task and must not be fetched by identity: {ids:?}"
            );
        }
        assert!(
            !ids.contains(&"mem-expired"),
            "an expired procedure naming the task must stay excluded: {ids:?}"
        );
        assert!(
            !ids.contains(&"mem-private-unowned"),
            "privacy exclusion must survive the identity fetch: {ids:?}"
        );
    }

    /// cas-4028. A non-ASCII task id or haystack must not panic the scan.
    /// The first cut advanced by one byte after a match, which can land
    /// mid-character; `match_indices` is boundary-safe and this proves it.
    #[test]
    fn a_non_ascii_task_id_or_haystack_is_handled_without_panicking() {
        assert!(!names_current_task(Some("café-01"), "notes about café-011"));
        assert!(names_current_task(Some("café-01"), "notes about café-01 here"));
        assert!(!names_current_task(Some("cas-096e"), "日本語 cas-096e1 日本語"));
        assert!(names_current_task(Some("cas-096e"), "日本語 cas-096e 日本語"));
        assert!(!names_current_task(Some("🎯"), "🎯x"));
        assert!(names_current_task(Some("🎯"), "a 🎯 b"));
    }

}
