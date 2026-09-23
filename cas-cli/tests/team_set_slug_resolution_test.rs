//! Integration tests for eager project-slug resolution at `cas cloud team set`
//! (task cas-1ced, EPIC cas-ffc4 — hypothesis #3 of the new-team-member bug
//! doc).
//!
//! Before this fix, `cas cloud team set <uuid>` printed
//! "Slug resolution deferred — see `cas cloud team show`" and did NOT resolve
//! the project canonical id eagerly. A new member who cloned the team's repo
//! into a directory whose basename didn't match the canonical slug (e.g.
//! clone-named `cas` vs canonical `cas-src`) had their first
//! `cas cloud sync` go out with `project_id=cas` — the wrong scope — silently
//! routing push/pull to a phantom project. The recorded workaround was a
//! manual directory rename.
//!
//! Resolution order added by this task:
//!  a. `.cas/config.toml [project] canonical_id = "..."` — source of truth.
//!  b. `git -C <cas_root> remote get-url origin`, normalized to
//!     `<host>/<owner>/<repo>` form (strips protocol/SSH prefix + `.git`).
//!  c. If neither resolves, keep the "Slug resolution deferred" message —
//!     do NOT default to the working-directory basename.
//!
//! Also covered: the new `cas cloud project set <canonical-id>` subcommand
//! (manual override for monorepo / non-git / custom layout cases) and
//! `cas cloud team show` displaying the resolved slug.
//!
//! Tests reuse `CasRootGuard` + static `ENV_LOCK` from
//! `team_pull_wiring_test.rs`'s pattern — both files mutate the process-wide
//! `CAS_ROOT` env var so a single Mutex serializes them. We re-declare it
//! locally because each `tests/*.rs` file compiles as its own binary; the
//! Mutex is per-process and there's no cross-binary state to share.

use std::path::Path;
use std::sync::Mutex;

mod common;
use common::{TEST_TEAM, make_cli_json};

use cas::cli::cloud::{
    CloudProjectCommands, CloudProjectSetArgs, CloudTeamCommands, CloudTeamSetArgs,
    execute_project, execute_team, execute_team_show_for_test,
};
use cas::cloud::{CloudConfig, TeamInfo};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Process-global lock for CAS_ROOT mutations. Per-binary (separate from the
/// one in `team_pull_wiring_test.rs` — each test binary has its own
/// process).
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct CasRootGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
    prev_user_cloud_json: Option<std::ffi::OsString>,
}

impl CasRootGuard {
    fn set(cas_root: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("CAS_ROOT");
        let prev_user_cloud_json = std::env::var_os("CAS_USER_CLOUD_JSON");
        // SAFETY: env mutation on an integration-test process, guarded by
        // ENV_LOCK so no other test can race the var concurrently.
        unsafe { std::env::set_var("CAS_ROOT", cas_root) };
        Self {
            _lock: lock,
            prev,
            prev_user_cloud_json,
        }
    }

    fn set_with_user_cloud_json(cas_root: &Path, user_cloud_json: &Path) -> Self {
        let guard = Self::set(cas_root);
        // SAFETY: env mutation on an integration-test process, guarded by
        // ENV_LOCK held by `guard`.
        unsafe { std::env::set_var("CAS_USER_CLOUD_JSON", user_cloud_json) };
        guard
    }
}

impl Drop for CasRootGuard {
    fn drop(&mut self) {
        // SAFETY: same as `set` — ENV_LOCK held for entire guard lifetime.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("CAS_ROOT", v),
                None => std::env::remove_var("CAS_ROOT"),
            }
            match &self.prev_user_cloud_json {
                Some(v) => std::env::set_var("CAS_USER_CLOUD_JSON", v),
                None => std::env::remove_var("CAS_USER_CLOUD_JSON"),
            }
        }
    }
}

/// Initialize a temp dir as a CAS root with cloud.json seeded for `endpoint`
/// (so `CloudConfig::load()` inside the handler finds a valid token). The
/// returned TempDir owns the files and must outlive the test.
fn seed_cas_root(endpoint: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let cas_dir = tmp.path();
    let mut cfg = CloudConfig::default();
    cfg.endpoint = endpoint.to_string();
    cfg.token = Some("test-token".to_string());
    cfg.save_to_cas_dir(cas_dir).unwrap();
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

fn seed_user_cloud_json(teams: Vec<TeamInfo>) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let mut cfg = CloudConfig::default();
    cfg.teams = teams;
    cfg.save_to_cas_dir(tmp.path()).unwrap();
    tmp
}

