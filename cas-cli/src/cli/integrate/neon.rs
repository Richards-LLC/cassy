//! `cas integrate neon <action>` — full implementation (task **cas-1ece**,
//! EPIC cas-b65f).
//!
//! ## Stub-handler convention (mirrors cas-8e37 vercel handler)
//!
//! Following the convention codified in cas-e6b6 / cas-8e37 for the integrate
//! family:
//!
//! - **`Ok(IntegrationOutcome { status: Skipped, .. })`** when Neon is
//!   genuinely not present in the repo (no detection signals). Init exits
//!   cleanly without error so `cas init` orchestration can move on to the
//!   next platform.
//! - **`Err(...)`** for unrecoverable conditions: malformed `package.json`,
//!   filesystem errors, MCP/Neon client failures, or required prompts the
//!   caller cannot answer.
//!
//! ## Keep-block convention
//!
//! All MCP-fetched IDs live inside a single named keep block,
//! `<!-- keep neon-ids -->` … `<!-- /keep neon-ids -->`. Named (not unnamed)
//! per cas-e6b6 design note: a future template revision that adds another
//! preserved block should not silently misroute user content into the wrong
//! slot.
//!
//! Refresh calls [`super::keep_block::orphaned_existing`] before
//! [`super::keep_block::merge`] and surfaces dropped blocks via
//! `IntegrationOutcome.summary` rather than letting them disappear.
//!
//! ## Multi-org wrinkle
//!
//! The user has multiple Neon orgs (memory `reference_neon_cas_cloud.md`),
//! so `mcp__neon__list_projects` *requires* `org_id`. Org selection is
//! interactive when more than one org is returned; otherwise auto-picked.
//!
//! ## Testability
//!
//! All MCP/Neon access goes through the [`NeonClient`] trait. Tests use
//! [`FakeNeonClient`] — `cargo test` never depends on a live Neon endpoint.
//! The production [`LiveNeonClient`] is wired to `cmcp_core::ProxyEngine`
//! (cas-1549) when the binary is built with `--features mcp-proxy`; off
//! that feature it bails with a clear rebuild hint. Parser-level fixture
//! tests live in `super::neon_parsers_test`; live MCP transport is
//! intentionally not exercised by `cargo test` (verified manually, same
//! scope cap as cas-7417's vercel wiring).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use super::keep_block::{self, MergeMode};
use super::types::{
    IdState, IntegrationAction, IntegrationOutcome, IntegrationStatus, Platform, VerifyItem,
    VerifyReport,
};

// --- Templates -------------------------------------------------------------

const TEMPLATE_CLAUDE: &str = include_str!("templates/neon/SKILL.md.template");
const TEMPLATE_CURSOR: &str = include_str!("templates/neon/cursor.md.template");
const TEMPLATE_QUERIES: &str = include_str!("templates/neon/references/queries.md");

// --- Output paths ----------------------------------------------------------

const CLAUDE_SKILL: &str = ".claude/skills/neon-database/SKILL.md";
const CLAUDE_QUERIES: &str = ".claude/skills/neon-database/references/queries.md";
const CURSOR_SKILL: &str = ".cursor/skills/neon-database/SKILL.md";

// --- Detection sentinels ---------------------------------------------------
//
// Hoisted to module-top per cas-fc38 / sibling-handler convention (vercel.rs,
// github.rs). Lets a future reader scan the file and immediately see what
// project signals this handler probes for, without spelunking through fn
// bodies.
const PACKAGE_JSON: &str = "package.json";
const NEONDATABASE_DEP_PREFIX: &str = "\"@neondatabase/";
const NEON_PRISMA_ADAPTER_DEP: &str = "\"@prisma/adapter-neon\"";
const PRISMA_NEON_URL_MARKER: &str = "neon.tech";

/// Well-known prisma schema locations probed during detection. Extending this
/// list to support a new monorepo layout is the supported customisation point.
const PRISMA_SCHEMA_PATHS: &[&str] = &[
    "prisma/schema.prisma",
    "apps/backend/prisma/schema.prisma",
    "apps/api/prisma/schema.prisma",
    "packages/db/prisma/schema.prisma",
];

// --- Keep-block name -------------------------------------------------------

/// Name of the canonical IDs keep block in the rendered SKILL.md. Centralised
/// so the template, refresh, verify, and orphan-detection paths can't drift
/// out of sync on the marker spelling.
const KEEP_IDS_BLOCK: &str = "neon-ids";

// --- Public types ----------------------------------------------------------

/// A Neon organization, as returned by `list_organizations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeonOrg {
    pub id: String,
    pub name: String,
}

/// A Neon project, as returned by `list_projects(org_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeonProject {
    pub id: String,
    pub name: String,
}

/// A Neon branch, as returned within `describe_project`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeonBranch {
    pub id: String,
    pub name: String,
    /// True for the project's default branch (typically `main`).
    pub is_default: bool,
}

/// Project detail with branches and a database name. Returned by
/// `describe_project`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeonProjectDetail {
    pub project: NeonProject,
    pub default_database: String,
    pub branches: Vec<NeonBranch>,
}

/// Provider trait for Neon read operations — mocked in tests.
pub trait NeonClient {
    fn list_organizations(&self) -> Result<Vec<NeonOrg>>;
    fn list_projects(&self, org_id: &str) -> Result<Vec<NeonProject>>;
    fn describe_project(&self, org_id: &str, project_id: &str) -> Result<NeonProjectDetail>;
    /// Returns Ok(true) if the branch still exists, Ok(false) if missing,
    /// Err for transport-level failures.
    fn describe_branch(&self, org_id: &str, project_id: &str, branch_id: &str) -> Result<bool>;
}

/// Production [`NeonClient`] backed by the shared
/// [`super::proxy::ProxyClient`] (cas-36fd0). The proxy lifecycle (lazy
/// init, engine reuse, Drop with shutdown, poison recovery) lives once
/// in `proxy.rs`; this struct is just the Neon-specific wrapper that
/// translates trait calls into `ProxyClient::call(tool, args)` plus the
/// neon-specific parsers below.
///
/// **Live MCP is not exercised by `cargo test`** — the parsers in
/// [`parse_list_organizations`] / [`parse_list_projects`] /
/// [`parse_describe_project`] / [`parse_describe_branch`] are
/// fixture-tested against canonical Neon MCP response shapes. End-to-end
/// transport is verified by the shared fixture in
/// `cas-cli/tests/integrate_lifecycle_test.rs` (cas-2dc9).
#[cfg(feature = "mcp-proxy")]
pub struct LiveNeonClient {
    inner: super::proxy::ProxyClient,
}

#[cfg(feature = "mcp-proxy")]
impl Default for LiveNeonClient {
    fn default() -> Self {
        Self {
            inner: super::proxy::ProxyClient::new("neon"),
        }
    }
}

#[cfg(feature = "mcp-proxy")]
impl LiveNeonClient {
    pub fn new() -> Self {
        Self::default()
    }

    fn arg_str(k: &str, v: &str) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        m
    }
}

#[cfg(not(feature = "mcp-proxy"))]
pub struct LiveNeonClient;

#[cfg(not(feature = "mcp-proxy"))]
impl Default for LiveNeonClient {
    fn default() -> Self {
        Self
    }
}

#[cfg(not(feature = "mcp-proxy"))]
impl LiveNeonClient {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(feature = "mcp-proxy"))]
const NO_MCP_PROXY: &str =
    "this build of cas does not include the mcp-proxy feature; rebuild with \
     `cargo install cas --features mcp-proxy` to enable `cas integrate neon`";

#[cfg(not(feature = "mcp-proxy"))]
impl NeonClient for LiveNeonClient {
    fn list_organizations(&self) -> Result<Vec<NeonOrg>> {
        Err(anyhow!(NO_MCP_PROXY))
    }
    fn list_projects(&self, _org_id: &str) -> Result<Vec<NeonProject>> {
        Err(anyhow!(NO_MCP_PROXY))
    }
    fn describe_project(&self, _org_id: &str, _project_id: &str) -> Result<NeonProjectDetail> {
        Err(anyhow!(NO_MCP_PROXY))
    }
    fn describe_branch(&self, _o: &str, _p: &str, _b: &str) -> Result<bool> {
        Err(anyhow!(NO_MCP_PROXY))
    }
}

#[cfg(feature = "mcp-proxy")]
impl NeonClient for LiveNeonClient {
    fn list_organizations(&self) -> Result<Vec<NeonOrg>> {
        let value = self.inner.call("list_organizations", None)?;
        parse_list_organizations(&value)
    }

    fn list_projects(&self, org_id: &str) -> Result<Vec<NeonProject>> {
        let args = Self::arg_str("org_id", org_id);
        let value = self.inner.call("list_projects", Some(args))?;
        parse_list_projects(&value)
    }

    fn describe_project(&self, org_id: &str, project_id: &str) -> Result<NeonProjectDetail> {
        let mut args = serde_json::Map::new();
        args.insert("org_id".to_string(), serde_json::Value::String(org_id.to_string()));
        args.insert(
            "project_id".to_string(),
            serde_json::Value::String(project_id.to_string()),
        );
        let value = self.inner.call("describe_project", Some(args))?;
        parse_describe_project(&value)
    }

    fn describe_branch(&self, org_id: &str, project_id: &str, branch_id: &str) -> Result<bool> {
        let mut args = serde_json::Map::new();
        args.insert("org_id".to_string(), serde_json::Value::String(org_id.to_string()));
        args.insert(
            "project_id".to_string(),
            serde_json::Value::String(project_id.to_string()),
        );
        args.insert(
            "branch_id".to_string(),
            serde_json::Value::String(branch_id.to_string()),
        );
        let value = self.inner.call("describe_branch", Some(args))?;
        parse_describe_branch(&value)
    }
}

