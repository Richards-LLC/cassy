//! Consume the cloud's per-project `aliases` record (GH #669).
//!
//! # Why the client needs it
//!
//! The server's alias-merge migration (petra-stella-cloud PR #60, recovered by
//! #61/#62) folded legacy spellings of one project — `ozer-health` and
//! `github.com/richards-llc/ozer-health` under `ozer`, and the same for the
//! `gabber-studio` / `pixel-hive` / `petra-stella-cloud` families — into a
//! single canonical bucket, and recorded every folded spelling in a per-project
//! `aliases` list.
//!
//! No normalizer can reproduce that fold: `ozer-health` and `ozer` are two
//! different strings under every rule both sides implement, and deliberately so
//! (the server refuses to infer `accounting` from
//! `gitlab.example/owner/accounting.git`, precisely to stop same-name
//! repositories on different hosts from merging). The fold is registry data.
//!
//! So a client that never reads the record keeps counting alias-scoped rows as
//! belonging to a *foreign* project: `cas doctor` reports them, the pull ingest
//! guard skips them, and `cas cloud purge-foreign` offers to delete rows that
//! are actually this project's own history.
//!
//! # Where the record comes from
//!
//! `GET /api/account/projects` returns, per project the caller can see:
//! `{ id, team_id, team_slug, team_name, canonical_id, name, created_by,
//! created_at, aliases: string[], contributor_count, memory_count }`.
//! `aliases` is a flat array of already-canonicalized strings, excluding
//! retired bindings.
//!
//! # Known gap, deliberately not papered over
//!
//! That endpoint (and `GET /api/teams/:teamId/projects`) exposes **team**
//! `project_aliases` only. The server's `personal_project_aliases` table has no
//! API exposure as of `ee422ce`, and the `gabber-studio` / `pixel-hive` /
//! `petra-stella-cloud` families were migrated in personal scope. For those,
//! [`fetch_project_alias_record`] correctly returns no aliases rather than
//! guessing — the fix is a server endpoint, and the client will pick the rows
//! up unchanged once one exists.

use std::path::Path;
use std::time::Duration;

use crate::cloud::config::{canonical_project_id, set_project_aliases_in_config_toml};
use crate::error::CasError;

/// One project's identity record as the cloud reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectAliasRecord {
    /// Server-side canonical id, canonicalized client-side as well so a drift
    /// in either normalizer shows up as a mismatch instead of a silent fork.
    pub canonical_id: String,
    /// Active alias spellings, canonicalized and sorted; never contains
    /// `canonical_id`.
    pub aliases: Vec<String>,
}

