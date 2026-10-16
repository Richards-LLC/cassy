//! Legacy Neon JSON/MCP-envelope compatibility fixtures for parsers exposed by
//! [`super::neon`] (cas-1549).
//!
//! Each fixture lives under `cas-cli/src/cli/integrate/fixtures/neon/` and is
//! embedded via `include_str!` so the parsers run against canonical Neon MCP
//! envelopes without any live transport. Parser-only tests live here (rather
//! than inline inside `neon.rs`'s `mod tests`) so the test-first posture
//! invariant holds — a future regression to a parser is caught by a test in a
//! file whose name advertises its subject.
//!
//! LiveNeonClient uses neonctl; these fixtures preserve the pure parsers
//! without requiring any live endpoint or MCP registration.

#![cfg(test)]

use super::neon::{
    parse_describe_branch, parse_describe_project, parse_list_organizations,
    parse_list_projects,
};

const FIXTURE_LIST_ORGS: &str = include_str!("fixtures/neon/list_organizations.json");
const FIXTURE_LIST_PROJECTS: &str = include_str!("fixtures/neon/list_projects.json");
const FIXTURE_DESCRIBE_PROJECT: &str = include_str!("fixtures/neon/describe_project.json");
const FIXTURE_DESCRIBE_BRANCH_PRESENT: &str =
    include_str!("fixtures/neon/describe_branch_present.json");
const FIXTURE_DESCRIBE_BRANCH_MISSING: &str =
    include_str!("fixtures/neon/describe_branch_missing.json");

fn parse_fixture(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("fixture must be valid JSON")
}

// --- list_organizations -----------------------------------------------------

#[test]
fn parse_list_organizations_handles_canonical_envelope() {
    let v = parse_fixture(FIXTURE_LIST_ORGS);
    let orgs = parse_list_organizations(&v).unwrap();
    assert_eq!(orgs.len(), 2);
    assert_eq!(orgs[0].id, "org-2k3q");
    assert_eq!(orgs[0].name, "Petra Stella");
    assert_eq!(orgs[1].id, "org-9m5p");
}

#[test]
fn parse_list_organizations_handles_bare_array() {
    let v = serde_json::json!([
        {"id": "org-1", "name": "A"},
        {"id": "org-2", "name": "B"}
    ]);
    let orgs = parse_list_organizations(&v).unwrap();
    assert_eq!(orgs.len(), 2);
}

// (cas-36fd0 dropped the `orgs` alias — it was speculative tolerance
// added without a real fixture. The corresponding test is removed.)

#[test]
fn parse_list_organizations_propagates_is_error_envelope() {
    let v = serde_json::json!({
        "content": [{"type": "text", "text": "{}"}],
        "isError": true
    });
    let err = parse_list_organizations(&v).unwrap_err();
    assert!(err.to_string().contains("isError=true"), "got: {err}");
}

#[test]
fn parse_list_organizations_bails_on_unknown_wrapper_key() {
    // A future upstream shape drift to {"items": [...]} must NOT silently
    // masquerade as an empty org list.
    let v = serde_json::json!({"items": [{"id": "org-1", "name": "X"}]});
    let err = parse_list_organizations(&v).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("no `organizations` array"), "got: {s}");
    // Bail message must include a recovery hint citing cas-36fd0.
    assert!(s.contains("cas-36fd0"), "missing recovery hint: {s}");
}

#[test]
fn parse_list_organizations_errors_on_malformed_text_envelope() {
    let v = serde_json::json!({
        "content": [{"type": "text", "text": "definitely not json {{}"}]
    });
    let err = parse_list_organizations(&v).unwrap_err();
    assert!(
        err.to_string().contains("parsing MCP text content"),
        "got: {err}"
    );
}

// --- list_projects ----------------------------------------------------------

#[test]
fn parse_list_projects_handles_canonical_envelope() {
    let v = parse_fixture(FIXTURE_LIST_PROJECTS);
    let projects = parse_list_projects(&v).unwrap();
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].id, "proj-cool-bird");
    assert_eq!(projects[0].name, "gabber-studio");
    assert_eq!(projects[1].name, "ozer-health");
}