// ---------------------------------------------------------------------------
// MCP response parsers (fixture-testable, transport-independent).
//
// All four `parse_*` functions follow the same shape as
// vercel::parse_list_projects: peel off the MCP envelope (`{ content: [{
// type: "text", text: "<json>" }] }`), then accept multiple inner shapes
// because the upstream Neon MCP server has rolled through several response
// layouts during its lifetime. The parsers must never silently swallow a
// transport-level failure — `isError: true` and JSON parse errors propagate
// as Err so verify can distinguish "transport error" from "drift".
// ---------------------------------------------------------------------------

/// Parse `mcp__neon__list_organizations` into a vec of [`NeonOrg`].
///
/// Tolerated shapes:
/// - `{ "organizations": [...] }` (canonical)
/// - bare `[...]` array
///
/// (cas-36fd0 dropped the `orgs` alias — it was speculative tolerance
/// added in cas-1549 with no fixture from a real Neon envelope. Add it
/// back if a real upstream rolls out the alias.)
pub fn parse_list_organizations(value: &serde_json::Value) -> Result<Vec<NeonOrg>> {
    let inner = unwrap_mcp_envelope(value)?;
    let array = match &inner {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(m) => m
            .get("organizations")
            .and_then(|v| v.as_array())
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "list_organizations: response had no `organizations` array. \
                     cas-36fd0 dropped speculative tolerance for `orgs` and \
                     unrecognized wrapper keys; if Neon's MCP rolled out a new \
                     shape, please file a bug with this payload: {inner}"
                )
            })?,
        _ => anyhow::bail!(
            "list_organizations: unexpected shape (expected array or `{{ organizations: [...] }}`): {inner}"
        ),
    };
    let mut out = Vec::with_capacity(array.len());
    for v in array {
        if let Some(o) = neon_org_from_value(&v) {
            out.push(o);
        }
    }
    Ok(out)
}

/// Parse `mcp__neon__list_projects` into a vec of [`NeonProject`].
///
/// Tolerated shapes:
/// - `{ "projects": [...] }` (canonical)
/// - bare `[...]` array
///
/// (cas-36fd0 dropped the `data` alias — speculative tolerance added in
/// cas-1549 without a fixture from a real Neon envelope. Re-add if a
/// real upstream rolls out the alias.)
pub fn parse_list_projects(value: &serde_json::Value) -> Result<Vec<NeonProject>> {
    let inner = unwrap_mcp_envelope(value)?;
    let array = match &inner {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(m) => m
            .get("projects")
            .and_then(|v| v.as_array())
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "list_projects: response had no `projects` array. cas-36fd0 \
                     dropped speculative tolerance for `data` and unrecognized \
                     wrapper keys; if Neon's MCP rolled out a new shape, please \
                     file a bug with this payload: {inner}"
                )
            })?,
        _ => anyhow::bail!(
            "list_projects: unexpected shape (expected array or `{{ projects: [...] }}`): {inner}"
        ),
    };
    let mut out = Vec::with_capacity(array.len());
    for v in array {
        if let Some(p) = neon_project_from_value(&v) {
            out.push(p);
        }
    }
    Ok(out)
}

/// Parse `mcp__neon__describe_project` into a [`NeonProjectDetail`].
///
/// Canonical shape:
/// ```ignore
/// { "project": { "id": "...", "name": "...", "default_database_name": "..." },
///   "branches": [ { "id": "...", "name": "...", "default": true|false } ] }
/// ```
///
/// (cas-36fd0 dropped the speculative flat-object tolerance + the
/// silent `"neondb"` fallback. The flat shape was added defensively
/// without a real fixture; the silent fallback masked upstream-shape
/// drift as "user has the default database". Both can be reintroduced
/// if a real upstream warrants it.)
pub fn parse_describe_project(value: &serde_json::Value) -> Result<NeonProjectDetail> {
    let inner = unwrap_mcp_envelope(value)?;
    let obj = inner
        .as_object()
        .ok_or_else(|| anyhow!("describe_project: expected object, got {inner}"))?;

    let project_value = obj.get("project").ok_or_else(|| {
        anyhow!(
            "describe_project: response had no `project` object: {inner}"
        )
    })?;
    let project = neon_project_from_value(project_value).ok_or_else(|| {
        anyhow!("describe_project: missing or malformed project: {project_value}")
    })?;

    // default_database resolution: try `project.default_database_name`,
    // then top-level `default_database`, then the first entry in
    // `databases[]`. If none of those land, **bail** rather than silently
    // defaulting to "neondb" — the user must see that the recorded
    // database name is not authoritative so they can correct the upstream
    // (or open a follow-up).
    let default_database = project_value
        .get("default_database_name")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("default_database").and_then(|v| v.as_str()))
        .or_else(|| {
            obj.get("databases")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|d| d.get("name"))
                .and_then(|v| v.as_str())
        })
        .ok_or_else(|| {
            anyhow!(
                "describe_project: no default database name found. Looked at \
                 `project.default_database_name`, top-level `default_database`, \
                 and `databases[0].name`. \
                 \n\
                 If this is a freshly-created project, the default database may \
                 not have provisioned yet — wait a few seconds and re-run \
                 `cas integrate neon refresh --update-ids`. Otherwise pass \
                 `InitChoices.database_name = Some(...)` (or open a bug). \
                 \n\
                 Response: {inner}"
            )
        })?
        .to_string();

    let branches = obj
        .get("branches")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(neon_branch_from_value).collect())
        .unwrap_or_default();

    Ok(NeonProjectDetail {
        project,
        default_database,
        branches,
    })
}

/// Parse `mcp__neon__describe_branch` into a presence bool.
///
/// Authoritative absence vs transport blip is load-bearing here: verify
/// routes the two to `IntegrationStatus::Stale` (drift) and
/// `IntegrationStatus::TransportError` respectively, and `cas init` reacts
/// differently to each. The function therefore admits **only** explicit
/// not-found signals as `Ok(false)` and bubbles everything else as `Err`:
///
/// - `Ok(true)` — response contains a recognizable branch object
///   (canonical `{ "branch": { id, name, ... } }` or a flat
///   `{ id, name, ... }`).
/// - `Ok(false)` — only when the inner JSON is **explicitly** an
///   `{ "branch": null }` object **or** an `{ "error": "branch ... not found ..." }`
///   string whose substring identifies the *branch* as the missing
///   resource. A bare `null` inner, an `error` string mentioning a
///   different resource ("project not found", "org not found", "rate
///   limited"), or any other ambiguous shape returns `Err`.
/// - `Err` — `isError: true` envelopes, JSON parse errors, empty-content
///   envelopes (which `unwrap_mcp_envelope` reports as `Null` and we
///   refuse to interpret), and any unrecognized shape. cas-fc38 contract:
///   never silently downgraded to `Ok(false)`.
pub fn parse_describe_branch(value: &serde_json::Value) -> Result<bool> {
    let inner = unwrap_mcp_envelope(value)?;
    // A bare `Null` inner is ambiguous (could be empty content from a
    // transport hiccup, could be the upstream returning null for a
    // genuinely-missing branch). cas-fc38 contract is to bubble — verify
    // routes this to TransportError, the safer default.
    if matches!(&inner, serde_json::Value::Null) {
        anyhow::bail!(
            "describe_branch returned an empty/null payload — refusing to \
             interpret as authoritative not-found; treating as transport error"
        );
    }
    let obj = inner.as_object().ok_or_else(|| {
        anyhow!("describe_branch: expected object, got {inner}")
    })?;

    // Explicit `{ "branch": null }` is an authoritative not-found.
    if matches!(obj.get("branch"), Some(serde_json::Value::Null)) {
        return Ok(false);
    }

    if let Some(err) = obj.get("error").and_then(|v| v.as_str()) {
        let lc = err.to_ascii_lowercase();
        // Tighten the not-found sniff: the error must mention "branch" so
        // an upstream "project not found" / "org not found" / "rate
        // limited" doesn't get misclassified as branch-missing.
        if lc.contains("not found") && lc.contains("branch") {
            return Ok(false);
        }
        anyhow::bail!("describe_branch returned non-branch-not-found error: {err}");
    }

    // Canonical shape wraps the branch under `branch`; accept flat too.
    // Both id and name are required so a malformed `{branch:{id:"x"}}` is
    // rejected (per testing finding #2 from cas-code-review).
    let branch_obj = obj
        .get("branch")
        .and_then(|v| v.as_object())
        .or_else(|| {
            if obj.contains_key("id") && obj.contains_key("name") {
                Some(obj)
            } else {
                None
            }
        });
    match branch_obj {
        Some(b)
            if b.get("id").and_then(|v| v.as_str()).is_some()
                && b.get("name").and_then(|v| v.as_str()).is_some() =>
        {
            Ok(true)
        }
        _ => anyhow::bail!(
            "describe_branch: response had neither `branch` nor a flat branch \
             object with id+name: {inner}"
        ),
    }
}

fn neon_org_from_value(v: &serde_json::Value) -> Option<NeonOrg> {
    let id = v.get("id")?.as_str()?.to_string();
    let name = v.get("name")?.as_str()?.to_string();
    Some(NeonOrg { id, name })
}

fn neon_project_from_value(v: &serde_json::Value) -> Option<NeonProject> {
    let id = v.get("id")?.as_str()?.to_string();
    let name = v.get("name")?.as_str()?.to_string();
    Some(NeonProject { id, name })
}

