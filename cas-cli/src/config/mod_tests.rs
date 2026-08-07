use crate::config::*;
use crate::test_support::TestEnvGuard;
use crate::ui::theme::{ThemeConfig, ThemeMode, ThemeVariant};
use tempfile::TempDir;

#[test]
fn test_config_defaults() {
    let config = Config::default();
    assert!(config.sync.enabled);
    assert_eq!(config.sync.target, ".claude/rules/cas");
    assert_eq!(config.sync.min_helpful, 1);
}

#[test]
fn test_config_save_load() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();
    config.sync.min_helpful = 5;

    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();

    assert_eq!(loaded.sync.min_helpful, 5);
}

#[test]
fn test_merge_missing_fills_none_fields() {
    let mut base = Config::default();
    assert!(base.theme.is_none());

    let mut other = Config::default();
    other.theme = Some(ThemeConfig {
        mode: ThemeMode::Dark,
        variant: ThemeVariant::Minions,
    });

    let changed = base.merge_missing(&other);
    assert!(changed);
    assert_eq!(base.theme.as_ref().unwrap().variant, ThemeVariant::Minions);
}

#[test]
fn load_with_host_staging_defaults_uses_host_staging_when_project_unset() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let host_cas = home.path().join(".cas");
    std::fs::create_dir_all(&host_cas).unwrap();
    std::fs::write(
        host_cas.join("config.toml"),
        "[staging]\nlarge_artifact_dir = \"/mnt/host-staging\"\n",
    )
    .unwrap();

    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    let loaded = Config::load_with_host_staging_defaults(project.path()).unwrap();

    assert_eq!(
        loaded
            .staging
            .as_ref()
            .and_then(|s| s.staging_dir.as_deref()),
        Some("/mnt/host-staging")
    );
}

#[test]
fn load_with_host_staging_defaults_project_staging_overrides_host_staging() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let host_cas = home.path().join(".cas");
    std::fs::create_dir_all(&host_cas).unwrap();
    std::fs::write(
        host_cas.join("config.toml"),
        "[staging]\nlarge_artifact_dir = \"/mnt/host-staging\"\n",
    )
    .unwrap();
    std::fs::write(
        project.path().join("config.toml"),
        "[staging]\nstaging_dir = \"/mnt/project-staging\"\n",
    )
    .unwrap();

    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    let loaded = Config::load_with_host_staging_defaults(project.path()).unwrap();

    assert_eq!(
        loaded
            .staging
            .as_ref()
            .and_then(|s| s.staging_dir.as_deref()),
        Some("/mnt/project-staging")
    );
}

#[test]
fn load_with_host_staging_defaults_does_not_leak_other_host_sections() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let host_cas = home.path().join(".cas");
    std::fs::create_dir_all(&host_cas).unwrap();
    std::fs::write(
        host_cas.join("config.toml"),
        "[staging]\nlarge_artifact_dir = \"/mnt/host-staging\"\n\n[hooks]\ncapture_enabled = false\n\n[llm]\nmodel = \"host-only-model\"\n",
    )
    .unwrap();

    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    let loaded = Config::load_with_host_staging_defaults(project.path()).unwrap();

    assert_eq!(
        loaded
            .staging
            .as_ref()
            .and_then(|s| s.staging_dir.as_deref()),
        Some("/mnt/host-staging")
    );
    assert!(loaded.hooks.is_none(), "host hooks config must not leak");
    assert!(loaded.llm.is_none(), "host llm config must not leak");
}

#[test]
fn config_set_supports_staging_keys_and_alias() {
    let mut config = Config::default();

    config
        .set("staging.large_artifact_dir", "/mnt/large-artifacts")
        .unwrap();
    config
        .set("staging.tmpfs_warning_threshold_bytes", "2048")
        .unwrap();

    let staging = config.staging.as_ref().expect("staging section");
    assert_eq!(staging.staging_dir.as_deref(), Some("/mnt/large-artifacts"));
    assert_eq!(staging.tmpfs_warning_threshold_bytes, 2048);

    config.set("staging.staging_dir", "").unwrap();
    assert_eq!(
        config
            .staging
            .as_ref()
            .and_then(|staging| staging.staging_dir.as_deref()),
        None
    );
}