/// Mount the team-membership probe endpoint with a 200 (member). The probe
/// is the gating HTTP call inside `execute_team_set`; without it, the handler
/// short-circuits before any slug-resolution work runs.
async fn mount_membership_ok(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/projects")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"projects": []})))
        .mount(server)
        .await;
}

/// Initialize `cas_root` as a git repo with `origin = <origin_url>`. Uses
/// `git init` + `git remote add` directly so the test doesn't depend on
/// `gitoxide` or anything beyond a system `git`.
fn init_git_repo_with_origin(cas_root: &Path, origin_url: &str) {
    use std::process::Command;
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(cas_root)
        .status()
        .expect("git init");
    Command::new("git")
        .args(["remote", "add", "origin", origin_url])
        .current_dir(cas_root)
        .status()
        .expect("git remote add");
}

/// AC: `team set` with `.cas/config.toml [project] canonical_id` already set
/// leaves it intact (config.toml is the source of truth, NEVER overwritten
/// by derivation).
#[tokio::test]
async fn team_set_preserves_existing_config_toml_canonical_id() {
    let server = MockServer::start().await;
    mount_membership_ok(&server).await;

    let tmp = seed_cas_root(&server.uri());
    let cas_root = tmp.path().to_path_buf();

    // Seed config.toml with a pre-existing canonical_id.
    std::fs::write(
        cas_root.join("config.toml"),
        "[project]\ncanonical_id = \"github.com/teamco/already-set\"\n",
    )
    .unwrap();

    let _env = CasRootGuard::set(&cas_root);

    let args = CloudTeamSetArgs {
        id: Some(TEST_TEAM.to_string()),
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || {
        execute_team(&CloudTeamCommands::Set(args), &cli, &cas_root_owned)
            .expect("team set must succeed");
    })
    .await
    .unwrap();

    // Config.toml must still hold the original value — NOT overwritten.
    let toml = std::fs::read_to_string(cas_root.join("config.toml")).unwrap();
    assert!(
        toml.contains("github.com/teamco/already-set"),
        "existing canonical_id must be preserved, got:\n{toml}"
    );
}

/// AC: `team set <slug>` resolves the slug from user-level cached teams[]
/// before running the existing membership probe, then writes the resolved UUID
/// and slug to the per-project cloud.json.
#[tokio::test]
async fn team_set_resolves_slug_from_cached_memberships() {
    let server = MockServer::start().await;
    mount_membership_ok(&server).await;

    let tmp = seed_cas_root(&server.uri());
    let cas_root = tmp.path().to_path_buf();
    let user = seed_user_cloud_json(vec![make_team(TEST_TEAM, "petra-stella", "Petra Stella")]);
    let user_cloud_json = user.path().join("cloud.json");

    let _env = CasRootGuard::set_with_user_cloud_json(&cas_root, &user_cloud_json);

    let args = CloudTeamSetArgs {
        id: Some("petra-stella".to_string()),
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || {
        execute_team(&CloudTeamCommands::Set(args), &cli, &cas_root_owned)
            .expect("team set by slug must succeed");
    })
    .await
    .unwrap();

    let loaded = CloudConfig::load_from_cas_dir(&cas_root).unwrap();
    assert_eq!(loaded.team_id.as_deref(), Some(TEST_TEAM));
    assert_eq!(loaded.team_slug.as_deref(), Some("petra-stella"));
}