fn neon_branch_from_value(v: &serde_json::Value) -> Option<NeonBranch> {
    let id = v.get("id")?.as_str()?.to_string();
    let name = v.get("name")?.as_str()?.to_string();
    // Neon's API uses both `default` and (older) `is_default`. Accept either.
    let is_default = v
        .get("default")
        .or_else(|| v.get("is_default"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    Some(NeonBranch { id, name, is_default })
}

/// MCP envelope unwrap. Delegates to the shared
/// [`super::proxy::unwrap_envelope`] when the proxy feature is on; non-
/// feature builds keep a tiny inline fallback so parser tests still
/// build without `--features mcp-proxy`.
#[cfg(feature = "mcp-proxy")]
fn unwrap_mcp_envelope(value: &serde_json::Value) -> Result<serde_json::Value> {
    super::proxy::unwrap_envelope(value)
}

#[cfg(not(feature = "mcp-proxy"))]
fn unwrap_mcp_envelope(value: &serde_json::Value) -> Result<serde_json::Value> {
    let serde_json::Value::Object(map) = value else {
        return Ok(value.clone());
    };
    if map.get("isError").and_then(|v| v.as_bool()) == Some(true) {
        anyhow::bail!("MCP returned isError=true: {value}");
    }
    let Some(serde_json::Value::Array(content)) = map.get("content") else {
        return Ok(value.clone());
    };
    let mut buf = String::new();
    for item in content {
        if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
            buf.push_str(t);
        }
    }
    if buf.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&buf).with_context(|| format!("parsing MCP text content: {buf}"))
}

// --- Detection -------------------------------------------------------------

/// Detection signals for Neon usage in a repo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NeonDetection {
    /// `prisma/schema.prisma` mentions a `neon.tech` URL.
    pub prisma_neon_url: bool,
    /// `package.json` declares a `@neondatabase/*` or `@prisma/adapter-neon` dep.
    pub package_neon_dep: bool,
}

impl NeonDetection {
    pub fn detected(&self) -> bool {
        self.prisma_neon_url || self.package_neon_dep
    }
}

/// Scan `repo_root` for Neon usage signals.
///
/// Pure I/O over the filesystem; never panics on missing files.
pub fn detect(repo_root: &Path) -> NeonDetection {
    let mut out = NeonDetection::default();

    // Probe well-known prisma locations from the module-top sentinel list.
    for rel in PRISMA_SCHEMA_PATHS {
        if out.prisma_neon_url {
            break;
        }
        // Use read_capped (cas-fc38) so an oversized or symlinked
        // schema.prisma can't redirect us into ~/.ssh or balloon memory.
        if let Ok(prisma) = super::fs::read_capped(&repo_root.join(rel)) {
            if prisma.contains(PRISMA_NEON_URL_MARKER) {
                out.prisma_neon_url = true;
            }
        }
    }

    if let Ok(pkg) = super::fs::read_capped(&repo_root.join(PACKAGE_JSON)) {
        // Cheap structural check: any `"@neondatabase/...": ` or
        // `"@prisma/adapter-neon": ` is enough — we don't care about which
        // dep section.
        if pkg.contains(NEONDATABASE_DEP_PREFIX) || pkg.contains(NEON_PRISMA_ADAPTER_DEP) {
            out.package_neon_dep = true;
        }
    }

    out
}

// --- Branch heuristic ------------------------------------------------------

/// Map a list of branches to logical environments using simple heuristics:
/// - the branch flagged `is_default` → `production`
/// - branch named exactly `staging` → `staging`
/// - branch name starting with `dev` (e.g. `dev`, `dev-foo`) → `dev`
/// - other branches keep their raw name as the env label
///
/// Order is preserved from the input. Stable, deterministic output suitable
/// for templating into the keep block.
pub fn map_branches(branches: &[NeonBranch]) -> Vec<(String, NeonBranch)> {
    let mut out: Vec<(String, NeonBranch)> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for b in branches {
        let base = if b.is_default {
            "production".to_string()
        } else if b.name == "staging" {
            "staging".to_string()
        } else if b.name == "dev" || b.name.starts_with("dev-") || b.name.starts_with("dev_") {
            "dev".to_string()
        } else {
            b.name.clone()
        };
        // Disambiguate when two branches collide on the same heuristic label
        // (e.g. `dev-api` and `dev-web` both → `dev`). The first occurrence
        // keeps the bare label; subsequent occurrences get a numeric suffix
        // so each row in the keep block carries a unique key.
        let count = seen.entry(base.clone()).or_insert(0);
        let label = if *count == 0 {
            base.clone()
        } else {
            format!("{base}-{}", *count + 1)
        };
        *count += 1;
        out.push((label, b.clone()));
    }
    out
}

// --- Init / Refresh / Verify options --------------------------------------