/// Pick the record for `project_id` out of a `GET /api/account/projects` body.
///
/// A project matches when `project_id` canonicalizes to its `canonical_id`
/// **or** to one of its aliases — the second case is exactly the machine that
/// is still pinned to a legacy spelling after the server merged it.
///
/// Returns `Ok(None)` when the account has no record for this project (not an
/// error: personal-scope projects are simply not exposed by this endpoint).
/// Returns `Err` when two different projects claim the same identity, because
/// silently picking one would re-home rows onto the wrong bucket.
pub fn select_alias_record(
    body: &serde_json::Value,
    project_id: &str,
) -> Result<Option<ProjectAliasRecord>, CasError> {
    let Some(project_id) = canonical_project_id(project_id) else {
        return Ok(None);
    };
    let Some(projects) = body.get("projects").and_then(serde_json::Value::as_array) else {
        return Err(CasError::Other(
            "/api/account/projects response has no `projects` array".to_string(),
        ));
    };

    let mut matches: Vec<ProjectAliasRecord> = Vec::new();
    for project in projects {
        let Some(canonical_id) = project
            .get("canonical_id")
            .and_then(serde_json::Value::as_str)
            .and_then(canonical_project_id)
        else {
            continue;
        };
        let mut aliases: Vec<String> = project
            .get("aliases")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter_map(canonical_project_id)
                    .filter(|alias| *alias != canonical_id)
                    .collect()
            })
            .unwrap_or_default();
        aliases.sort();
        aliases.dedup();

        if canonical_id == project_id || aliases.iter().any(|alias| *alias == project_id) {
            matches.push(ProjectAliasRecord {
                canonical_id,
                aliases,
            });
        }
    }

    matches.sort_by(|a, b| a.canonical_id.cmp(&b.canonical_id));
    matches.dedup();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => Err(CasError::Other(format!(
            "identity `{project_id}` is claimed by {} cloud projects ({}); refusing to \
             attribute rows until the registry is disambiguated",
            matches.len(),
            matches
                .iter()
                .map(|m| m.canonical_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Fetch the record for `project_id` from `GET {endpoint}/api/account/projects`.
pub fn fetch_project_alias_record(
    endpoint: &str,
    token: &str,
    project_id: &str,
    timeout: Duration,
) -> Result<Option<ProjectAliasRecord>, CasError> {
    let url = format!("{}/api/account/projects", endpoint.trim_end_matches('/'));
    let body: serde_json::Value = ureq::get(&url)
        .timeout(timeout)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| CasError::Other(format!("/api/account/projects failed: {e}")))?
        .into_json()
        .map_err(|e| CasError::Other(format!("/api/account/projects is not JSON: {e}")))?;
    select_alias_record(&body, project_id)
}

/// Best-effort refresh of `<cas_root>/.cas/config.toml [project] aliases` from
/// the cloud, run on pull.
///
/// Returns the aliases now cached locally. Every failure — offline, 401, an
/// ambiguous registry — is returned as `Err` for the caller to log; a pull must
/// never fail because the identity record could not be refreshed, and the
/// previously cached record stays in place when it cannot.
pub fn refresh_project_alias_record(
    cas_root: &Path,
    endpoint: &str,
    token: &str,
    project_id: &str,
    timeout: Duration,
) -> Result<Vec<String>, CasError> {
    match fetch_project_alias_record(endpoint, token, project_id, timeout)? {
        Some(record) => set_project_aliases_in_config_toml(cas_root, &record.aliases),
        // No record for this project: leave whatever is cached alone rather
        // than erasing it on an endpoint that does not cover personal scope.
        None => Ok(crate::cloud::config::project_aliases_from_config_toml(
            cas_root,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shape of the live `GET /api/account/projects` response
    /// (2026-09-03, after the alias-merge migration).
    fn account_projects() -> serde_json::Value {
        serde_json::json!({
            "projects": [
                { "canonical_id": "cas-src", "aliases": [] },
                {
                    "canonical_id": "ozer",
                    "aliases": ["github.com/richards-llc/ozer-health", "ozer-health"]
                },
                { "canonical_id": "penguinz", "aliases": [] },
                { "canonical_id": "github.com/richards-llc/mecha-cassy", "aliases": [] }
            ]
        })
    }

    #[test]
    fn selects_the_record_by_canonical_id() {
        let record = select_alias_record(&account_projects(), "ozer")
            .unwrap()
            .expect("ozer is registered");
        assert_eq!(record.canonical_id, "ozer");
        assert_eq!(
            record.aliases,
            vec!["github.com/richards-llc/ozer-health", "ozer-health"]
        );
    }

    /// The machine that still calls itself `ozer-health` is the whole reason
    /// the record exists — it has to find its own project through an alias.
    #[test]
    fn selects_the_record_through_a_legacy_alias_spelling() {
        let record = select_alias_record(
            &account_projects(),
            "git@GitHub.com:Richards-LLC/Ozer-Health.git",
        )
        .unwrap()
        .expect("the remote spelling is a registered alias");
        assert_eq!(record.canonical_id, "ozer");
    }

    #[test]
    fn a_project_with_no_registered_aliases_reports_an_empty_list_not_a_guess() {
        let record = select_alias_record(&account_projects(), "cas-src")
            .unwrap()
            .expect("cas-src is registered");
        assert!(record.aliases.is_empty());
    }

    /// Personal-scope projects are absent from this endpoint. `None` must mean
    /// "no record", never "no aliases, go ahead and rewrite".
    #[test]
    fn an_unknown_project_yields_no_record() {
        assert_eq!(
            select_alias_record(&account_projects(), "gabber-studio").unwrap(),
            None
        );
    }

    #[test]
    fn two_projects_claiming_one_identity_are_refused() {
        let body = serde_json::json!({
            "projects": [
                { "canonical_id": "one", "aliases": ["shared"] },
                { "canonical_id": "two", "aliases": ["shared"] }
            ]
        });
        let error = select_alias_record(&body, "shared")
            .unwrap_err()
            .to_string();
        assert!(error.contains("claimed by 2 cloud projects"), "{error}");
    }

    #[test]
    fn a_malformed_body_is_an_error_rather_than_an_empty_record() {
        assert!(select_alias_record(&serde_json::json!({}), "cas-src").is_err());
    }
}