/// AC: `team set` with a git origin but no config.toml derives the slug from
/// the git remote, writes it to `.cas/config.toml`, and the value matches the
/// normalized `<host>/<owner>/<repo>` form.
#[tokio::test]
async fn team_set_derives_canonical_id_from_https_git_remote() {
    let server = MockServer::start().await;
    mount_membership_ok(&server).await;

    let tmp = seed_cas_root(&server.uri());
    let cas_root = tmp.path().to_path_buf();
    init_git_repo_with_origin(&cas_root, "https://github.com/foo/bar.git");

    let _env = CasRootGuard::set(&cas_root);

    let args = CloudTeamSetArgs {
        id: Some(TEST_TEAM.to_string()),
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || {
        execute_team(&CloudTeamCommands::Set(args), &cli, &cas_root_owned)
            .expect("team set must succeed");
    })
    .await
    .unwrap();

    let toml = std::fs::read_to_string(cas_root.join("config.toml"))
        .expect("team_set should have written config.toml after deriving from git remote");
    assert!(
        toml.contains("github.com/foo/bar"),
        "config.toml must contain the derived canonical_id, got:\n{toml}"
    );
    // Negative invariant: must NOT contain the `.git` suffix or `https://` prefix.
    assert!(
        !toml.contains(".git"),
        ".git suffix must be stripped from the derived canonical_id"
    );
    assert!(
        !toml.contains("https://"),
        "https:// prefix must be stripped from the derived canonical_id"
    );
}

/// SSH-form git remotes (`git@host:owner/repo.git`) must normalize the same
/// way as the HTTPS form. This locks in the SSH parse path.
#[tokio::test]
async fn team_set_derives_canonical_id_from_ssh_git_remote() {
    let server = MockServer::start().await;
    mount_membership_ok(&server).await;

    let tmp = seed_cas_root(&server.uri());
    let cas_root = tmp.path().to_path_buf();
    init_git_repo_with_origin(&cas_root, "git@github.com:foo/bar.git");

    let _env = CasRootGuard::set(&cas_root);

    let args = CloudTeamSetArgs {
        id: Some(TEST_TEAM.to_string()),
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || {
        execute_team(&CloudTeamCommands::Set(args), &cli, &cas_root_owned)
            .expect("team set must succeed");
    })
    .await
    .unwrap();

    let toml = std::fs::read_to_string(cas_root.join("config.toml")).unwrap();
    assert!(
        toml.contains("github.com/foo/bar"),
        "SSH-form remote must normalize to host/owner/repo, got:\n{toml}"
    );
}

/// AC negative: `team set` in a non-git directory without config.toml must
/// NOT silently default to the working-directory basename. config.toml
/// either stays absent OR contains no `canonical_id` line — definitely not
/// the tempdir's random basename.
#[tokio::test]
async fn team_set_does_not_default_to_basename_when_neither_source_resolves() {
    let server = MockServer::start().await;
    mount_membership_ok(&server).await;

    let tmp = seed_cas_root(&server.uri());
    let cas_root = tmp.path().to_path_buf();

    let basename = cas_root.file_name().unwrap().to_string_lossy().to_string();

    let _env = CasRootGuard::set(&cas_root);

    let args = CloudTeamSetArgs {
        id: Some(TEST_TEAM.to_string()),
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || {
        execute_team(&CloudTeamCommands::Set(args), &cli, &cas_root_owned)
            .expect("team set must succeed (deferred output is not an error)");
    })
    .await
    .unwrap();

    // config.toml is allowed to be absent OR present without a canonical_id
    // line. The load-bearing assertion is the NEGATIVE: tempdir basename
    // must NOT have leaked into the file.
    if let Ok(toml) = std::fs::read_to_string(cas_root.join("config.toml")) {
        assert!(
            !toml.contains(&basename),
            "team set must NOT default to working-directory basename ({basename}); \
             config.toml:\n{toml}"
        );
    }
}

/// AC: `cas cloud project set <canonical-id>` writes `[project] canonical_id`
/// to `.cas/config.toml`. Manual override path for monorepos / non-git
/// directories / cases where derivation fails.
#[tokio::test]
async fn project_set_writes_canonical_id_to_config_toml() {
    let tmp = TempDir::new().unwrap();
    let cas_root = tmp.path().to_path_buf();
    let _env = CasRootGuard::set(&cas_root);

    let args = CloudProjectSetArgs {
        canonical_id: "github.com/foo/bar".to_string(),
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || {
        execute_project(&CloudProjectCommands::Set(args), &cli, &cas_root_owned)
            .expect("project set must succeed");
    })
    .await
    .unwrap();

    let toml = std::fs::read_to_string(cas_root.join("config.toml"))
        .expect("project set must create config.toml");
    assert!(
        toml.contains("[project]"),
        "config.toml must contain [project] block, got:\n{toml}"
    );
    assert!(
        toml.contains("canonical_id = \"github.com/foo/bar\"")
            || toml.contains("canonical_id=\"github.com/foo/bar\""),
        "config.toml must contain canonical_id=github.com/foo/bar, got:\n{toml}"
    );
}

