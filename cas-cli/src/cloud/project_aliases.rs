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
/// cas-092e: the body lists personal projects (`team_id` null) beside team
/// projects, and the server resolves an identity inside one scope only
/// (petra-stella-cloud `resolveProjectIdentity`). So when matches span scopes,
/// the record in `team_id`'s scope wins, then the personal one; a legacy
/// personal `accounting` bucket no longer contradicts the team project that
/// folded `accounting` as an alias.
///
/// Returns `Ok(None)` when the account has no record for this project.
/// Returns `Err` when two different projects claim the identity inside the
/// chosen scope, because silently picking one would re-home rows onto the
/// wrong bucket.
pub fn select_alias_record(
    body: &serde_json::Value,
    project_id: &str,
    team_id: Option<&str>,
) -> Result<Option<ProjectAliasRecord>, CasError> {
    let Some(project_id) = canonical_project_id(project_id) else {
        return Ok(None);
    };
    let Some(projects) = body.get("projects").and_then(serde_json::Value::as_array) else {
        return Err(CasError::Other(
            "/api/account/projects response has no `projects` array".to_string(),
        ));
    };

    let mut matches: Vec<(Option<String>, ProjectAliasRecord)> = Vec::new();
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
            let scope = project
                .get("team_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            matches.push((scope, ProjectAliasRecord { canonical_id, aliases }));
        }
    }

    matches.sort_by(|a, b| (&a.1.canonical_id, &a.0).cmp(&(&b.1.canonical_id, &b.0)));
    matches.dedup();
    let distinct = |records: &[&(Option<String>, ProjectAliasRecord)]| {
        let mut ids: Vec<&str> = records.iter().map(|(_, record)| record.canonical_id.as_str()).collect();
        ids.dedup();
        ids.len()
    };
    let all: Vec<_> = matches.iter().collect();
    let team_scope: Vec<_> = all
        .iter()
        .copied()
        .filter(|(scope, _)| scope.is_some() && scope.as_deref() == team_id)
        .collect();
    let personal_scope: Vec<_> = all.iter().copied().filter(|(scope, _)| scope.is_none()).collect();
    let chosen = [team_scope, personal_scope]
        .into_iter()
        .find(|scope| !scope.is_empty())
        .unwrap_or(all);
    match distinct(&chosen) {
        0 => Ok(None),
        1 => Ok(chosen.first().map(|(_, record)| record.clone())),
        _ => {
            let in_scope = chosen;
            Err(CasError::Other(format!(
                "identity `{project_id}` is claimed by {} cloud projects ({}); refusing to \
                 attribute rows until the registry is disambiguated. Pin this checkout to the \
                 project that owns it with `cas cloud project set <canonical-id>`, or have the \
                 cloud owner retire the alias on the other project",
                distinct(&in_scope),
                in_scope
                    .iter()
                    .map(|(scope, record)| match scope {
                        Some(team) => format!("{} in team {team}", record.canonical_id),
                        None => format!("{} (personal)", record.canonical_id),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
    }
}

/// Fetch the record for `project_id` from `GET {endpoint}/api/account/projects`,
/// resolved in `team_id`'s scope (personal when `None`).
pub fn fetch_project_alias_record(
    endpoint: &str,
    token: &str,
    project_id: &str,
    team_id: Option<&str>,
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
    select_alias_record(&body, project_id, team_id)
}

fn union_project_aliases(local: &[String], server: &[String]) -> Vec<String> {
    let mut aliases = local
        .iter()
        .chain(server)
        .filter_map(|alias| canonical_project_id(alias))
        .collect::<Vec<_>>();
    aliases.sort();
    aliases.dedup();
    aliases
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
    team_id: Option<&str>,
    timeout: Duration,
) -> Result<Vec<String>, CasError> {
    match fetch_project_alias_record(endpoint, token, project_id, team_id, timeout)? {
        Some(record) => {
            let local = crate::cloud::config::project_aliases_from_config_toml(cas_root);
            set_project_aliases_in_config_toml(
                cas_root,
                &union_project_aliases(&local, &record.aliases),
            )
        }
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
                { "canonical_id": "github.com/richards-llc/violet", "aliases": [] }
            ]
        })
    }

    #[test]
    fn selects_the_record_by_canonical_id() {
        let record = select_alias_record(&account_projects(), "ozer", None)
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
            None,
        )
        .unwrap()
        .expect("the remote spelling is a registered alias");
        assert_eq!(record.canonical_id, "ozer");
    }

    #[test]
    fn a_project_with_no_registered_aliases_reports_an_empty_list_not_a_guess() {
        let record = select_alias_record(&account_projects(), "cas-src", None)
            .unwrap()
            .expect("cas-src is registered");
        assert!(record.aliases.is_empty());
    }

    /// Personal-scope projects are absent from this endpoint. `None` must mean
    /// "no record", never "no aliases, go ahead and rewrite".
    #[test]
    fn an_unknown_project_yields_no_record() {
        assert_eq!(
            select_alias_record(&account_projects(), "gabber-studio", None).unwrap(),
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
        let error = select_alias_record(&body, "shared", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("claimed by 2 cloud projects"), "{error}");
        assert!(error.contains("one (personal), two (personal)") && error.contains("cas cloud project set"), "{error}");
    }

    /// cas-092e: the live registry lists a legacy personal `accounting` bucket
    /// beside team project `richards-llc-accounting`, which folded
    /// `accounting` as an alias. Resolution stays inside the sync's scope.
    #[test]
    fn an_identity_claimed_in_two_scopes_resolves_in_the_sync_scope_cas_092e() {
        let body = serde_json::json!({
            "projects": [
                { "team_id": null, "canonical_id": "accounting", "aliases": ["accounting"] },
                {
                    "team_id": "team-1",
                    "canonical_id": "richards-llc-accounting",
                    "aliases": ["accounting", "petra-stella-accounting", "richards-llc-accounting"]
                }
            ]
        });
        let team = select_alias_record(&body, "accounting", Some("team-1")).unwrap().unwrap();
        assert_eq!(team.canonical_id, "richards-llc-accounting");
        assert_eq!(team.aliases, vec!["accounting", "petra-stella-accounting"]);
        let personal = select_alias_record(&body, "accounting", None).unwrap().unwrap();
        assert_eq!(personal.canonical_id, "accounting");
    }

    #[test]
    fn a_malformed_body_is_an_error_rather_than_an_empty_record() {
        assert!(select_alias_record(&serde_json::json!({}), "cas-src", None).is_err());
    }

    #[test]
    fn adopting_server_aliases_unions_the_local_cache() {
        let local = vec!["github.com/richards-llc/cassy".to_string(), "old".to_string()];
        let server = vec!["old".to_string(), "server-spelling".to_string()];
        assert_eq!(
            union_project_aliases(&local, &server),
            vec![
                "github.com/richards-llc/cassy".to_string(),
                "old".to_string(),
                "server-spelling".to_string()
            ]
        );
    }
}