#[test]
fn test_merge_missing_does_not_overwrite_existing() {
    let mut base = Config::default();
    base.theme = Some(ThemeConfig {
        mode: ThemeMode::Light,
        variant: ThemeVariant::Default,
    });

    let mut other = Config::default();
    other.theme = Some(ThemeConfig {
        mode: ThemeMode::Dark,
        variant: ThemeVariant::Minions,
    });

    let changed = base.merge_missing(&other);
    assert!(!changed);
    assert_eq!(base.theme.as_ref().unwrap().variant, ThemeVariant::Default);
}

#[test]
fn test_load_merges_stale_yaml_into_toml() {
    let temp = TempDir::new().unwrap();

    // Write TOML without theme
    let config = Config::default();
    config.save_toml(temp.path()).unwrap();

    // Write YAML with theme (simulates stale write)
    let yaml = "theme:\n  variant: minions\n";
    std::fs::write(temp.path().join("config.yaml"), yaml).unwrap();

    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(
        loaded.theme.as_ref().unwrap().variant,
        ThemeVariant::Minions,
        "theme from YAML should be merged into TOML config"
    );

    // YAML should be renamed to .bak
    assert!(!temp.path().join("config.yaml").exists());
    assert!(temp.path().join("config.yaml.bak").exists());

    // TOML should now contain the theme
    let reloaded = Config::load(temp.path()).unwrap();
    assert_eq!(
        reloaded.theme.as_ref().unwrap().variant,
        ThemeVariant::Minions,
        "theme should persist in TOML after merge"
    );
}

#[test]
fn test_config_get_set() {
    let mut config = Config::default();

    config.set("sync.enabled", "false").unwrap();
    assert_eq!(config.get("sync.enabled"), Some("false".to_string()));

    config.set("sync.target", "/custom/path").unwrap();
    assert_eq!(config.get("sync.target"), Some("/custom/path".to_string()));
}

#[test]
fn issues_repo_is_project_local_config_with_no_inferred_default() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    assert_eq!(config.get("issues.repo"), Some(String::new()));
    assert!(
        config
            .list()
            .contains(&("issues.repo".to_string(), String::new()))
    );

    config.set("issues.repo", " owner/example-cas ").unwrap();
    assert_eq!(
        config.get("issues.repo"),
        Some("owner/example-cas".to_string())
    );
    config.save(temp.path()).unwrap();

    let raw = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(raw.contains("[issues]"));
    assert!(raw.contains("repo = \"owner/example-cas\""));
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(
        loaded
            .issues
            .as_ref()
            .and_then(|issues| issues.repo.as_deref()),
        Some("owner/example-cas")
    );

    config.set("issues.repo", "").unwrap();
    assert_eq!(config.get("issues.repo"), Some(String::new()));
    assert!(config.issues.as_ref().unwrap().repo.is_none());

    let meta = meta::registry()
        .get("issues.repo")
        .expect("issues.repo registry metadata");
    assert_eq!(meta.section, "issues");
    assert_eq!(meta.default, "");
}

#[test]
fn test_worktrees_abandon_ttl_hours_default() {
    let config = Config::default();
    assert_eq!(
        config.get("worktrees.abandon_ttl_hours"),
        Some("24".to_string())
    );
    assert_eq!(config.worktrees().abandon_ttl_hours, 24);
}

#[test]
fn test_worktrees_abandon_ttl_hours_roundtrip() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    config.set("worktrees.abandon_ttl_hours", "72").unwrap();
    assert_eq!(
        config.get("worktrees.abandon_ttl_hours"),
        Some("72".to_string())
    );

    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.worktrees().abandon_ttl_hours, 72);
}

#[test]
fn test_worktrees_abandon_ttl_hours_invalid() {
    let mut config = Config::default();
    assert!(
        config
            .set("worktrees.abandon_ttl_hours", "not-a-number")
            .is_err()
    );
    // Value must be unchanged after a rejected set.
    assert_eq!(config.worktrees().abandon_ttl_hours, 24);
}