/// Inputs that init has resolved interactively (org pick, project confirm,
/// branch label overrides). Tests bypass the prompts and pass this directly.
#[derive(Debug, Clone, Default)]
pub struct InitChoices {
    /// If `Some`, use this org without prompting (single-org auto-pick or test injection).
    pub org_id: Option<String>,
    /// If `Some`, force this project id without prompting.
    pub project_id: Option<String>,
    /// Optional override for the default-database name (defaults to value
    /// returned by the client).
    pub database_name: Option<String>,
    /// Optional override of the (env_label -> branch_id) mapping; replaces
    /// the heuristic output entirely if non-empty.
    pub branch_label_overrides: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct RefreshOpts {
    /// Re-fetch IDs from Neon and overwrite the keep block. When false
    /// (default), the keep block is preserved verbatim and only the
    /// surrounding prose is regenerated.
    pub update_ids: bool,
}

// --- Public CLI dispatch ---------------------------------------------------

/// CLI entrypoint. Constructs a [`LiveNeonClient`] and dispatches.
///
/// Repo-root resolution goes through [`super::fs::locate_repo_root`] so the
/// handler operates on the inner repo when invoked from a submodule or
/// nested-worktree directory (cas-fc38).
pub fn execute(action: IntegrationAction) -> Result<IntegrationOutcome> {
    let repo_root = super::fs::locate_repo_root().context("locating repo root")?;
    let client = LiveNeonClient::new();
    match action {
        IntegrationAction::Init => init(&repo_root, &client, InitChoices::default()),
        IntegrationAction::Refresh => refresh(&repo_root, &client, RefreshOpts::default()),
        IntegrationAction::Verify => verify(&repo_root, &client),
    }
}

// --- Init ------------------------------------------------------------------

/// Init flow — pure-fn / dependency-injected for testability.
pub fn init<C: NeonClient>(
    repo_root: &Path,
    client: &C,
    choices: InitChoices,
) -> Result<IntegrationOutcome> {
    let detection = detect(repo_root);
    if !detection.detected() && choices.org_id.is_none() {
        let mut outcome = IntegrationOutcome::new(
            Platform::Neon,
            IntegrationAction::Init,
            IntegrationStatus::Skipped,
        );
        outcome
            .summary
            .push("no Neon detection signals (prisma/schema.prisma neon.tech URL or @neondatabase deps)".into());
        return Ok(outcome);
    }

    // 1. Pick org
    let org_id = match choices.org_id.clone() {
        Some(id) => id,
        None => {
            let orgs = client
                .list_organizations()
                .context("listing Neon organizations")?;
            if orgs.is_empty() {
                return Err(anyhow!("Neon returned no organizations for this user"));
            }
            if orgs.len() == 1 {
                orgs[0].id.clone()
            } else {
                // Multi-org case: prompt for selection.
                pick_org_interactively(&orgs)?
            }
        }
    };

    // 2. Pick project (fuzzy-match by repo basename, prompt on ambiguity).
    let project_id = match choices.project_id.clone() {
        Some(id) => id,
        None => {
            let projects = client
                .list_projects(&org_id)
                .with_context(|| format!("listing Neon projects for org {org_id}"))?;
            if projects.is_empty() {
                return Err(anyhow!("Neon returned no projects for org {org_id}"));
            }
            let basename = repo_basename(repo_root);
            pick_project(&projects, &basename)?
        }
    };

    // 3. Describe project, capture branches.
    let detail = client
        .describe_project(&org_id, &project_id)
        .with_context(|| format!("describing project {project_id} in org {org_id}"))?;

    let database_name = choices
        .database_name
        .clone()
        .unwrap_or_else(|| detail.default_database.clone());

    let mut branch_map: Vec<(String, NeonBranch)> = if !choices.branch_label_overrides.is_empty() {
        // Caller injected explicit labels — apply them by branch id.
        let by_id: std::collections::HashMap<&str, &NeonBranch> = detail
            .branches
            .iter()
            .map(|b| (b.id.as_str(), b))
            .collect();
        choices
            .branch_label_overrides
            .iter()
            .filter_map(|(label, id)| by_id.get(id.as_str()).map(|b| (label.clone(), (*b).clone())))
            .collect()
    } else {
        map_branches(&detail.branches)
    };
    // Stable order: production first, staging, dev, then others alphabetically.
    branch_map.sort_by_key(|(label, _)| label_sort_key(label));

    let context = TemplateContext {
        project_basename: repo_basename(repo_root),
        org_id: org_id.clone(),
        project_id: project_id.clone(),
        database_name: database_name.clone(),
        branch_map: branch_map.clone(),
    };

    let claude = render_template(TEMPLATE_CLAUDE, &context);
    let cursor = render_template(TEMPLATE_CURSOR, &context);

    let mut outcome = IntegrationOutcome::new(
        Platform::Neon,
        IntegrationAction::Init,
        IntegrationStatus::Configured,
    );
    write_with_dirs(repo_root, CLAUDE_SKILL, &claude)?;
    outcome.files.push(PathBuf::from(CLAUDE_SKILL));
    write_with_dirs(repo_root, CURSOR_SKILL, &cursor)?;
    outcome.files.push(PathBuf::from(CURSOR_SKILL));
    write_with_dirs(repo_root, CLAUDE_QUERIES, TEMPLATE_QUERIES)?;
    outcome.files.push(PathBuf::from(CLAUDE_QUERIES));

    outcome.summary.push(format!(
        "neon org {} / project {} / db {} ({} branch(es))",
        org_id,
        project_id,
        database_name,
        branch_map.len()
    ));
    Ok(outcome)
}

// --- Refresh --------------------------------------------------------------

/// Refresh flow.
///
/// - Default (`update_ids = false`): regenerate prose around an unchanged keep
///   block. The previous keep-block content (org_id, projectId, branches) is
///   preserved verbatim. Used when the user has hand-edited the IDs or the
///   branch labels and only wants the surrounding template prose refreshed.
/// - `update_ids = true`: re-fetch via the client and overwrite the keep block.
pub fn refresh<C: NeonClient>(
    repo_root: &Path,
    client: &C,
    opts: RefreshOpts,
) -> Result<IntegrationOutcome> {
    let claude_path = repo_root.join(CLAUDE_SKILL);
    let cursor_path = repo_root.join(CURSOR_SKILL);
    // read_capped (cas-fc38): symlink-rejecting + size-capped.
    let existing_claude = super::fs::read_capped(&claude_path).ok();
    let existing_cursor = super::fs::read_capped(&cursor_path).ok();

    if existing_claude.is_none() && existing_cursor.is_none() {
        let mut outcome = IntegrationOutcome::new(
            Platform::Neon,
            IntegrationAction::Refresh,
            IntegrationStatus::Skipped,
        );
        outcome.summary.push(
            "no existing neon SKILL.md found — run `cas integrate neon init` first".into(),
        );
        return Ok(outcome);
    }

    let mut outcome = IntegrationOutcome::new(
        Platform::Neon,
        IntegrationAction::Refresh,
        IntegrationStatus::Refreshed,
    );

    if opts.update_ids {
        // Re-fetch via the client. We need the org_id and project_id from the
        // existing keep block to know where to look.
        let existing = existing_claude
            .as_deref()
            .or(existing_cursor.as_deref())
            .ok_or_else(|| anyhow!("no existing skill file to derive ids from"))?;
        let prev = parse_keep_payload(existing)?;
        let detail = client
            .describe_project(&prev.org_id, &prev.project_id)
            .with_context(|| {
                format!(
                    "re-describing project {} in org {}",
                    prev.project_id, prev.org_id
                )
            })?;
        let mut branch_map = map_branches(&detail.branches);
        branch_map.sort_by_key(|(label, _)| label_sort_key(label));
        let context = TemplateContext {
            project_basename: repo_basename(repo_root),
            org_id: prev.org_id.clone(),
            project_id: prev.project_id.clone(),
            database_name: detail.default_database.clone(),
            branch_map,
        };
        let claude = render_template(TEMPLATE_CLAUDE, &context);
        let cursor = render_template(TEMPLATE_CURSOR, &context);

        // Surface orphan blocks even on the overwrite path: when the user
        // hand-added a keep block beyond `neon-ids`, PreferTemplate would
        // silently drop it. Foundation cas-e6b6 review note: call
        // orphaned_existing before merge regardless of mode.
        if let Some(existing) = existing_claude.as_deref() {
            for orphan in keep_block::orphaned_existing(&claude, existing)? {
                outcome.summary.push(format!(
                    "warning: --update-ids dropping orphan keep block in {}: name={:?}",
                    CLAUDE_SKILL, orphan.name
                ));
            }
        }
        if let Some(existing) = existing_cursor.as_deref() {
            for orphan in keep_block::orphaned_existing(&cursor, existing)? {
                outcome.summary.push(format!(
                    "warning: --update-ids dropping orphan keep block in {}: name={:?}",
                    CURSOR_SKILL, orphan.name
                ));
            }
        }

        // PreferTemplate: caller has fresh keep-block content.
        write_merged(
            repo_root,
            CLAUDE_SKILL,
            &claude,
            existing_claude.as_deref(),
            MergeMode::PreferTemplate,
            &mut outcome,
        )?;
        write_merged(
            repo_root,
            CURSOR_SKILL,
            &cursor,
            existing_cursor.as_deref(),
            MergeMode::PreferTemplate,
            &mut outcome,
        )?;
    } else {
        // Preserve keep block; regenerate prose from a deflated template.
        // We feed the template as-is (placeholder values) and let merge
        // splice the user's keep block into it. If the existing file is
        // missing the `neon-ids` keep block (e.g. user renamed/deleted it),
        // splicing would leave the placeholder strings in the rendered
        // prose — refuse instead of writing junk.
        let context = blank_template_context(repo_basename(repo_root));
        let claude = render_template(TEMPLATE_CLAUDE, &context);
        let cursor = render_template(TEMPLATE_CURSOR, &context);

        for (label, existing_opt) in [
            (CLAUDE_SKILL, existing_claude.as_deref()),
            (CURSOR_SKILL, existing_cursor.as_deref()),
        ] {
            if let Some(existing) = existing_opt {
                let blocks = keep_block::extract(existing).with_context(|| {
                    format!("parsing keep blocks in existing {label}")
                })?;
                let has_neon_ids = blocks
                    .iter()
                    .any(|b| b.name.as_deref() == Some(KEEP_IDS_BLOCK));
                if !has_neon_ids {
                    return Err(anyhow!(
                        "{label} is missing the `<!-- keep neon-ids -->` block — refusing to write template placeholder values. Run `cas integrate neon init` to regenerate, or restore the keep block manually."
                    ));
                }
            }
        }

        if let Some(existing) = existing_claude.as_deref() {
            for orphan in keep_block::orphaned_existing(&claude, existing)? {
                outcome.summary.push(format!(
                    "warning: dropping orphan keep block in {}: name={:?}",
                    CLAUDE_SKILL, orphan.name
                ));
            }
        }
        if let Some(existing) = existing_cursor.as_deref() {
            for orphan in keep_block::orphaned_existing(&cursor, existing)? {
                outcome.summary.push(format!(
                    "warning: dropping orphan keep block in {}: name={:?}",
                    CURSOR_SKILL, orphan.name
                ));
            }
        }

        write_merged(
            repo_root,
            CLAUDE_SKILL,
            &claude,
            existing_claude.as_deref(),
            MergeMode::PreserveExisting,
            &mut outcome,
        )?;
        write_merged(
            repo_root,
            CURSOR_SKILL,
            &cursor,
            existing_cursor.as_deref(),
            MergeMode::PreserveExisting,
            &mut outcome,
        )?;
    }

    // Always re-stamp queries.md (no keep blocks to preserve there).
    write_with_dirs(repo_root, CLAUDE_QUERIES, TEMPLATE_QUERIES)?;
    outcome.files.push(PathBuf::from(CLAUDE_QUERIES));

    Ok(outcome)
}

// --- Verify ---------------------------------------------------------------

pub fn verify<C: NeonClient>(repo_root: &Path, client: &C) -> Result<IntegrationOutcome> {
    let claude_path = repo_root.join(CLAUDE_SKILL);
    let existing = super::fs::read_capped(&claude_path).with_context(|| {
        format!(
            "{} missing — run `cas integrate neon init` first",
            CLAUDE_SKILL
        )
    })?;
    let payload = parse_keep_payload(&existing)?;

    // Per cas-fc38: distinguish transport errors (we couldn't reach Neon)
    // from genuine drift (Neon answered, the answer disagrees). Both used
    // to collapse into IntegrationStatus::Stale, which made it impossible
    // for `cas init` / `cas doctor` to tell flaky network from real drift.
    let mut any_stale = false;
    let mut any_transport_error = false;
    let mut summary = Vec::new();
    summary.push(format!(
        "checking {} branch(es) in project {}",
        payload.branches.len(),
        payload.project_id
    ));
    for (label, branch_id) in &payload.branches {
        match client.describe_branch(&payload.org_id, &payload.project_id, branch_id) {
            Ok(true) => summary.push(format!("  {label} ({branch_id}): ok")),
            Ok(false) => {
                any_stale = true;
                summary.push(format!("  {label} ({branch_id}): STALE"));
            }
            Err(e) => {
                any_transport_error = true;
                summary.push(format!("  {label} ({branch_id}): transport error: {e:#}"));
            }
        }
    }

    let status = if any_stale {
        // Drift wins over transport error: if even one branch confirmably
        // doesn't exist, we know the recorded IDs are stale regardless of
        // whether other branches were unreachable.
        IntegrationStatus::Stale
    } else if any_transport_error {
        IntegrationStatus::TransportError
    } else {
        IntegrationStatus::AlreadyConfigured
    };
    let mut outcome = IntegrationOutcome::new(Platform::Neon, IntegrationAction::Verify, status);
    outcome.summary = summary;
    Ok(outcome)
}

/// Structured verify, for `cas doctor` consumption.
///
/// Returns a [`VerifyReport`] with one [`VerifyItem`] per recorded branch.
/// Transport errors from `describe_branch` become [`IdState::McpUnreachable`]
/// (so doctor renders a "skip" rather than hard-failing). A missing SKILL
/// file or absent keep block returns `not_configured = true`.
pub fn verify_report<C: NeonClient>(repo_root: &Path, client: &C) -> VerifyReport {
    let claude_path = repo_root.join(CLAUDE_SKILL);
    let existing = match fs::read_to_string(&claude_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return VerifyReport::not_configured(
                Platform::Neon,
                format!("{CLAUDE_SKILL} not present"),
            );
        }
        Err(e) => {
            return VerifyReport::not_configured(
                Platform::Neon,
                format!("could not read {CLAUDE_SKILL}: {e}"),
            );
        }
    };
    let payload = match parse_keep_payload(&existing) {
        Ok(p) => p,
        Err(e) => {
            return VerifyReport::not_configured(
                Platform::Neon,
                format!("could not parse keep block in {CLAUDE_SKILL}: {e:#}"),
            );
        }
    };
    if payload.branches.is_empty() {
        return VerifyReport::not_configured(
            Platform::Neon,
            "no branches recorded in keep block",
        );
    }
    let mut report = VerifyReport::ok(Platform::Neon);
    for (label, branch_id) in &payload.branches {
        let state = match client.describe_branch(&payload.org_id, &payload.project_id, branch_id) {
            Ok(true) => IdState::Ok,
            Ok(false) => IdState::Stale,
            Err(e) => IdState::McpUnreachable(format!("{e:#}")),
        };
        report.items.push(VerifyItem {
            label: format!("{label} branchId"),
            id: branch_id.clone(),
            state,
        });
    }
    report
}