// (cas-36fd0 dropped the `data` alias — speculative tolerance without a
// real fixture. The corresponding test is removed.)

#[test]
fn parse_list_projects_propagates_is_error_envelope() {
    let v = serde_json::json!({
        "content": [{"type": "text", "text": "{}"}],
        "isError": true
    });
    let err = parse_list_projects(&v).unwrap_err();
    assert!(err.to_string().contains("isError=true"), "got: {err}");
}

#[test]
fn parse_list_projects_bails_on_unknown_wrapper_key() {
    let v = serde_json::json!({"items": [{"id": "p1", "name": "x"}]});
    let err = parse_list_projects(&v).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("no `projects` array"), "got: {s}");
    assert!(s.contains("cas-36fd0"), "missing recovery hint: {s}");
}

// --- describe_project -------------------------------------------------------

#[test]
fn parse_describe_project_handles_canonical_envelope() {
    let v = parse_fixture(FIXTURE_DESCRIBE_PROJECT);
    let detail = parse_describe_project(&v).unwrap();
    assert_eq!(detail.project.id, "proj-cool-bird");
    assert_eq!(detail.project.name, "gabber-studio");
    assert_eq!(detail.default_database, "neondb");
    assert_eq!(detail.branches.len(), 3);
    let main = detail
        .branches
        .iter()
        .find(|b| b.id == "br-main-1")
        .unwrap();
    assert!(main.is_default);
    let staging = detail
        .branches
        .iter()
        .find(|b| b.id == "br-staging-7")
        .unwrap();
    assert!(!staging.is_default);
}

// (cas-36fd0 dropped the flat-object describe_project tolerance — it was
// added defensively without a real fixture. Canonical shape now requires
// the nested `project` object. The corresponding test is removed.)

#[test]
fn parse_describe_project_bails_when_no_default_database_name_is_resolvable() {
    // cas-36fd0: the silent "neondb" fallback used to mask upstream-shape
    // drift. We now require the database name to be discoverable from one
    // of (project.default_database_name, top-level default_database,
    // databases[0].name) — anything else is an explicit error so the user
    // can see they need to investigate.
    let v = serde_json::json!({
        "project": {"id": "p", "name": "n"},
        "branches": []
    });
    let err = parse_describe_project(&v).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("no default database name"), "got: {s}");
    // Bail message must point at the freshly-created-project recovery.
    assert!(
        s.contains("freshly-created project") || s.contains("provisioned"),
        "missing provisioning recovery hint: {s}"
    );
}

#[test]
fn parse_describe_project_falls_back_to_databases_array() {
    let v = serde_json::json!({
        "project": {"id": "p", "name": "n"},
        "branches": [],
        "databases": [{"name": "alt-db", "branch_id": "br1"}]
    });
    let detail = parse_describe_project(&v).unwrap();
    assert_eq!(detail.default_database, "alt-db");
}

#[test]
fn parse_describe_project_accepts_is_default_alias_on_branch() {
    let v = serde_json::json!({
        "project": {"id": "p", "name": "n", "default_database_name": "appdb"},
        "branches": [{"id": "br1", "name": "main", "is_default": true}]
    });
    let detail = parse_describe_project(&v).unwrap();
    assert!(detail.branches[0].is_default);
}

#[test]
fn parse_describe_project_handles_branch_with_neither_default_nor_is_default() {
    let v = serde_json::json!({
        "project": {"id": "p", "name": "n", "default_database_name": "appdb"},
        "branches": [{"id": "br1", "name": "main"}]
    });
    let detail = parse_describe_project(&v).unwrap();
    assert_eq!(detail.branches.len(), 1);
    assert!(!detail.branches[0].is_default);
}

#[test]
fn parse_describe_project_errors_on_missing_project_field() {
    let v = serde_json::json!({"random": 42});
    let err = parse_describe_project(&v).unwrap_err();
    assert!(err.to_string().contains("project"), "got: {err}");
}

#[test]
fn parse_describe_project_propagates_is_error_envelope() {
    let v = serde_json::json!({
        "content": [{"type": "text", "text": "{}"}],
        "isError": true
    });
    let err = parse_describe_project(&v).unwrap_err();
    assert!(err.to_string().contains("isError=true"), "got: {err}");
}