#[test]
fn test_worktrees_global_sweep_debounce_secs_default() {
    let config = Config::default();
    assert_eq!(
        config.get("worktrees.global_sweep_debounce_secs"),
        Some("3600".to_string())
    );
    assert_eq!(config.worktrees().global_sweep_debounce_secs, 3600);
}

#[test]
fn test_worktrees_global_sweep_debounce_secs_roundtrip() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    config
        .set("worktrees.global_sweep_debounce_secs", "900")
        .unwrap();
    assert_eq!(
        config.get("worktrees.global_sweep_debounce_secs"),
        Some("900".to_string())
    );

    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.worktrees().global_sweep_debounce_secs, 900);
}

#[test]
fn test_worktrees_global_sweep_debounce_secs_invalid() {
    let mut config = Config::default();
    assert!(
        config
            .set("worktrees.global_sweep_debounce_secs", "nope")
            .is_err()
    );
    assert_eq!(config.worktrees().global_sweep_debounce_secs, 3600);
}

// ── cas-fbac: llm.harness reset/clear must not hard-error ──────────────────
//
// llm.harness's seed `default:` is the sentinel "(default)" (it resolves per
// role, not to one literal — see cas-05e3/cas-fbac), but its constraint is
// `Constraint::OneOf(["claude", "codex"])`. `Config::set` used to validate
// unconditionally before dispatch, so `set(key, "(default)")` — exactly what
// `cas config reset` / the TUI 'd' key / the interactive editor send — and
// plain `set(key, "")` both failed OneOf validation instead of clearing the
// field. These tests pin the fix: both spellings must clear `harness` back
// to `None` without error, which restores the worker-stock-floor / literal-
// claude split from `harness_for_role`.

#[test]
fn test_llm_harness_reset_sentinel_clears_to_stock_floor() {
    let mut config = Config::default();
    config.set("llm.harness", "claude").unwrap();
    assert_eq!(config.llm().harness, Some("claude".to_string()));

    // Exactly what `cas config reset llm.harness` / TUI 'd' / the interactive
    // editor do: `config.set(key, meta.default)`.
    let meta = meta::registry().get("llm.harness").unwrap();
    assert_eq!(
        meta.default, "(default)",
        "this test assumes llm.harness's seed default is still the sentinel"
    );
    config
        .set("llm.harness", meta.default)
        .expect("reset sentinel must not hard-error on a OneOf-constrained field");

    assert_eq!(
        config.llm().harness,
        None,
        "reset must clear harness back to unset, not persist the literal \"(default)\" string"
    );
    assert_eq!(config.llm().harness_for_role("worker"), "codex");
    assert_eq!(config.llm().harness_for_role("supervisor"), "claude");
}

#[test]
fn test_llm_harness_set_empty_string_clears_to_stock_floor() {
    let mut config = Config::default();
    config.set("llm.harness", "claude").unwrap();

    config
        .set("llm.harness", "")
        .expect("clearing via an empty string must not hard-error on a OneOf-constrained field");

    assert_eq!(config.llm().harness, None);
    assert_eq!(config.llm().harness_for_role("worker"), "codex");
    assert_eq!(config.llm().harness_for_role("supervisor"), "claude");
}

#[test]
fn test_llm_harness_still_rejects_invalid_values() {
    // The (default)/"" clear-path carve-out must not weaken OneOf validation
    // for genuinely invalid input.
    let mut config = Config::default();
    assert!(config.set("llm.harness", "chatgpt").is_err());
    assert_eq!(config.llm().harness, None);
}

#[test]
fn test_llm_harness_top_level_override_suppresses_worker_stock_floor() {
    // Coverage gap flagged in review: a top-level `llm.harness = "claude"`
    // with no `[llm.worker]` block must still win over the worker stock
    // floor — proving step 2 of the fallback chain (top-level override)
    // suppresses step 3 (worker-only stock default).
    let mut config = Config::default();
    config.set("llm.harness", "claude").unwrap();

    assert_eq!(
        config.llm().harness_for_role("worker"),
        "claude",
        "explicit top-level harness must suppress the codex stock floor for workers"
    );
    assert_eq!(config.llm().harness_for_role("supervisor"), "claude");
}