// --- Helpers ---------------------------------------------------------------

fn label_sort_key(label: &str) -> (u8, String) {
    match label {
        "production" => (0, label.to_string()),
        "staging" => (1, label.to_string()),
        "dev" => (2, label.to_string()),
        _ => (3, label.to_string()),
    }
}

fn repo_basename(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string()
}

/// Write a file under `repo_root/rel` via the shared atomic-write helper
/// (cas-fc38). Atomic at the rename, refuses to write through a symlink at
/// the target — same semantics as vercel.rs and github.rs.
fn write_with_dirs(repo_root: &Path, rel: &str, content: &str) -> Result<()> {
    let path = repo_root.join(rel);
    super::fs::atomic_write_create_dirs(&path, content)
}

fn write_merged(
    repo_root: &Path,
    rel: &str,
    template: &str,
    existing: Option<&str>,
    mode: MergeMode,
    outcome: &mut IntegrationOutcome,
) -> Result<()> {
    let merged = keep_block::merge(template, existing, mode)
        .with_context(|| format!("merging keep blocks for {rel}"))?;
    write_with_dirs(repo_root, rel, &merged)?;
    outcome.files.push(PathBuf::from(rel));
    Ok(())
}

#[derive(Debug, Clone)]
struct TemplateContext {
    project_basename: String,
    org_id: String,
    project_id: String,
    database_name: String,
    /// (env_label, branch) in display order.
    branch_map: Vec<(String, NeonBranch)>,
}

fn blank_template_context(basename: String) -> TemplateContext {
    TemplateContext {
        project_basename: basename,
        org_id: "<org_id>".into(),
        project_id: "<project_id>".into(),
        database_name: "<databaseName>".into(),
        branch_map: Vec::new(),
    }
}

