//! Integration tests for `cas cloud team default` subcommand (cas-6804).
//!
//! Verifies:
//! - Setting default by slug resolves against cached teams[] and writes
//!   `default_team_id` to the injected user_cas_dir.
//! - Setting default by UUID works the same way.
//! - `--personal` clears `default_team_id`.
//! - Clear error when slug/uuid doesn't match any cached team.
//! - Helpful error when teams[] is empty (not yet refreshed via login).
//!
//! Tests use the injected `user_cas_dir` path (via
//! `execute_team_default_for_test`) to avoid touching the real
//! `~/.cas/cloud.json` — same pattern as `team_set_slug_resolution_test.rs`.

use cas::cli::cloud::{CloudTeamDefaultArgs, execute_team_default_for_test};
use cas::cloud::{CloudConfig, TeamInfo};
use tempfile::TempDir;

#[path = "common/mod.rs"]
mod common;
use common::make_cli_json;

/// Build a user-level cas dir (tempdir) seeded with the given teams and
/// optional default_team_id. Returns the TempDir (caller must keep it alive).
fn seed_user_cas_dir(teams: Vec<TeamInfo>, default_team_id: Option<String>) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let mut cfg = CloudConfig::default();
    cfg.token = Some("test-token".to_string());
    cfg.teams = teams;
    cfg.default_team_id = default_team_id;
    cfg.save_to_cas_dir(dir).unwrap();
    tmp
}

fn make_team(id: &str, slug: &str, name: &str) -> TeamInfo {
    TeamInfo {
        id: id.to_string(),
        slug: slug.to_string(),
        name: name.to_string(),
        role: "member".to_string(),
    }
}

// ── Happy-path: set by slug ──────────────────────────────────────────────────

/// AC: `cas cloud team default <slug>` resolves slug against cached teams[]
/// and writes `default_team_id` (the UUID) to user-level cloud.json.
#[test]
fn team_default_by_slug_writes_team_id() {
    let teams = vec![
        make_team("tid-alpha", "alpha-team", "Alpha Team"),
        make_team("tid-beta", "beta-team", "Beta Team"),
    ];
    let tmp = seed_user_cas_dir(teams, None);
    let dir = tmp.path().to_path_buf();

    let args = CloudTeamDefaultArgs {
        slug_or_uuid: Some("alpha-team".to_string()),
        personal: false,
    };
    let cli = make_cli_json();

    let result = execute_team_default_for_test(&args, &cli, &dir).unwrap();
    assert_eq!(
        result["default_team_id"].as_str(),
        Some("tid-alpha"),
        "default_team_id must be the UUID of the matched team, not the slug"
    );

    // Verify it persisted to disk.
    let loaded = CloudConfig::load_from_cas_dir(&dir).unwrap();
    assert_eq!(loaded.default_team_id, Some("tid-alpha".to_string()));
}

// ── Happy-path: set by UUID ──────────────────────────────────────────────────

/// AC: `cas cloud team default <uuid>` resolves against teams[].id and writes
/// `default_team_id` to user-level cloud.json.
#[test]
fn team_default_by_uuid_writes_team_id() {
    let teams = vec![make_team("tid-gamma", "gamma-team", "Gamma Team")];
    let tmp = seed_user_cas_dir(teams, None);
    let dir = tmp.path().to_path_buf();

    let args = CloudTeamDefaultArgs {
        slug_or_uuid: Some("tid-gamma".to_string()),
        personal: false,
    };
    let cli = make_cli_json();

    let result = execute_team_default_for_test(&args, &cli, &dir).unwrap();
    assert_eq!(result["default_team_id"].as_str(), Some("tid-gamma"));

    let loaded = CloudConfig::load_from_cas_dir(&dir).unwrap();
    assert_eq!(loaded.default_team_id, Some("tid-gamma".to_string()));
}

// ── Happy-path: --personal clears default ───────────────────────────────────