// ── cas-62b0 / GH #152: `[code_review] owner` is a first-class config key ──
//
// The struct has existed since cas-b51a and every runtime gate reads it, but
// the CLI surface (get/set/list/registry) never knew the key. A downstream
// project set `owner = "supervisor"`, asked `cas config get
// code_review.owner`, was told "Unknown config key", and reasonably concluded
// the setting did nothing — while five multi-persona review runs (~500k
// subagent tokens each) proceeded against it.

#[test]
fn code_review_owner_is_readable_settable_and_listed() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    // Absent section must report the *effective* value, not empty. The
    // runtime default is supervisor-owned (cas-865b), so anything else here
    // would misreport which gate the project is actually under.
    assert!(config.code_review.is_none());
    assert_eq!(
        config.get("code_review.owner"),
        Some("supervisor".to_string()),
        "an absent [code_review] section must still report the effective default"
    );
    assert!(
        config
            .list()
            .contains(&("code_review.owner".to_string(), "supervisor".to_string())),
        "`cas config list` must show the key so the policy is auditable"
    );

    config.set("code_review.owner", "worker").unwrap();
    assert_eq!(config.get("code_review.owner"), Some("worker".to_string()));
    assert!(!config.code_review.as_ref().unwrap().supervisor_owned());

    // Round-trips through TOML — the shape a project actually commits.
    config.save(temp.path()).unwrap();
    let raw = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(raw.contains("[code_review]"));
    assert!(raw.contains("owner = \"worker\""));
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.get("code_review.owner"), Some("worker".to_string()));

    // Case-tolerant on the way in, canonical on the way out, because
    // `supervisor_owned()` compares case-insensitively and a value that reads
    // back differently than it was written is how "the setting isn't
    // recognized" gets reported a second time. (Surrounding whitespace is
    // rejected by the registry's OneOf constraint before `set` is reached —
    // same as every other enum-valued key, e.g. llm.harness; not special-cased
    // here.)
    config.set("code_review.owner", "SUPERVISOR").unwrap();
    assert_eq!(
        config.get("code_review.owner"),
        Some("supervisor".to_string())
    );
    assert!(config.code_review.as_ref().unwrap().supervisor_owned());
}

#[test]
fn code_review_owner_rejects_values_the_runtime_cannot_honour() {
    let mut config = Config::default();

    // `supervisor_owned()` is an equality test against "supervisor"; every
    // other string silently means "worker". A typo must fail loudly at set
    // time rather than quietly reinstate the ~500k-token inline pipeline.
    let err = config.set("code_review.owner", "supervisors").unwrap_err();
    assert!(
        err.to_string().contains("supervisor") && err.to_string().contains("worker"),
        "rejection must name both accepted owners, got: {err}"
    );
    assert!(
        config.code_review.is_none(),
        "a rejected set must not mutate"
    );

    assert!(
        meta::registry()
            .validate("code_review.owner", "worker")
            .is_ok(),
        "registry must recognize the key for `cas config set` validation"
    );
    assert!(
        meta::registry()
            .validate("code_review.owner", "nobody")
            .is_err(),
        "registry constraint must reject unknown owners"
    );
}

#[test]
fn code_review_owner_has_registry_metadata_so_describe_and_search_find_it() {
    let reg = meta::registry();
    let meta_entry = reg
        .get("code_review.owner")
        .expect("code_review.owner must be registered — this is the GH #152 'Unknown config key'");
    assert_eq!(meta_entry.section, "code_review");
    assert_eq!(meta_entry.default, "supervisor");
    assert!(
        reg.section_description("code_review").is_some(),
        "a section with keys but no description renders headerless in `cas config list`"
    );
    assert!(
        reg.search("cas-code-review")
            .iter()
            .any(|m| m.key == "code_review.owner"),
        "searching for the skill name must surface the key that governs it"
    );
}