fn render_template(template: &str, ctx: &TemplateContext) -> String {
    use super::md::{escape_md_cell, escape_md_cell_code};

    // cas-fc38: every platform-supplied string spliced into a markdown
    // cell goes through escape_md_cell / escape_md_cell_code so a value
    // containing `|`, `<!--`, `-->`, CR/LF, or backticks cannot corrupt
    // the surrounding table or keep markers. Branch names are
    // user-controlled at the Neon dashboard (and so are project
    // basenames in monorepos), so this is not theoretical.
    let branch_rows = if ctx.branch_map.is_empty() {
        "| **branches** | _none recorded_ |".to_string()
    } else {
        ctx.branch_map
            .iter()
            .map(|(label, branch)| {
                format!(
                    "| **{} branchId** | `{}` (name: `{}`) |",
                    escape_md_cell(label),
                    escape_md_cell_code(&branch.id),
                    escape_md_cell_code(&branch.name),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    // cas-fc38: the cas:full_name tag goes through emit_cas_full_name_tag
    // so an org/project id containing literal `-->` (or CR/LF) cannot
    // corrupt the surrounding `<!-- keep neon-ids -->` markers. Other
    // substitutions are scoped to markdown table cells and routed through
    // escape_md_cell_code (which strips backticks too).
    let cas_tag = super::md::emit_cas_full_name_tag(&format!(
        "{}/{}",
        ctx.org_id, ctx.project_id
    ));
    template
        .replace("{{CAS_FULL_NAME_TAG}}", &cas_tag)
        .replace("{{PROJECT_BASENAME}}", &escape_md_cell(&ctx.project_basename))
        .replace("{{ORG_ID}}", &escape_md_cell_code(&ctx.org_id))
        .replace("{{PROJECT_ID}}", &escape_md_cell_code(&ctx.project_id))
        .replace("{{DATABASE_NAME}}", &escape_md_cell_code(&ctx.database_name))
        .replace("{{BRANCH_TABLE_ROWS}}", &branch_rows)
}

/// Decoded contents of the `<!-- keep neon-ids -->` block.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KeepPayload {
    org_id: String,
    project_id: String,
    database_name: String,
    /// (label, branchId) pairs.
    branches: Vec<(String, String)>,
}

/// The project and its production branch ids recorded in a generated Neon
/// skill file's `neon-ids` keep block (GH #907): the worker SQL write guard
/// reads them to recognise a production `branchId`. `None` when the file has
/// no parsable block.
pub fn recorded_production_branches(existing: &str) -> Option<(String, Vec<String>)> {
    let payload = parse_keep_payload(existing).ok()?;
    let production = payload
        .branches
        .into_iter()
        .filter(|(label, _)| label == "production")
        .map(|(_, id)| id)
        .collect();
    Some((payload.project_id, production))
}

fn parse_keep_payload(existing: &str) -> Result<KeepPayload> {
    let blocks = keep_block::extract(existing)
        .with_context(|| "parsing keep blocks in existing skill file")?;
    let block = blocks
        .into_iter()
        .find(|b| b.name.as_deref() == Some(KEEP_IDS_BLOCK))
        .ok_or_else(|| anyhow!("no `<!-- keep neon-ids -->` block found in existing skill file"))?;

    let mut org_id = None;
    let mut project_id = None;
    let mut database_name = None;
    let mut branches: Vec<(String, String)> = Vec::new();

    for line in block.body.lines() {
        // Markdown table rows look like `| **org_id** | \`value\` |` or
        // `| **production branchId** | \`br-foo\` (name: \`main\`) |`.
        if let Some((label, value)) = parse_kv_row(line) {
            let lower = label.to_lowercase();
            if lower == "org_id" {
                org_id = Some(value);
            } else if lower == "projectid" {
                project_id = Some(value);
            } else if lower == "databasename" {
                database_name = Some(value);
            } else if let Some(env) = lower.strip_suffix(" branchid") {
                branches.push((env.to_string(), value));
            }
        }
    }

    Ok(KeepPayload {
        org_id: org_id.ok_or_else(|| anyhow!("keep block missing org_id"))?,
        project_id: project_id.ok_or_else(|| anyhow!("keep block missing projectId"))?,
        database_name: database_name
            .ok_or_else(|| anyhow!("keep block missing databaseName"))?,
        branches,
    })
}

/// Parse one `| **<label>** | \`<value>\` ... |` row. Returns
/// (label_without_bold, value_without_backticks). Tolerates trailing parenthetical.
fn parse_kv_row(line: &str) -> Option<(String, String)> {
    let trim = line.trim();
    if !trim.starts_with('|') || !trim.ends_with('|') {
        return None;
    }
    let cells: Vec<&str> = trim.trim_matches('|').split('|').collect();
    if cells.len() < 2 {
        return None;
    }
    let label = cells[0].trim().trim_matches('*').trim().to_string();
    let raw_value = cells[1].trim();
    // Pull the *first* backtick span: opening backtick, then the next backtick
    // strictly after it. If either is missing we fall through to the raw
    // trimmed cell so callers still get something to inspect.
    let value = match raw_value.find('`') {
        Some(s) => match raw_value[s + 1..].find('`') {
            Some(rel_end) => raw_value[s + 1..s + 1 + rel_end].to_string(),
            None => raw_value[s + 1..].to_string(),
        },
        None => raw_value.to_string(),
    };
    if label.is_empty() {
        return None;
    }
    Some((label, value))
}

// --- Interactive prompt seams ---------------------------------------------
//
// These are split out so unit tests can exercise the non-interactive paths
// (auto-pick / strong-match / fixture-driven choices) without spinning up a
// terminal.

fn pick_org_interactively(orgs: &[NeonOrg]) -> Result<String> {
    use inquire::Select;
    let display: Vec<String> = orgs
        .iter()
        .map(|o| format!("{} — {}", o.name, o.id))
        .collect();
    let chosen = Select::new("Multiple Neon orgs available — pick one:", display.clone())
        .prompt()
        .with_context(|| "Neon org selection prompt failed (run with --org-id to bypass)")?;
    let idx = display
        .iter()
        .position(|d| d == &chosen)
        .ok_or_else(|| anyhow!("inquire returned a selection not present in display list"))?;
    Ok(orgs[idx].id.clone())
}

/// Candidate projects whose name is bidirectionally a substring of `basename`
/// (or vice versa), with a 3-char minimum-length guard on BOTH operands
/// (cas-fc38). Generic 1–2 char project names like "ui"/"db" or 1–2 char repo
/// basenames otherwise auto-match every project.
///
/// Pulled out of [`pick_project`] so tests can exercise the actual filter
/// instead of re-implementing it.
pub(crate) fn fuzzy_substring_candidates<'a>(
    projects: &'a [NeonProject],
    basename: &str,
) -> Vec<&'a NeonProject> {
    const MIN_FUZZY_LEN: usize = 3;
    if basename.chars().count() < MIN_FUZZY_LEN {
        return Vec::new();
    }
    projects
        .iter()
        .filter(|p| {
            p.name.chars().count() >= MIN_FUZZY_LEN
                && (p.name.contains(basename) || basename.contains(&p.name))
        })
        .collect()
}

fn pick_project(projects: &[NeonProject], basename: &str) -> Result<String> {
    // Strong match: exact name == basename, or single substring hit.
    if let Some(exact) = projects.iter().find(|p| p.name == basename) {
        return Ok(exact.id.clone());
    }

    let substring = fuzzy_substring_candidates(projects, basename);
    if substring.len() == 1 {
        return Ok(substring[0].id.clone());
    }

    use inquire::Select;
    let display: Vec<String> = projects
        .iter()
        .map(|p| format!("{} — {}", p.name, p.id))
        .collect();
    let chosen = Select::new(
        &format!(
            "No strong match for repo basename {basename:?} — pick a Neon project:"
        ),
        display.clone(),
    )
    .prompt()
    .with_context(|| "Neon project selection prompt failed (run with --project-id to bypass)")?;
    let idx = display
        .iter()
        .position(|d| d == &chosen)
        .ok_or_else(|| anyhow!("inquire returned a selection not present in display list"))?;
    Ok(projects[idx].id.clone())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
pub(crate) mod tests {
    use super::*;


    use std::cell::RefCell;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// In-memory fake satisfying [`NeonClient`].
    pub struct FakeNeonClient {
        pub orgs: Vec<NeonOrg>,
        pub projects_by_org: HashMap<String, Vec<NeonProject>>,
        pub details: HashMap<(String, String), NeonProjectDetail>,
        pub live_branches: RefCell<HashMap<(String, String, String), bool>>,
    }

    impl FakeNeonClient {
        pub fn empty() -> Self {
            Self {
                orgs: Vec::new(),
                projects_by_org: HashMap::new(),
                details: HashMap::new(),
                live_branches: RefCell::new(HashMap::new()),
            }
        }
    }

    impl NeonClient for FakeNeonClient {
        fn list_organizations(&self) -> Result<Vec<NeonOrg>> {
            Ok(self.orgs.clone())
        }
        fn list_projects(&self, org_id: &str) -> Result<Vec<NeonProject>> {
            Ok(self
                .projects_by_org
                .get(org_id)
                .cloned()
                .unwrap_or_default())
        }
        fn describe_project(&self, org_id: &str, project_id: &str) -> Result<NeonProjectDetail> {
            self.details
                .get(&(org_id.to_string(), project_id.to_string()))
                .cloned()
                .ok_or_else(|| anyhow!("fake: no detail for {org_id}/{project_id}"))
        }
        fn describe_branch(
            &self,
            org_id: &str,
            project_id: &str,
            branch_id: &str,
        ) -> Result<bool> {
            Ok(self
                .live_branches
                .borrow()
                .get(&(
                    org_id.to_string(),
                    project_id.to_string(),
                    branch_id.to_string(),
                ))
                .copied()
                .unwrap_or(false))
        }
    }

    fn make_repo() -> TempDir {
        TempDir::new().expect("tempdir")
    }

    fn write_prisma_with_neon(repo: &Path) {
        let prisma_dir = repo.join("prisma");
        fs::create_dir_all(&prisma_dir).unwrap();
        fs::write(
            prisma_dir.join("schema.prisma"),
            "datasource db {\n  provider = \"postgresql\"\n  url = env(\"DATABASE_URL\")\n}\n// neon.tech\n",
        )
        .unwrap();
    }

    fn write_pkg_with_neon_dep(repo: &Path) {
        fs::write(
            repo.join("package.json"),
            r#"{ "dependencies": { "@neondatabase/serverless": "^0.9.0" } }"#,
        )
        .unwrap();
    }

    fn fixture_with_two_orgs() -> FakeNeonClient {
        let mut client = FakeNeonClient::empty();
        client.orgs = vec![
            NeonOrg {
                id: "org-a".into(),
                name: "Org A".into(),
            },
            NeonOrg {
                id: "org-b".into(),
                name: "Org B".into(),
            },
        ];
        client.projects_by_org.insert(
            "org-a".into(),
            vec![NeonProject {
                id: "proj-a".into(),
                name: "demo".into(),
            }],
        );
        client.details.insert(
            ("org-a".into(), "proj-a".into()),
            NeonProjectDetail {
                project: NeonProject {
                    id: "proj-a".into(),
                    name: "demo".into(),
                },
                default_database: "neondb".into(),
                branches: vec![
                    NeonBranch {
                        id: "br-main".into(),
                        name: "main".into(),
                        is_default: true,
                    },
                    NeonBranch {
                        id: "br-staging".into(),
                        name: "staging".into(),
                        is_default: false,
                    },
                    NeonBranch {
                        id: "br-dev".into(),
                        name: "dev-feature".into(),
                        is_default: false,
                    },
                ],
            },
        );
        client
            .live_branches
            .borrow_mut()
            .insert(("org-a".into(), "proj-a".into(), "br-main".into()), true);
        client.live_branches.borrow_mut().insert(
            ("org-a".into(), "proj-a".into(), "br-staging".into()),
            true,
        );
        client
            .live_branches
            .borrow_mut()
            .insert(("org-a".into(), "proj-a".into(), "br-dev".into()), true);
        client
    }

    // --- Detection ---------------------------------------------------------

    #[test]
    fn detect_finds_prisma_neon_url() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let d = detect(repo.path());
        assert!(d.prisma_neon_url);
        assert!(!d.package_neon_dep);
        assert!(d.detected());
    }

    #[test]
    fn detect_finds_neondatabase_dep() {
        let repo = make_repo();
        write_pkg_with_neon_dep(repo.path());
        let d = detect(repo.path());
        assert!(!d.prisma_neon_url);
        assert!(d.package_neon_dep);
        assert!(d.detected());
    }

    #[test]
    fn detect_finds_prisma_adapter_neon() {
        let repo = make_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{ "dependencies": { "@prisma/adapter-neon": "^1.0.0" } }"#,
        )
        .unwrap();
        assert!(detect(repo.path()).detected());
    }

    #[test]
    fn detect_returns_false_when_no_signals() {
        let repo = make_repo();
        let d = detect(repo.path());
        assert!(!d.detected());
    }

    // --- Branch heuristic --------------------------------------------------

    #[test]
    fn map_branches_labels_default_as_production() {
        let bs = vec![
            NeonBranch {
                id: "1".into(),
                name: "main".into(),
                is_default: true,
            },
            NeonBranch {
                id: "2".into(),
                name: "staging".into(),
                is_default: false,
            },
            NeonBranch {
                id: "3".into(),
                name: "dev-foo".into(),
                is_default: false,
            },
            NeonBranch {
                id: "4".into(),
                name: "feature-x".into(),
                is_default: false,
            },
        ];
        let mapped = map_branches(&bs);
        assert_eq!(mapped[0].0, "production");
        assert_eq!(mapped[1].0, "staging");
        assert_eq!(mapped[2].0, "dev");
        assert_eq!(mapped[3].0, "feature-x");
    }

    // --- Init: skip when not detected -------------------------------------

    #[test]
    fn init_skips_cleanly_when_no_neon_signals() {
        let repo = make_repo();
        let client = FakeNeonClient::empty();
        let outcome = init(repo.path(), &client, InitChoices::default()).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Skipped);
        assert!(outcome.files.is_empty());
        assert!(!repo.path().join(CLAUDE_SKILL).exists());
    }

    // --- Init: happy path --------------------------------------------------

    #[test]
    fn render_template_escapes_branch_name_with_pipes_and_keep_markers() {
        // cas-fc38 verifier feedback: branch names are user-controlled at the
        // Neon dashboard. A name with `|`, `<!--`, `-->`, or backticks must
        // not break the rendered table or corrupt the surrounding
        // <!-- keep neon-ids --> markers.
        let ctx = TemplateContext {
            project_basename: "x".to_string(),
            org_id: "org".to_string(),
            project_id: "p".to_string(),
            database_name: "db".to_string(),
            branch_map: vec![(
                "production".to_string(),
                NeonBranch {
                    id: "br-1".to_string(),
                    name: "evil|branch <!-- gotcha -->".to_string(),
                    is_default: true,
                },
            )],
        };
        let doc = render_template(TEMPLATE_CLAUDE, &ctx);

        // 1. Keep block must still parse cleanly.
        let blocks = super::super::keep_block::extract(&doc).expect("keep markers intact");
        let ids = blocks
            .iter()
            .find(|b| b.name.as_deref() == Some(KEEP_IDS_BLOCK))
            .expect("neon-ids block must exist");

        // 2. The `|` was escaped to `\|` (preserves the cell layout).
        assert!(
            ids.body.contains("evil\\|branch"),
            "literal `|` must be escaped: {body}",
            body = ids.body
        );
        // 3. `<!--` and `-->` were neutralized.
        assert!(!ids.body.contains("<!-- gotcha"));
        assert!(ids.body.contains("&lt;!-- gotcha"));
        assert!(!ids.body.contains("gotcha -->"));
    }

    #[test]
    fn render_template_escapes_org_and_project_id_with_pipes() {
        // Neon ids are typically slug-shaped, but we treat them as
        // user-controlled per the convention.
        let ctx = TemplateContext {
            project_basename: "x".to_string(),
            org_id: "org|with|pipes".to_string(),
            project_id: "p`with`backticks".to_string(),
            database_name: "db".to_string(),
            branch_map: vec![],
        };
        let doc = render_template(TEMPLATE_CLAUDE, &ctx);
        let blocks = super::super::keep_block::extract(&doc).expect("keep markers intact");
        let ids = blocks
            .iter()
            .find(|b| b.name.as_deref() == Some(KEEP_IDS_BLOCK))
            .unwrap();
        // Restrict assertions to the table rows (skip the cas:full_name tag,
        // which legitimately preserves raw bytes — backticks inside an HTML
        // comment cannot open a code span).
        let table_only: String = ids
            .body
            .lines()
            .filter(|l| l.trim_start().starts_with('|'))
            .collect::<Vec<_>>()
            .join("\n");
        // Pipes in ORG_ID escaped in the table cell.
        assert!(
            table_only.contains("org\\|with\\|pipes"),
            "ORG_ID pipes must be escaped in cell: {table_only}"
        );
        // Backticks in PROJECT_ID stripped in the table cell (the rendered
        // row is `pwithbackticks` after escape_md_cell_code).
        assert!(
            table_only.contains("`pwithbackticks`"),
            "PROJECT_ID backticks must be stripped in cell: {table_only}"
        );
        assert!(
            !table_only.contains("p`with`backticks"),
            "raw backticks must not appear in any cell: {table_only}"
        );
    }

    #[test]
    fn render_template_sanitizes_cas_full_name_tag_against_close_marker() {
        // cas-fc38 autofix: an org_id or project_id containing `-->` or a
        // newline must not corrupt the surrounding `<!-- keep neon-ids -->`
        // markers. The render path routes the {ORG}/{PROJECT} pair through
        // emit_cas_full_name_tag.
        let ctx = TemplateContext {
            project_basename: "x".to_string(),
            org_id: "org-->bad".to_string(),
            project_id: "p\n2".to_string(),
            database_name: "db".to_string(),
            branch_map: vec![],
        };
        let doc = render_template(TEMPLATE_CLAUDE, &ctx);
        let blocks = super::super::keep_block::extract(&doc).unwrap();
        let ids_block = blocks
            .iter()
            .find(|b| b.name.as_deref() == Some(KEEP_IDS_BLOCK))
            .expect("neon-ids keep block must still parse");
        let recovered =
            super::super::md::parse_cas_full_name_tag(&ids_block.body).unwrap();
        assert!(
            !recovered.contains("-->"),
            "literal `-->` must be neutralized: {recovered}"
        );
        assert!(
            !recovered.contains('\n'),
            "tag value must be single-line: {recovered}"
        );
    }

    #[test]
    fn init_writes_three_files_with_keep_block_payload() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let client = fixture_with_two_orgs();
        let choices = InitChoices {
            org_id: Some("org-a".into()),
            project_id: Some("proj-a".into()),
            ..Default::default()
        };
        let outcome = init(repo.path(), &client, choices).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Configured);
        assert_eq!(outcome.files.len(), 3);

        let claude = fs::read_to_string(repo.path().join(CLAUDE_SKILL)).unwrap();
        assert!(claude.contains("<!-- keep neon-ids -->"));
        assert!(claude.contains("<!-- /keep neon-ids -->"));
        // cas-fc38: canonical cas:full_name tag inside the keep block.
        assert!(
            claude.contains("<!-- cas:full_name=org-a/proj-a -->"),
            "expected cas:full_name tag in claude SKILL: {claude}"
        );
        assert!(claude.contains("org-a"));
        assert!(claude.contains("proj-a"));
        assert!(claude.contains("neondb"));
        assert!(claude.contains("br-main"));
        assert!(claude.contains("br-staging"));
        // Ordering: production before staging before dev.
        let prod = claude.find("production branchId").unwrap();
        let stag = claude.find("staging branchId").unwrap();
        assert!(prod < stag);

        let cursor = fs::read_to_string(repo.path().join(CURSOR_SKILL)).unwrap();
        assert!(cursor.contains("br-main"));
        assert!(cursor.contains("server: user-neon"));

        let queries = fs::read_to_string(repo.path().join(CLAUDE_QUERIES)).unwrap();
        assert!(queries.contains("3-step flow"));
    }

    // --- Init: multi-org auto-pick path -----------------------------------

    #[test]
    fn init_uses_injected_org_id_in_multi_org_environment() {
        // Even though there are 2 orgs available, providing org_id via choices
        // bypasses the prompt — exactly the path tests need.
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let client = fixture_with_two_orgs();
        let choices = InitChoices {
            org_id: Some("org-a".into()),
            project_id: Some("proj-a".into()),
            ..Default::default()
        };
        let outcome = init(repo.path(), &client, choices).unwrap();
        let claude = fs::read_to_string(repo.path().join(CLAUDE_SKILL)).unwrap();
        assert!(claude.contains("org-a"));
        // The other org's id should not leak in.
        assert!(!claude.contains("org-b"));
        assert_eq!(outcome.status, IntegrationStatus::Configured);
    }

    // --- Refresh: preserves keep block by default -------------------------

    #[test]
    fn refresh_default_preserves_user_edited_keep_block() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let client = fixture_with_two_orgs();
        let choices = InitChoices {
            org_id: Some("org-a".into()),
            project_id: Some("proj-a".into()),
            ..Default::default()
        };
        init(repo.path(), &client, choices).unwrap();

        // Hand-edit the keep block — pretend the user added a hand-curated row.
        let path = repo.path().join(CLAUDE_SKILL);
        let mut content = fs::read_to_string(&path).unwrap();
        content = content.replace(
            "| **org_id** | `org-a` |",
            "| **org_id** | `org-a` |\n| **note** | `manual edit retained` |",
        );
        fs::write(&path, &content).unwrap();

        let outcome =
            refresh(repo.path(), &client, RefreshOpts { update_ids: false }).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Refreshed);

        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("manual edit retained"),
            "user-added keep-block content should survive refresh:\n{after}"
        );
    }

    // --- Refresh: --update-ids re-fetches ----------------------------------

    #[test]
    fn refresh_with_update_ids_overwrites_keep_block_with_fresh_data() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let client = fixture_with_two_orgs();
        init(
            repo.path(),
            &client,
            InitChoices {
                org_id: Some("org-a".into()),
                project_id: Some("proj-a".into()),
                ..Default::default()
            },
        )
        .unwrap();

        // Sanity: hand-edit a stale value to prove update-ids overwrites it.
        let path = repo.path().join(CLAUDE_SKILL);
        let original = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            original.replace("`neondb`", "`old-stale-name-from-user-edit`"),
        )
        .unwrap();

        let outcome =
            refresh(repo.path(), &client, RefreshOpts { update_ids: true }).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Refreshed);

        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("`neondb`"),
            "update_ids should restore freshly-fetched databaseName:\n{after}"
        );
        assert!(
            !after.contains("old-stale-name-from-user-edit"),
            "stale user-edit should be overwritten by update_ids:\n{after}"
        );
    }

    // --- Refresh: skip when nothing to refresh -----------------------------

    #[test]
    fn refresh_skips_when_no_existing_skill_files() {
        let repo = make_repo();
        let client = FakeNeonClient::empty();
        let outcome = refresh(repo.path(), &client, RefreshOpts::default()).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Skipped);
    }

    // --- Verify: ok and stale ---------------------------------------------

    #[test]
    fn verify_reports_ok_when_all_branches_live() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let client = fixture_with_two_orgs();
        init(
            repo.path(),
            &client,
            InitChoices {
                org_id: Some("org-a".into()),
                project_id: Some("proj-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let outcome = verify(repo.path(), &client).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::AlreadyConfigured);
        let joined = outcome.summary.join("\n");
        assert!(joined.contains("ok"));
        assert!(!joined.contains("STALE"));
    }

    #[test]
    fn verify_reports_stale_only_for_deleted_branch() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let client = fixture_with_two_orgs();
        init(
            repo.path(),
            &client,
            InitChoices {
                org_id: Some("org-a".into()),
                project_id: Some("proj-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        // Mark dev branch as deleted.
        client
            .live_branches
            .borrow_mut()
            .insert(("org-a".into(), "proj-a".into(), "br-dev".into()), false);
        let outcome = verify(repo.path(), &client).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Stale);
        let joined = outcome.summary.join("\n");
        assert!(joined.contains("br-dev"));
        assert!(joined.contains("STALE"));
        // Other branches should still be ok.
        assert!(joined.contains("br-main"));
        assert!(joined.contains("br-staging"));
    }

    #[test]
    fn verify_errors_when_skill_file_missing() {
        let repo = make_repo();
        let client = FakeNeonClient::empty();
        let err = verify(repo.path(), &client).unwrap_err().to_string();
        assert!(
            err.contains("cas integrate neon init"),
            "expected hint to run init: {err}"
        );
    }

    // --- Keep-payload parser ----------------------------------------------

    #[test]
    fn parse_keep_payload_round_trips_init_output() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let client = fixture_with_two_orgs();
        init(
            repo.path(),
            &client,
            InitChoices {
                org_id: Some("org-a".into()),
                project_id: Some("proj-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let content = fs::read_to_string(repo.path().join(CLAUDE_SKILL)).unwrap();
        let payload = parse_keep_payload(&content).unwrap();
        assert_eq!(payload.org_id, "org-a");
        assert_eq!(payload.project_id, "proj-a");
        assert_eq!(payload.database_name, "neondb");
        let labels: Vec<&str> = payload.branches.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.contains(&"production"));
        assert!(labels.contains(&"staging"));
        assert!(labels.contains(&"dev"));
    }

    #[test]
    fn parse_keep_payload_errors_when_block_missing() {
        let err = parse_keep_payload("# Hi\nNo keep block.\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("neon-ids"));
    }

    // --- Templates well-formed (extract round-trip) ------------------------

    #[test]
    fn templates_have_well_formed_keep_blocks() {
        // The keep-block helper must accept our templates verbatim.
        for (label, tmpl) in [("claude", TEMPLATE_CLAUDE), ("cursor", TEMPLATE_CURSOR)] {
            let blocks = keep_block::extract(tmpl)
                .unwrap_or_else(|e| panic!("{label} template parse error: {e}"));
            assert_eq!(
                blocks.len(),
                1,
                "{label} template should have exactly one keep block"
            );
            assert_eq!(blocks[0].name.as_deref(), Some(KEEP_IDS_BLOCK));
        }
    }

    // --- Reviewer-driven coverage additions (cas-1ece autofix round) -------

    #[test]
    fn detect_finds_monorepo_apps_backend_path() {
        let repo = make_repo();
        let dir = repo.path().join("apps/backend/prisma");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("schema.prisma"),
            "// neon.tech\ndatasource db { provider=\"postgresql\" }\n",
        )
        .unwrap();
        assert!(detect(repo.path()).prisma_neon_url);
    }

    #[test]
    fn map_branches_disambiguates_colliding_dev_labels() {
        // Two dev-* branches should not collapse to a single `dev` label.
        let bs = vec![
            NeonBranch {
                id: "1".into(),
                name: "main".into(),
                is_default: true,
            },
            NeonBranch {
                id: "2".into(),
                name: "dev-api".into(),
                is_default: false,
            },
            NeonBranch {
                id: "3".into(),
                name: "dev-web".into(),
                is_default: false,
            },
        ];
        let labels: Vec<String> = map_branches(&bs).into_iter().map(|(l, _)| l).collect();
        assert_eq!(labels, vec!["production", "dev", "dev-2"]);
    }

    #[test]
    fn init_errors_when_no_organizations() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let client = FakeNeonClient::empty(); // no orgs
        let err = init(repo.path(), &client, InitChoices::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no organizations"), "got: {err}");
    }

    #[test]
    fn init_errors_when_no_projects_for_org() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let mut client = FakeNeonClient::empty();
        client.orgs = vec![NeonOrg {
            id: "org-only".into(),
            name: "Solo".into(),
        }];
        // No projects registered for org-only.
        let err = init(repo.path(), &client, InitChoices::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no projects"), "got: {err}");
    }

    #[test]
    fn init_auto_picks_single_org_without_prompting() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let mut client = FakeNeonClient::empty();
        client.orgs = vec![NeonOrg {
            id: "org-solo".into(),
            name: "Solo".into(),
        }];
        client.projects_by_org.insert(
            "org-solo".into(),
            vec![NeonProject {
                id: "proj-solo".into(),
                // Match the tempdir basename so pick_project's substring
                // heuristic accepts it without prompting.
                name: repo
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            }],
        );
        client.details.insert(
            ("org-solo".into(), "proj-solo".into()),
            NeonProjectDetail {
                project: NeonProject {
                    id: "proj-solo".into(),
                    name: "anything".into(),
                },
                default_database: "neondb".into(),
                branches: vec![NeonBranch {
                    id: "br-main".into(),
                    name: "main".into(),
                    is_default: true,
                }],
            },
        );
        let outcome = init(repo.path(), &client, InitChoices::default()).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Configured);
        let claude = fs::read_to_string(repo.path().join(CLAUDE_SKILL)).unwrap();
        assert!(claude.contains("org-solo"));
    }

    #[test]
    fn refresh_default_refuses_when_keep_block_missing() {
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        // Write a SKILL.md with NO keep-neon-ids block — placeholder leak guard
        // must refuse rather than write `<org_id>` etc into the user's file.
        let p = repo.path().join(CLAUDE_SKILL);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "# manually edited\nno keep block here.\n").unwrap();
        let client = FakeNeonClient::empty();
        let err = refresh(repo.path(), &client, RefreshOpts { update_ids: false })
            .unwrap_err()
            .to_string();
        assert!(err.contains("neon-ids"), "got: {err}");
    }

    #[test]
    fn parse_kv_row_extracts_first_backtick_span() {
        // Branch rows look like: `| **production branchId** | `br-foo` (name: `main`) |`
        let row = "| **production branchId** | `br-foo` (name: `main`) |";
        let (label, value) = parse_kv_row(row).unwrap();
        assert_eq!(label, "production branchId");
        assert_eq!(
            value, "br-foo",
            "first backtick span only (not first-to-last)"
        );
    }

    #[test]
    fn parse_kv_row_returns_none_for_non_table_lines() {
        assert!(parse_kv_row("# not a table row").is_none());
        assert!(parse_kv_row("| only one cell").is_none());
    }

    #[test]
    fn verify_treats_describe_branch_err_as_transport_error_with_error_text() {
        // Bespoke client: describe_branch returns Err for the dev branch.
        struct ErrClient {
            inner: FakeNeonClient,
        }
        impl NeonClient for ErrClient {
            fn list_organizations(&self) -> Result<Vec<NeonOrg>> {
                self.inner.list_organizations()
            }
            fn list_projects(&self, o: &str) -> Result<Vec<NeonProject>> {
                self.inner.list_projects(o)
            }
            fn describe_project(&self, o: &str, p: &str) -> Result<NeonProjectDetail> {
                self.inner.describe_project(o, p)
            }
            fn describe_branch(&self, o: &str, p: &str, b: &str) -> Result<bool> {
                if b == "br-dev" {
                    Err(anyhow!("network unreachable"))
                } else {
                    self.inner.describe_branch(o, p, b)
                }
            }
        }

        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let inner = fixture_with_two_orgs();
        let client = ErrClient { inner };
        init(
            repo.path(),
            &client,
            InitChoices {
                org_id: Some("org-a".into()),
                project_id: Some("proj-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let outcome = verify(repo.path(), &client).unwrap();
        // cas-fc38: transport errors map to TransportError, not Stale. Stale
        // means "platform answered, the answer disagrees"; here the platform
        // never answered for this branch.
        assert_eq!(outcome.status, IntegrationStatus::TransportError);
        let joined = outcome.summary.join("\n");
        assert!(
            joined.contains("network unreachable"),
            "transport error text should appear in summary: {joined}"
        );
        assert!(
            joined.contains("transport error"),
            "summary should label the line as a transport error: {joined}"
        );
    }

    #[test]
    fn verify_drift_wins_over_transport_error_when_both_present() {
        // Bespoke client: one branch returns transport error, another
        // returns Ok(false) (genuine drift). Stale should win — knowing
        // we have at least one confirmed-missing branch is more actionable
        // than a transport blip elsewhere.
        struct MixedClient {
            inner: FakeNeonClient,
        }
        impl NeonClient for MixedClient {
            fn list_organizations(&self) -> Result<Vec<NeonOrg>> {
                self.inner.list_organizations()
            }
            fn list_projects(&self, o: &str) -> Result<Vec<NeonProject>> {
                self.inner.list_projects(o)
            }
            fn describe_project(&self, o: &str, p: &str) -> Result<NeonProjectDetail> {
                self.inner.describe_project(o, p)
            }
            fn describe_branch(&self, o: &str, p: &str, b: &str) -> Result<bool> {
                if b == "br-dev" {
                    Err(anyhow!("transient error"))
                } else if b == "br-staging" {
                    Ok(false)
                } else {
                    self.inner.describe_branch(o, p, b)
                }
            }
        }
        let repo = make_repo();
        write_prisma_with_neon(repo.path());
        let inner = fixture_with_two_orgs();
        let client = MixedClient { inner };
        init(
            repo.path(),
            &client,
            InitChoices {
                org_id: Some("org-a".into()),
                project_id: Some("proj-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let outcome = verify(repo.path(), &client).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Stale);
    }

    #[test]
    fn fuzzy_substring_candidates_3char_guard_blocks_short_basename() {
        // Direct test against the production helper (cas-fc38 autofix).
        let projects = vec![
            NeonProject { id: "p1".into(), name: "xenon".into() },
            NeonProject { id: "p2".into(), name: "xerus".into() },
        ];
        // 1-char basename → 0 candidates regardless of project name length.
        assert!(fuzzy_substring_candidates(&projects, "x").is_empty());
        // 2-char basename → still 0.
        assert!(fuzzy_substring_candidates(&projects, "xe").is_empty());
        // 3-char basename → matches both via .contains().
        let hits = fuzzy_substring_candidates(&projects, "xen");
        assert_eq!(hits.len(), 1, "only 'xenon' contains 'xen'");
        assert_eq!(hits[0].id, "p1");
    }

    #[test]
    fn fuzzy_substring_candidates_3char_guard_blocks_short_project_name() {
        // Even with a long basename, projects with 1- or 2-char names must
        // not bidirectionally-fuzzy-match.
        let projects = vec![
            NeonProject { id: "p1".into(), name: "ui".into() }, // 2 chars
            NeonProject { id: "p2".into(), name: "x".into() },  // 1 char
            NeonProject { id: "p3".into(), name: "ui-stack".into() },
        ];
        let hits = fuzzy_substring_candidates(&projects, "my-ui-app");
        // Only "ui-stack" survives; the bare "ui" / "x" projects are blocked
        // by the per-project name_long_enough check, and "my-ui-app" doesn't
        // contain "ui-stack" so the bidirectional miss is also fine.
        // Intersection: "ui-stack" length 8 ≥ 3, basename contains "ui-stack"?
        // No — "my-ui-app" doesn't contain "ui-stack". Vice versa? "ui-stack"
        // doesn't contain "my-ui-app". So 0 hits — exactly what we want.
        assert!(
            hits.is_empty(),
            "no candidate is bidirectional substring with 'my-ui-app': {hits:?}"
        );
    }
}