// --- describe_branch --------------------------------------------------------

#[test]
fn parse_describe_branch_present_returns_true() {
    let v = parse_fixture(FIXTURE_DESCRIBE_BRANCH_PRESENT);
    assert_eq!(parse_describe_branch(&v).unwrap(), true);
}

#[test]
fn parse_describe_branch_missing_returns_false() {
    let v = parse_fixture(FIXTURE_DESCRIBE_BRANCH_MISSING);
    assert_eq!(parse_describe_branch(&v).unwrap(), false);
}

#[test]
fn parse_describe_branch_null_inner_bubbles_as_transport_error() {
    // cas-1549 autofix: a bare null inner is ambiguous — verify routes to
    // TransportError. Only an explicit {branch: null} is authoritative
    // not-found.
    let v = serde_json::json!({"content": [{"type": "text", "text": "null"}]});
    let err = parse_describe_branch(&v).unwrap_err();
    assert!(
        err.to_string().contains("empty/null payload"),
        "got: {err}"
    );
}

#[test]
fn parse_describe_branch_explicit_branch_null_returns_false() {
    let v = serde_json::json!({
        "content": [{"type": "text", "text": "{\"branch\": null}"}]
    });
    assert_eq!(parse_describe_branch(&v).unwrap(), false);
}

#[test]
fn parse_describe_branch_empty_content_array_bubbles() {
    // cas-1549 autofix: empty content array → unwrap returns Null →
    // caller bubbles (verify routes to TransportError, not Stale).
    let v = serde_json::json!({"content": [], "isError": false});
    let err = parse_describe_branch(&v).unwrap_err();
    assert!(
        err.to_string().contains("empty/null payload"),
        "expected transport-error bubble; got: {err}"
    );
}

#[test]
fn parse_describe_branch_project_not_found_does_not_misclassify_as_branch_missing() {
    // Tightened error sniff: an "org not found" / "project not found" reply
    // must NOT collapse to Ok(false). cas-fc38 contract — verify routes to
    // TransportError, never silent Stale.
    let v = serde_json::json!({
        "content": [{
            "type": "text",
            "text": "{\"error\":\"project proj-cool-bird not found\"}"
        }]
    });
    let err = parse_describe_branch(&v).unwrap_err();
    assert!(
        err.to_string().contains("non-branch-not-found"),
        "got: {err}"
    );
}

#[test]
fn parse_describe_branch_flat_form_requires_id_and_name() {
    let only_id = serde_json::json!({"id": "br1"});
    assert!(parse_describe_branch(&only_id).is_err());
    let wrapped_no_name = serde_json::json!({"branch": {"id": "br1"}});
    assert!(parse_describe_branch(&wrapped_no_name).is_err());
}

#[test]
fn parse_describe_branch_flat_object_returns_true() {
    let v = serde_json::json!({"id": "br1", "name": "main", "default": true});
    assert_eq!(parse_describe_branch(&v).unwrap(), true);
}

#[test]
fn parse_describe_branch_propagates_non_not_found_error() {
    // Transport-level error must propagate as Err, never silently become
    // Ok(false) — verify_at relies on this distinction to route to
    // IntegrationStatus::TransportError vs Stale.
    let v = serde_json::json!({
        "content": [{
            "type": "text",
            "text": "{\"error\":\"unauthorized: token expired\"}"
        }]
    });
    let err = parse_describe_branch(&v).unwrap_err();
    assert!(err.to_string().contains("unauthorized"), "got: {err}");
}

#[test]
fn parse_describe_branch_propagates_is_error_envelope() {
    let v = serde_json::json!({
        "content": [{"type": "text", "text": "{}"}],
        "isError": true
    });
    let err = parse_describe_branch(&v).unwrap_err();
    assert!(err.to_string().contains("isError=true"), "got: {err}");
}

#[test]
fn parse_describe_branch_malformed_response_errors() {
    let v = serde_json::json!({"random": 42});
    let err = parse_describe_branch(&v).unwrap_err();
    assert!(
        err.to_string().contains("neither `branch`"),
        "got: {err}"
    );
}