/// AC: `cas cloud team default --personal` clears `default_team_id` to None.
#[test]
fn team_default_personal_clears_default_team_id() {
    let teams = vec![make_team("tid-delta", "delta-team", "Delta Team")];
    let tmp = seed_user_cas_dir(teams, Some("tid-delta".to_string()));
    let dir = tmp.path().to_path_buf();

    // Precondition: default_team_id is set.
    let pre = CloudConfig::load_from_cas_dir(&dir).unwrap();
    assert_eq!(pre.default_team_id, Some("tid-delta".to_string()));

    let args = CloudTeamDefaultArgs {
        slug_or_uuid: None,
        personal: true,
    };
    let cli = make_cli_json();
    let result = execute_team_default_for_test(&args, &cli, &dir).unwrap();
    assert!(
        result["default_team_id"].is_null(),
        "default_team_id must be null after --personal, got: {}",
        result
    );

    let loaded = CloudConfig::load_from_cas_dir(&dir).unwrap();
    assert!(
        loaded.default_team_id.is_none(),
        "default_team_id must be None on disk after --personal"
    );
}

/// AC: `cas cloud team default --personal` is a no-op (not an error) when no
/// default was set.
#[test]
fn team_default_personal_is_noop_when_not_set() {
    let tmp = seed_user_cas_dir(vec![], None);
    let dir = tmp.path().to_path_buf();

    let args = CloudTeamDefaultArgs {
        slug_or_uuid: None,
        personal: true,
    };
    let cli = make_cli_json();
    let result = execute_team_default_for_test(&args, &cli, &dir);
    assert!(result.is_ok(), "--personal on unset config must not error");
    let loaded = CloudConfig::load_from_cas_dir(&dir).unwrap();
    assert!(loaded.default_team_id.is_none());
}

// ── Error path: slug/uuid not found in teams[] ───────────────────────────────

/// AC: clear error when the slug/UUID is not in the cached teams[].
#[test]
fn team_default_errors_when_slug_not_found() {
    let teams = vec![make_team("tid-eta", "eta-team", "Eta Team")];
    let tmp = seed_user_cas_dir(teams, None);
    let dir = tmp.path().to_path_buf();

    let args = CloudTeamDefaultArgs {
        slug_or_uuid: Some("nonexistent-team".to_string()),
        personal: false,
    };
    let cli = make_cli_json();
    let err = execute_team_default_for_test(&args, &cli, &dir)
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("nonexistent-team"),
        "error must echo the unknown query, got: {err}"
    );
}

/// AC: when teams[] is empty, error message tells the user to run
/// `cas cloud login` to refresh team membership.
#[test]
fn team_default_errors_with_login_hint_when_teams_empty() {
    let tmp = seed_user_cas_dir(vec![], None);
    let dir = tmp.path().to_path_buf();

    let args = CloudTeamDefaultArgs {
        slug_or_uuid: Some("my-team".to_string()),
        personal: false,
    };
    let cli = make_cli_json();
    let err = execute_team_default_for_test(&args, &cli, &dir)
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("cas cloud login"),
        "error must suggest running `cas cloud login` when teams[] is empty, got: {err}"
    );
}

// ── Persistence invariants ───────────────────────────────────────────────────

/// Existing `team_id` (per-project field) in the same cloud.json must not
/// be disturbed when `default_team_id` is written — the two fields are
/// independent.
#[test]
fn team_default_does_not_overwrite_existing_team_id() {
    let teams = vec![make_team("tid-zeta", "zeta-team", "Zeta Team")];
    let tmp = seed_user_cas_dir(teams, None);
    let dir = tmp.path().to_path_buf();

    // Pre-seed a team_id (per-project override field).
    let mut pre = CloudConfig::load_from_cas_dir(&dir).unwrap();
    pre.team_id = Some("original-project-team-id".to_string());
    pre.save_to_cas_dir(&dir).unwrap();

    let args = CloudTeamDefaultArgs {
        slug_or_uuid: Some("zeta-team".to_string()),
        personal: false,
    };
    let cli = make_cli_json();
    execute_team_default_for_test(&args, &cli, &dir).unwrap();

    let loaded = CloudConfig::load_from_cas_dir(&dir).unwrap();
    assert_eq!(
        loaded.team_id,
        Some("original-project-team-id".to_string()),
        "existing team_id (per-project) must be preserved"
    );
    assert_eq!(
        loaded.default_team_id,
        Some("tid-zeta".to_string()),
        "default_team_id must be written"
    );
}
