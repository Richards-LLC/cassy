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
    assert_eq!(
        config.daemon().archive_max_bytes,
        cas_store::DEFAULT_TRACE_ARCHIVE_MAX_BYTES
    );
    assert_eq!(config.daemon().archive_retention_days, 0);
    assert_eq!(config.memory().decay.curated_importance_floor, 0.9);
    assert!(config.memory().decay.promote_on_access);
    assert_eq!(
        config.get("memory.decay.curated_importance_floor"),
        Some("0.9".to_string())
    );
    assert_eq!(config.sync.promotion_threshold, 2);
    assert_eq!(config.sync.demotion_threshold, 2);
    assert_eq!(config.sync.promotion_evidence, vec!["helpful"]);
    assert!(!config.skill_validation().require_sandbox);
    assert_eq!(
        config.get("skill_validation.require_sandbox"),
        Some("false".to_string())
    );
    assert!(
        meta::registry()
            .get("skill_validation.require_sandbox")
            .is_some()
    );
    assert!(config.hooks().stop.rule_review_enabled);
    let rule_review = meta::registry()
        .get("hooks.stop.rule_review_enabled")
        .expect("rule review config metadata");
    assert_eq!(rule_review.default, "true");
    assert!(
        rule_review
            .description
            .contains("Factory workers are exempt")
    );
}
#[test]
fn memory_decay_policy_is_configurable_and_round_trips() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    config
        .set("memory.decay.curated_importance_floor", "0.95")
        .unwrap();
    config
        .set("memory.decay.promote_on_access", "false")
        .unwrap();

    assert_eq!(config.memory().decay.curated_importance_floor, 0.95);
    assert!(!config.memory().decay.promote_on_access);
    assert!(
        config
            .set("memory.decay.curated_importance_floor", "1.1")
            .is_err()
    );
    assert!(
        config
            .set("memory.decay.curated_importance_floor", "nan")
            .is_err()
    );

    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.memory().decay.curated_importance_floor, 0.95);
    assert!(!loaded.memory().decay.promote_on_access);
}

#[test]
fn daemon_archive_retention_is_configurable_and_round_trips() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    assert_eq!(
        config.get("daemon.archive_retention_days"),
        Some("0".to_string())
    );
    config.set("daemon.archive_retention_days", "90").unwrap();
    assert_eq!(config.daemon().archive_retention_days, 90);
    assert!(config.list().contains(&(
        "daemon.archive_retention_days".to_string(),
        "90".to_string()
    )));
    assert!(
        meta::registry()
            .get("daemon.archive_retention_days")
            .is_some()
    );

    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.daemon().archive_retention_days, 90);
}

#[test]
fn daemon_archive_size_cap_is_configurable_and_rejects_zero() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    assert_eq!(
        config.get("daemon.archive_max_bytes"),
        Some(cas_store::DEFAULT_TRACE_ARCHIVE_MAX_BYTES.to_string())
    );
    config.set("daemon.archive_max_bytes", "4096").unwrap();
    assert_eq!(config.daemon().archive_max_bytes, 4096);
    assert!(
        config
            .list()
            .contains(&("daemon.archive_max_bytes".to_string(), "4096".to_string()))
    );
    assert!(meta::registry().get("daemon.archive_max_bytes").is_some());
    assert!(config.set("daemon.archive_max_bytes", "0").is_err());

    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.daemon().archive_max_bytes, 4096);
}

#[test]
fn daemon_relevance_sampling_is_configurable_and_has_weekly_defaults() {
    let mut config = Config::default();
    assert!(config.daemon().relevance_sampling_enabled);
    assert_eq!(config.daemon().relevance_sampling_interval_secs, 604_800);
    assert_eq!(config.daemon().relevance_sampling_sample_size, 20);
    assert!(
        meta::registry()
            .get("daemon.relevance_sampling_enabled")
            .is_some()
    );

    config
        .set("daemon.relevance_sampling_enabled", "false")
        .unwrap();
    config
        .set("daemon.relevance_sampling_interval_secs", "3600")
        .unwrap();
    config
        .set("daemon.relevance_sampling_sample_size", "7")
        .unwrap();
    assert!(!config.daemon().relevance_sampling_enabled);
    assert_eq!(config.daemon().relevance_sampling_interval_secs, 3600);
    assert_eq!(config.daemon().relevance_sampling_sample_size, 7);
    assert!(
        config
            .set("daemon.relevance_sampling_interval_secs", "0")
            .is_err()
    );
    assert!(
        config
            .set("daemon.relevance_sampling_sample_size", "0")
            .is_err()
    );
}