/// AC: `cas cloud team show` displays the resolved project slug alongside
/// the team UUID. Mode = JSON to keep parsing deterministic.
#[tokio::test]
async fn team_show_displays_resolved_project_slug() {
    let server = MockServer::start().await;
    let tmp = seed_cas_root(&server.uri());
    let cas_root = tmp.path().to_path_buf();
    std::fs::write(
        cas_root.join("config.toml"),
        "[project]\ncanonical_id = \"github.com/showco/showrepo\"\n",
    )
    .unwrap();

    // Seed cloud.json with a team_id so team_show has something to show.
    let mut cfg = CloudConfig::load_from_cas_dir(&cas_root).unwrap();
    cfg.team_id = Some(TEST_TEAM.to_string());
    cfg.save_to_cas_dir(&cas_root).unwrap();

    let _env = CasRootGuard::set(&cas_root);

    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    let output = tokio::task::spawn_blocking(move || {
        execute_team_show_for_test(&cli, &cas_root_owned).expect("team show must succeed")
    })
    .await
    .unwrap();

    // `execute_team_show_for_test` returns the JSON string it would have
    // printed — this is the testable seam matching the pattern used in
    // adjacent integration tests. Assert it carries the resolved slug.
    assert!(
        output.contains("github.com/showco/showrepo"),
        "team show output must include resolved project slug, got:\n{output}"
    );
    assert!(
        output.contains(TEST_TEAM),
        "team show output must include team UUID, got:\n{output}"
    );
}

/// cas-f07a AC2: `cas cloud team show` must never print `<not resolved>` for
/// an active project when config.toml is absent. The full resolution chain
/// falls through to the folder name (parent-dir basename), so the output
/// must contain a non-null `canonical_id`.
///
/// Before the fix, `team_show_json` called `canonical_id_from_config_toml`
/// which returns `None` when config.toml is absent, causing the CLI to
/// display `<not resolved>`. After the fix it calls `resolve_canonical_id`
/// which falls through to the folder-name step and returns the tempdir name.
#[tokio::test]
async fn team_show_never_shows_not_resolved_for_active_project() {
    let server = MockServer::start().await;
    let tmp = seed_cas_root(&server.uri());
    let cas_root = tmp.path().to_path_buf();

    // Derive the expected canonical_id.  `canonical_id_from_cas_root` treats
    // `cas_root` as the `.cas/` directory, so it returns the *parent*'s
    // file_name (the project root).  In tests, `cas_root` is the TempDir
    // root itself (`/tmp/.tmpXXXX`), so the resolved id is
    // `cas_root.parent().file_name()` — typically `tmp` on Linux.  The
    // exact value doesn't matter; what matters is that it is non-null and is
    // NOT the sentinel `<not resolved>`.
    let expected_folder = cas_root
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| cas_root.to_string_lossy().to_string());
    let expected_id = cas::cloud::canonical_project_id(&expected_folder).unwrap();

    // Seed cloud.json with a team_id but DO NOT write config.toml —
    // that is the condition under which the old code showed <not resolved>.
    let mut cfg = cas::cloud::CloudConfig::load_from_cas_dir(&cas_root).unwrap();
    cfg.team_id = Some(TEST_TEAM.to_string());
    cfg.save_to_cas_dir(&cas_root).unwrap();

    let _env = CasRootGuard::set(&cas_root);

    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    let output = tokio::task::spawn_blocking(move || {
        execute_team_show_for_test(&cli, &cas_root_owned).expect("team show must succeed")
    })
    .await
    .unwrap();

    // The output must carry the resolved canonical_id (the tempdir name).
    assert!(
        output.contains(&expected_id),
        "team show must show resolved canonical_id (folder name) when config.toml absent, \
         expected '{}', got:\n{output}",
        expected_id
    );
    // Negative invariant: the sentinel string must NOT appear.
    assert!(
        !output.contains("not resolved"),
        "team show must NOT show 'not resolved' for an active project, got:\n{output}"
    );
}
