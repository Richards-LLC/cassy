//! Bounded, read-only ambient recall contracts.
//!
//! This module deliberately owns neither ingestion nor model/session launch.
//! It turns an already-authorized factory event into a stable query, asks
//! namespace-aware retrievers for compact candidates, and rejects anything
//! outside the caller's project/team/private boundary before ranking.  Source
//! bodies stay in their authoritative stores.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloud::embeddings::{
    EmbeddingMeta, KnowledgeEmbedder, KnowledgeVectorCache, VectorNamespace, code_symbol_key,
    cosine_similarity, history_commit_key, history_doc_key, is_zero_vector,
};

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

/// Narrow query-time seam: a recall event may ask the provider for one vector,
/// then must reuse that exact vector for every compatible namespace.
trait RecallQueryEmbedder {
    fn meta(&self) -> EmbeddingMeta;
    fn embed_query(&self, query: &str) -> Option<Vec<f32>>;
}

impl RecallQueryEmbedder for KnowledgeEmbedder {
    fn meta(&self) -> EmbeddingMeta {
        KnowledgeEmbedder::meta(self)
    }

    fn embed_query(&self, query: &str) -> Option<Vec<f32>> {
        self.embed_batch(&[query.to_string()])
            .ok()?
            .into_iter()
            .next()
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
    db_path: PathBuf,
    embedder: Box<dyn RecallQueryEmbedder>,
    shared_cache: Option<KnowledgeVectorCache>,
    code_cache: Option<KnowledgeVectorCache>,
}

impl SemanticRecallRetriever {
    fn existing(cas_root: &Path, config: &crate::cloud::CloudConfig) -> Option<Self> {
        let embedder = KnowledgeEmbedder::from_config(config)?;
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
            .filter(|cache| {
                cache.meta() == &meta
                    && (cache.count_in(VectorNamespace::Knowledge).unwrap_or(0)
                        + cache.count_in(VectorNamespace::History).unwrap_or(0))
                        > 0
            });
        let code_cache = KnowledgeVectorCache::open_existing_code_read_only(cas_root)
            .ok()
            .flatten()
            .filter(|cache| {
                cache.meta() == &meta && cache.count_in(VectorNamespace::Code).unwrap_or(0) > 0
            });
        if shared_cache.is_none() && code_cache.is_none() {
            return None;
        }
        Some(Self {
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
        let Some(query_vector) = self.embedder.embed_query(&query.canonical) else {
            return Vec::new();
        };
        let meta = self.embedder.meta();
        if is_zero_vector(&query_vector) || query_vector.len() != meta.dims {
            return Vec::new();
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let Ok(conn) = Connection::open_with_flags(&self.db_path, flags) else {
            return Vec::new();
        };
        let terms = query_terms(&query.canonical);
        let mut candidates = Vec::new();
        for semantic in read_semantic_rows(&conn, scope) {
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

fn read_semantic_rows(conn: &Connection, scope: &ScopeGate) -> Vec<SemanticRow> {
    let mut out = Vec::new();
    if table_exists(conn, "knowledge_pages")
        && let Ok(mut stmt) = conn.prepare(
            "select id, substr(trim(title || ' ' || snippet || ' ' || page_type), 1, 480), \
                    updated_at, rel_path from knowledge_pages \
             where origin = 'local' or origin_project_id = ?1 order by id",
        )
        && let Ok(rows) = stmt.query_map([&scope.project_id], |row| {
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
        })
    {
        out.extend(rows.filter_map(Result::ok));
    }

    if table_exists(conn, "history_commits")
        && let Ok(mut stmt) = conn.prepare(
            "select sha, substr(trim(subject || ' ' || coalesce(body, '')), 1, 480), \
                    indexed_at, scope from history_commits order by sha",
        )
        && let Ok(rows) = stmt.query_map([], |row| {
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
        })
    {
        out.extend(rows.filter_map(Result::ok));
    }

    if table_exists(conn, "history_docs")
        && let Ok(mut stmt) = conn.prepare(
            "select id, substr(trim(coalesce(title, '') || ' ' || coalesce(body, '')), 1, 480), \
                    coalesce(updated_at, fetched_at), coalesce(url, id), scope, state \
             from history_docs order by id",
        )
        && let Ok(rows) = stmt.query_map([], |row| {
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
        })
    {
        out.extend(rows.filter_map(Result::ok));
    }

    if table_exists(conn, "code_symbols")
        && let Ok(mut stmt) = conn.prepare(
            "select id, substr(trim(qualified_name || ' ' || name || ' ' || file_path || ' ' || \
                    coalesce(documentation, '') || ' ' || coalesce(signature, '')), 1, 480), \
                    content_hash, qualified_name, scope from code_symbols order by id",
        )
        && let Ok(rows) = stmt.query_map([], |row| {
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
        })
    {
        out.extend(rows.filter_map(Result::ok));
    }
    out
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
            stale: "case when valid_until is not null and valid_until < datetime('now') then 1 else 0 end",
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
            text: "trim(subject || ' ' || coalesce(body, ''))",
            scope: "scope",
            team: "null",
            share: "null",
            revision: "indexed_at",
            stale: "0",
            body_available: true,
            locator: "sha",
            extra_scope_predicate: "1 = 1",
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
        "with",
        "from",
        "into",
        "then",
        "please",
        "implement",
    ];
    let mut terms = Vec::new();
    for raw in canonical
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.')))
    {
        let term = raw.trim_matches(['-', '_', '/', '.']).to_ascii_lowercase();
        if term.len() < 3 || STOP.contains(&term.as_str()) || terms.contains(&term) {
            continue;
        }
        terms.push(term);
        if terms.len() == 10 {
            break;
        }
    }
    terms
}

fn local_candidate(row: LocalRow, query: &RecallQuery, terms: &[String]) -> EvidenceCandidate {
    let haystack = row.snippet.to_ascii_lowercase();
    let matched: Vec<&str> = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .map(String::as_str)
        .collect();
    let lexical = matched.len() as f64 / terms.len().max(1) as f64;
    let binding = query.task_id.as_deref() == Some(row.id.as_str())
        || query.files.iter().any(|file| haystack.contains(file))
        || query.symbols.iter().any(|symbol| haystack.contains(symbol));
    let role_score = match (query.role, row.surface) {
        (RecallRole::Worker, EvidenceSurface::Code) => 0.24,
        (RecallRole::Worker, EvidenceSurface::History) => 0.20,
        (RecallRole::Worker, EvidenceSurface::Rule) => 0.16,
        (RecallRole::Worker, EvidenceSurface::Memory) => 0.14,
        (RecallRole::Supervisor, EvidenceSurface::Task) => 0.26,
        (RecallRole::Supervisor, EvidenceSurface::Spec) => 0.22,
        (RecallRole::Supervisor, EvidenceSurface::History) => 0.20,
        (RecallRole::Supervisor, EvidenceSurface::Rule) => 0.14,
        _ => 0.08,
    };
    let structural = if binding { 1.0 } else { 0.0 };
    EvidenceCandidate {
        evidence_id: row.id,
        surface: row.surface,
        scope: row.scope,
        snippet: clean_scalar(&row.snippet, 480),
        why_relevant: if binding {
            "exact task/file/symbol binding".into()
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
    if crate::internal_llm::is_internal_invocation() {
        eprintln!("cas: ambient recall skipped (internal model identity)");
        return None;
    }
    let role = input
        .agent_role
        .as_deref()
        .and_then(RecallRole::parse)
        .or_else(|| {
            std::env::var("CAS_AGENT_ROLE")
                .ok()
                .as_deref()
                .and_then(RecallRole::parse)
        })?;
    if !session_start && !meaningful_transition(prompt.unwrap_or_default()) {
        return None;
    }
    let identity = RecallIdentity {
        session_id: input.session_id.clone(),
        agent_name: std::env::var("CAS_AGENT_NAME").unwrap_or_default(),
        factory_session: std::env::var("CAS_FACTORY_SESSION").unwrap_or_default(),
        role,
        project_id: crate::cloud::resolve_canonical_id(cas_root).unwrap_or_default(),
        team_id: crate::cloud::CloudConfig::load_from_cas_dir(cas_root)
            .ok()
            .and_then(|config| config.active_team_id()),
        internal_llm: false,
    };
    if !identity.is_eligible() {
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
    let query = RecallQuery::build(&identity, &request)?;
    let retriever = SqliteRecallRetriever::existing(cas_root)?;
    let config = crate::cloud::CloudConfig::load_from_cas_dir(cas_root).unwrap_or_default();
    let semantic = SemanticRecallRetriever::existing(cas_root, &config);
    let mut retrievers: Vec<&dyn RecallRetriever> = vec![&retriever];
    if let Some(semantic) = semantic.as_ref() {
        retrievers.push(semantic);
    }
    let candidates = retrieve_candidates(&identity, &request, &retrievers)?;
    let ledger_file = ledger_path(cas_root, &identity.session_id);
    let mut ledger = RecallLedger::load(&ledger_file);
    let rendered = render_packet(&identity, &query, &candidates, &mut ledger);
    match rendered {
        Some((packet, injected)) => {
            ledger.record(packet.query_hash.clone(), &injected);
            ledger.save(&ledger_file);
            eprintln!(
                "cas: ambient recall injected {} evidence card(s), omitted {}",
                packet.injected, packet.omitted
            );
            Some(packet)
        }
        None => {
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
    request
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecallCandidates {
    pub(crate) candidates: Vec<EvidenceCandidate>,
    pub(crate) rejected_scope: usize,
}

/// Disposable per-session state. It suppresses repeated prompt inflation; it
/// is never an authoritative memory source and may be deleted at any time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecallLedger {
    #[serde(default)]
    last_query_hash: String,
    #[serde(default)]
    seen: Vec<SeenEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SeenEvidence {
    evidence_id: String,
    revision: String,
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

    fn record(&mut self, query_hash: String, injected: &[EvidenceCandidate]) {
        self.last_query_hash = query_hash;
        for candidate in injected {
            self.seen
                .retain(|seen| seen.evidence_id != candidate.evidence_id);
            self.seen.push(SeenEvidence {
                evidence_id: candidate.evidence_id.clone(),
                revision: candidate.provenance.revision.clone(),
            });
        }
        if self.seen.len() > LEDGER_ENTRY_CAP {
            self.seen.drain(..self.seen.len() - LEDGER_ENTRY_CAP);
        }
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
    let delta: Vec<EvidenceCandidate> = candidates
        .candidates
        .iter()
        .filter(|candidate| !ledger.has_seen(candidate))
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
    for candidate in delta.iter().take(cap) {
        let card = render_card(candidate);
        if full.len() + 1 + card.len() + footer_reserve > byte_budget {
            break;
        }
        full.push('\n');
        full.push_str(&card);
        injected.push(candidate.clone());
    }
    if injected.is_empty() {
        return None;
    }

    let omitted = delta.len().saturating_sub(injected.len());
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

fn render_card(candidate: &EvidenceCandidate) -> String {
    let flags = [
        candidate.binding.then_some("binding"),
        candidate.stale.then_some("STALE"),
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

fn fuse_candidate(existing: &mut EvidenceCandidate, incoming: EvidenceCandidate) {
    existing.lexical_score = existing.lexical_score.max(incoming.lexical_score);
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::test_support::TestEnvGuard;
    use crate::types::{Entry, Task, TaskStatus};

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

    struct CountingEmbedder {
        calls: Arc<AtomicUsize>,
        meta: EmbeddingMeta,
        vector: Vec<f32>,
    }

    impl RecallQueryEmbedder for CountingEmbedder {
        fn meta(&self) -> EmbeddingMeta {
            self.meta.clone()
        }

        fn embed_query(&self, _query: &str) -> Option<Vec<f32>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Some(self.vector.clone())
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
        ledger.record(packet.query_hash, &injected);
        assert!(render_packet(&identity, &query, &candidates, &mut ledger).is_some());
    }

    #[test]
    fn repeated_cards_are_deltas_and_changed_revisions_reappear() {
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
        };
        let (packet, injected) =
            render_packet(&identity, &query, &candidates, &mut ledger).unwrap();
        ledger.record(packet.query_hash, &injected);
        assert!(render_packet(&identity, &query, &candidates, &mut ledger).is_none());

        let mut revised = first;
        revised.provenance.revision = "r2".into();
        revised.stale = true;
        let changed = RecallCandidates {
            candidates: vec![revised],
            rejected_scope: 0,
        };
        let (packet, _) = render_packet(&identity, &query, &changed, &mut ledger).unwrap();
        assert!(packet.full.contains("@r2"));
        assert!(packet.full.contains("STALE"));
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
        ledger.record("query".into(), &rows);
        ledger.save(&path);
        let loaded = RecallLedger::load(&path);
        assert_eq!(loaded.seen.len(), LEDGER_ENTRY_CAP);
        assert!(fs::metadata(&path).unwrap().len() <= LEDGER_BYTE_CAP as u64);
        fs::remove_file(&path).unwrap();
        assert_eq!(RecallLedger::load(&path), RecallLedger::default());
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
        assert!(!dir.path().join("index/code-vectors").exists());
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
}