#[test]
fn test_config_save_load() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();
    config.sync.min_helpful = 5;
    config.sync.promotion_threshold = 4;
    config.sync.demotion_threshold = 3;
    config.sync.promotion_evidence = vec!["retrieval".to_string()];
    config
        .set("skill_validation.require_sandbox", "true")
        .unwrap();

    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();

    assert_eq!(loaded.sync.min_helpful, 5);
    assert_eq!(loaded.sync.promotion_threshold, 4);
    assert_eq!(loaded.sync.demotion_threshold, 3);
    assert_eq!(loaded.sync.promotion_evidence, vec!["retrieval"]);
    assert!(loaded.skill_validation().require_sandbox);
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
    config
        .set("staging.scratch_root", "/mnt/agent-scratch")
        .unwrap();

    let staging = config.staging.as_ref().expect("staging section");
    assert_eq!(staging.staging_dir.as_deref(), Some("/mnt/large-artifacts"));
    assert_eq!(staging.scratch_root.as_deref(), Some("/mnt/agent-scratch"));
    assert_eq!(staging.tmpfs_warning_threshold_bytes, 2048);

    config.set("staging.staging_dir", "").unwrap();
    assert_eq!(
        config
            .staging
            .as_ref()
            .and_then(|staging| staging.staging_dir.as_deref()),
        None
    );

    config.set("staging.scratch_root", "").unwrap();
    assert_eq!(
        config
            .staging
            .as_ref()
            .and_then(|staging| staging.scratch_root.as_deref()),
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

    config.set("sync.promotion_threshold", "4").unwrap();
    assert_eq!(
        config.get("sync.promotion_threshold"),
        Some("4".to_string())
    );

    config.set("sync.demotion_threshold", "3").unwrap();
    assert_eq!(config.get("sync.demotion_threshold"), Some("3".to_string()));

    config
        .set("sync.promotion_evidence", "retrieval, helpful")
        .unwrap();
    assert_eq!(
        config.get("sync.promotion_evidence"),
        Some("retrieval,helpful".to_string())
    );
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
fn issue_repo_registry_resolves_defaults_and_overrides_without_serializing_defaults() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    assert_eq!(
        config.get("issues.components.cassy"),
        Some("Richards-LLC/cassy".to_string())
    );
    assert_eq!(
        config.get("issues.components.mecha_cassy"),
        Some("Richards-LLC/mecha-cassy".to_string())
    );
    assert_eq!(
        config.get("issues.components.cloud"),
        Some("Richards-LLC/petra-stella-cloud".to_string())
    );

    let defaults = toml::to_string(&config).unwrap();
    assert!(
        !defaults.contains("[issues.components]"),
        "compiled defaults must not be written to config.toml"
    );

    config
        .set("issues.components.cassy", "example/runtime")
        .unwrap();
    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(
        loaded.get("issues.components.cassy"),
        Some("example/runtime".to_string())
    );
    assert_eq!(
        loaded.get("issues.components.cloud"),
        Some("Richards-LLC/petra-stella-cloud".to_string())
    );
    let raw = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(raw.contains("[issues.components]"));
    assert!(raw.contains("cassy = \"example/runtime\""));
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

#[test]
fn code_review_owner_is_unknown_after_dispatch_layer_removal() {
    let mut config = Config::default();

    assert_eq!(config.get("code_review.owner"), None);
    assert!(
        !config
            .list()
            .iter()
            .any(|(key, _)| key == "code_review.owner")
    );
    assert!(meta::registry().get("code_review.owner").is_none());
    assert!(config.set("code_review.owner", "supervisor").is_err());
}
