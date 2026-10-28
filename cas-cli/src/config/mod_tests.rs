use crate::config::*;
use crate::test_support::TestEnvGuard;
use crate::ui::theme::{ThemeConfig, ThemeMode, ThemeVariant};
use tempfile::TempDir;

#[test]
fn qa_capture_keys_round_trip_cas_db23() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();
    let values = [
        ("base_url", "http://localhost:4312"),
        ("serve", "pnpm dev --port {port}"),
        ("ready_path", "/health"),
        ("shots", "docs/qa/custom-shots.json"),
        ("credentials_env", "PROJECT_QA_CREDENTIALS"),
        ("login_path", "/login"),
        ("success_url", "^https?://[^/]+/(dashboard|account)"),
        ("email_sel", "#email"),
        ("password_sel", "#password"),
        ("submit_sel", "button[type=submit]"),
        ("mask", "[data-pii], .account-name"),
        ("viewports", "1280x800,390x844"),
        ("max_shots", "4"),
        ("timeout_secs", "180"),
        ("max_html_mb", "6"),
    ];
    for (name, value) in values {
        let key = format!("qa.capture.{name}");
        let meta = meta::registry().get(&key).expect("registered capture key");
        assert_eq!(config.get(&key).as_deref(), Some(meta.default), "{key}");
        config.set(&key, value).unwrap();
    }
    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    for (name, value) in values {
        let key = format!("qa.capture.{name}");
        let expected = if name == "mask" { "[data-pii],.account-name" } else { value };
        assert_eq!(loaded.get(&key).as_deref(), Some(expected), "{key}");
        assert!(loaded.list().contains(&(key, expected.into())));
    }
}

#[test]
fn qa_capture_rejects_invalid_values_without_mutation_cas_db23() {
    let mut config = Config::default();
    config.set("qa.capture.credentials_env", "PROJECT_QA_CREDENTIALS").unwrap();
    config.set("qa.capture.success_url", "^https?://[^/]+/dashboard").unwrap();
    for (name, value) in [
        ("credentials_env", "password=do-not-store"),
        ("credentials_env", "/tmp/credentials.json"),
        ("credentials_env", "9INVALID"),
        ("success_url", "/dashboard"),
        ("success_url", "^https://example.com/dashboard|/login"),
        ("success_url", "^(?x)dashboard # [\n | login"),
        ("success_url", "^(["),
        ("base_url", "file:///tmp/page"),
        ("base_url", "https://user:password@example.com"),
        ("max_shots", "0"),
        ("max_shots", "7"),
        ("serve", "pnpm dev"),
        ("ready_path", "https://other.example/"),
        ("login_path", "//other.example/login"),
        ("timeout_secs", "241"),
        ("max_html_mb", "9"),
        ("viewports", "desktop"),
    ] {
        let key = format!("qa.capture.{name}");
        let before = toml::to_string(&config).unwrap();
        assert!(meta::registry().validate(&key, value).is_err(), "{key}");
        assert!(config.set(&key, value).is_err(), "{key}");
        assert_eq!(toml::to_string(&config).unwrap(), before, "{key}");
    }
}

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
            .contains("including factory sessions")
    );
}

#[test]
fn terminal_render_defaults_and_explicit_paths_round_trip_cas_2de9() {
    let env = crate::test_support::TestEnvGuard::temp_home();
    let root = env.home().join("project/.cas");
    std::fs::create_dir_all(&root).unwrap();
    let key = "qa.terminal_render_paths";
    let mut config = Config::default();
    assert_eq!(config.get(key).unwrap(), meta::registry().get(key).unwrap().default);
    config.set(key, "src/shared/console.rs").unwrap();
    config.save(&root).unwrap();
    assert_eq!(Config::load(&root).unwrap().get(key).as_deref(), Some("src/shared/console.rs"));
}

#[test]
fn terminal_interaction_config_round_trips_and_resets_cas_266e() {
    let temp = TempDir::new().unwrap();
    let mut config: Config = toml::from_str("[qa]\nevidence_gate = true\n").unwrap();
    let key = "qa.terminal_interaction_paths";
    let default = config.get(key).unwrap();
    assert!(default.contains("**/ui/factory/**"));
    assert_eq!(meta::registry().get(key).unwrap().default, default);
    config.set(key, "src/tui/**, src/pty/**").unwrap();
    config.save(temp.path()).unwrap();
    let mut loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.get(key).as_deref(), Some("src/tui/**,src/pty/**"));
    assert!(loaded.list().contains(&(key.into(), "src/tui/**,src/pty/**".into())));
    loaded.set(key, "").unwrap();
    assert!(loaded.qa().terminal_interaction_paths.is_empty());
    loaded.set(key, meta::registry().get(key).unwrap().default).unwrap();
    assert_eq!(loaded.get(key), Some(default));
}

#[test]
fn qa_user_facing_labels_default_and_round_trip() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    assert_eq!(
        config.qa().user_facing_labels,
        vec!["ui", "hub", "hub-web", "cli-ux", "commander", "frontend"]
    );
    assert_eq!(
        config.get("qa.user_facing_labels"),
        Some("ui,hub,hub-web,cli-ux,commander,frontend".to_string())
    );
    assert!(meta::registry().get("qa.user_facing_labels").is_some());

    config
        .set("qa.user_facing_labels", "mobile, public-api")
        .unwrap();
    assert_eq!(
        config
            .list()
            .into_iter()
            .find(|(key, _)| key == "qa.user_facing_labels"),
        Some((
            "qa.user_facing_labels".to_string(),
            "mobile,public-api".to_string()
        ))
    );
    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.qa().user_facing_labels, vec!["mobile", "public-api"]);
}

#[test]
fn qa_preflight_keys_default_off_and_round_trip_cas_d5c1() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();
    assert!(!crate::qa_pass::preflight::is_configured(&config.qa()));
    for key in [
        "qa.preflight_gh_token",
        "qa.preflight_env_files",
        "qa.preflight_hook",
        "qa.preflight_hook_timeout_secs",
    ] {
        assert!(meta::registry().get(key).is_some(), "{key}");
    }
    assert_eq!(config.get("qa.preflight_hook_timeout_secs"), Some("120".to_string()));

    config.set("qa.preflight_gh_token", "true").unwrap();
    config
        .set("qa.preflight_env_files", "GABBER_BACKEND_ENV_FILE, ")
        .unwrap();
    config
        .set("qa.preflight_hook", "scripts/qa-topup-credits.sh")
        .unwrap();
    config.set("qa.preflight_hook_timeout_secs", "30").unwrap();
    assert!(config.set("qa.preflight_hook_timeout_secs", "0").is_err());
    config.save(temp.path()).unwrap();

    let qa = Config::load(temp.path()).unwrap().qa();
    assert!(qa.preflight_gh_token);
    assert_eq!(qa.preflight_env_files, vec!["GABBER_BACKEND_ENV_FILE"]);
    assert_eq!(qa.preflight_hook.as_deref(), Some("scripts/qa-topup-credits.sh"));
    assert_eq!(qa.preflight_hook_timeout_secs, 30);
    assert!(crate::qa_pass::preflight::is_configured(&qa));
}

#[test]
fn qa_telemetry_sweep_is_optional_and_round_trips() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    assert_eq!(config.qa().telemetry_sweep, None);
    assert_eq!(config.get("qa.telemetry_sweep"), Some(String::new()));
    assert!(meta::registry().get("qa.telemetry_sweep").is_some());

    config
        .set("qa.telemetry_sweep", " scripts/qa/telemetry-sweep.sh ")
        .unwrap();
    assert_eq!(
        config.qa().telemetry_sweep.as_deref(),
        Some("scripts/qa/telemetry-sweep.sh")
    );
    assert!(config.list().contains(&(
        "qa.telemetry_sweep".to_string(),
        "scripts/qa/telemetry-sweep.sh".to_string()
    )));

    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(
        loaded.qa().telemetry_sweep.as_deref(),
        Some("scripts/qa/telemetry-sweep.sh")
    );

    config.set("qa.telemetry_sweep", "  ").unwrap();
    assert_eq!(config.qa().telemetry_sweep, None);
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

    // The docs gates share the public get/set/list/meta/save/load contract.
    for key in ["upkeep.codemap", "upkeep.product_overview"] {
        assert_eq!(config.get(key).as_deref(), Some("enforce"));
        assert!(meta::registry().get(key).is_some());
        assert!(config.set(key, "strict").is_err());
        config.set(key, "observe").unwrap();
        assert!(config.list().contains(&(key.into(), "observe".into())));
    }
    config.set("qa.registry.enforce", "observe").unwrap();
    config.save(temp.path()).unwrap();
    let mut loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.get("upkeep.product_overview").as_deref(), Some("observe"));
    // Explicitly selected observe survives subsequent releases.
    assert!(refresh_upkeep(temp.path(), "9.99.2", false, false).unwrap().is_empty());
    let existing = TempDir::new().unwrap();
    let path = existing.path().join("config.toml");
    let original = "# operator context\n[project_overview]\ndomain_paths=['schema/model.sql']\n";
    std::fs::write(&path, original).unwrap();
    assert_eq!(refresh_upkeep(existing.path(), "9.99.1", false, true).unwrap().len(), 3);
    // pin: dry-run adoption must leave the operator's TOML bytes untouched.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "dry run must not write");
    // Registry gates G1–G3 share the window with the docs gates.
    assert_eq!(refresh_upkeep(existing.path(), "9.99.1", false, false).unwrap().len(), 3);
    assert_eq!(Config::load(existing.path()).unwrap().get("qa.registry.enforce").as_deref(), Some("observe"));
    let generated = std::fs::read_to_string(&path).unwrap();
    assert!(generated.contains(original), "preserve comments and unknown sections");
    assert!(refresh_upkeep(existing.path(), "9.99.1", false, false).unwrap().is_empty(), "same release preserves window");
    assert!(refresh_upkeep(existing.path(), "9.99.0", false, false).unwrap().is_empty(), "downgrade preserves window");
    loaded = Config::load(existing.path()).unwrap();
    loaded.set("upkeep.codemap", "observe").unwrap(); // pin an autogenerated value
    loaded.set("qa.registry.enforce", "observe").unwrap();
    loaded.save(existing.path()).unwrap();
    let pinned: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(pinned["project_overview"]["domain_paths"][0].as_str(), Some("schema/model.sql"), "pinning must preserve overview watch targets");
    let receipt = refresh_upkeep(existing.path(), "9.99.2", false, false).unwrap();
    assert_eq!(receipt, vec!["upkeep.product_overview = enforce"]);
    loaded = Config::load(existing.path()).unwrap();
    assert_eq!(loaded.get("upkeep.codemap").as_deref(), Some("observe"));
    assert_eq!(loaded.get("upkeep.product_overview").as_deref(), Some("enforce"));
    assert_eq!(loaded.get("qa.registry.enforce").as_deref(), Some("observe"));
    assert!(refresh_upkeep(existing.path(), "9.99.3", false, false).unwrap().is_empty());
    loaded.set("qa.registry.enforce", "strict").unwrap();
    loaded.save(existing.path()).unwrap();
    refresh_upkeep(existing.path(), "9.99.3", true, false).unwrap();
    loaded = Config::load(existing.path()).unwrap();
    assert_eq!(loaded.get("upkeep.codemap").as_deref(), Some("enforce"));
    assert_eq!(loaded.get("qa.registry.enforce").as_deref(), Some("strict"), "seed keeps opt-in strict");
    assert!(loaded.upkeep().observe_since.is_empty(), "seed starts enforce");
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
fn config_save_replaces_the_whole_document_and_preserves_project_aliases() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("config.toml"),
        "[hooks]\nai_context = false\n\n[project]\ncanonical_id = \"github.com/foo/bar\"\naliases = [\"legacy-bar\"]\n",
    )
    .unwrap();

    let mut config = Config::load(temp.path()).unwrap();
    config.set("hooks.ai_context", "true").unwrap();
    config.save_toml(temp.path()).unwrap();

    let raw = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    let parsed: toml::Value = toml::from_str(&raw).unwrap();
    assert_eq!(parsed["hooks"]["ai_context"].as_bool(), Some(true));
    assert_eq!(
        parsed["project"]["canonical_id"].as_str(),
        Some("github.com/foo/bar")
    );
    assert_eq!(
        parsed["project"]["aliases"].as_array().unwrap(),
        &[toml::Value::String("legacy-bar".to_string())]
    );
    assert!(
        temp.path().join(".config.toml.cas-write.lock").exists(),
        "config saves must use the shared project-config lock"
    );
    assert!(
        !raw.contains("\nlse\n"),
        "a short project block must never leave stale bytes in the document"
    );
}

#[test]
fn malformed_config_error_names_line_and_repair_remedy() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("config.toml"),
        "[project]\naliases = []\nlse\n[hooks]\nai_context = false\n\n[project]\naliases = []\ncanonical_id = \"cas-src\"\n",
    )
    .unwrap();

    let error = Config::load(temp.path()).unwrap_err().to_string();
    assert!(
        error.contains("line 3"),
        "error must identify the bad line: {error}"
    );
    assert!(
        error.contains("Restore a known-good config.toml backup"),
        "error must name the repair remedy: {error}"
    );
}

#[test]
fn test_config_get_set() {
    let mut config = Config::default();

    config.set("slack.channels", r#"["G123", "C123", "C123"]"#).unwrap();
    assert_eq!(config.get("slack.channels").as_deref(), Some(r#"["C123","G123"]"#));
    assert!(config.set("slack.channels", r##"["#general"]"##).is_err());
    let roundtrip: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(roundtrip.get("slack.channels"), config.get("slack.channels"));
    assert!(config.list().iter().any(|(key, value)| key == "slack.channels" && value == r#"["C123","G123"]"#));

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
fn history_github_repo_is_a_separate_optional_config_key() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();

    assert_eq!(config.get("history.github_repo"), Some(String::new()));
    assert!(
        config
            .list()
            .contains(&("history.github_repo".to_string(), String::new()))
    );
    let meta = meta::registry()
        .get("history.github_repo")
        .expect("history github repository metadata");
    assert_eq!(meta.section, "history");
    assert_eq!(meta.default, "");

    config
        .set("history.github_repo", " owner/history-repo ")
        .unwrap();
    assert_eq!(
        config.get("history.github_repo"),
        Some("owner/history-repo".to_string())
    );
    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(
        loaded
            .history
            .as_ref()
            .and_then(|history| history.github_repo.as_deref()),
        Some("owner/history-repo")
    );
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
        config.get("issues.components.violet"),
        Some("Richards-LLC/violet_ps".to_string())
    );
    assert_eq!(
        config.get("issues.components.cloud"),
        Some("Richards-LLC/petra-stella-cloud".to_string())
    );
    for (key, default) in [
        ("issues.components.cassy", "Richards-LLC/cassy"),
        ("issues.components.violet", "Richards-LLC/violet_ps"),
        ("issues.components.cloud", "Richards-LLC/petra-stella-cloud"),
    ] {
        assert!(config
            .list()
            .contains(&(key.to_string(), default.to_string())));
        let meta = meta::registry().get(key).expect("component issue metadata");
        assert_eq!(meta.section, "issues.components");
        assert_eq!(meta.default, default);
    }

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
fn violet_issue_key_resolves_and_retired_key_is_rejected() {
    let mut config = Config::default();
    let retired = &cas_types::violet_compatibility::violet_compatibility().retired_issue_key;
    assert_eq!(
        config.get("issues.components.violet"),
        Some("Richards-LLC/violet_ps".to_owned())
    );
    assert!(config.get(retired).is_none());
    assert!(config.set(retired, "example/hub").is_err());
    assert!(meta::registry().get(retired).is_none());
    config
        .set("issues.components.violet", "example/hub")
        .unwrap();
    assert_eq!(config.issue_repo_registry().violet, "example/hub");
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

/// cas-8d54 (GH #1011): `cas config get/set factory.epic_base_branch` used to
/// say "Unknown config key" although the runtime reads the key. It is
/// registered, readable, listable, and a set value round-trips to the reader
/// the factory uses.
#[test]
fn factory_epic_base_branch_is_registered_and_round_trips_cas_8d54() {
    let temp = TempDir::new().unwrap();
    let cas_dir = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    let mut config = Config::default();

    assert!(meta::registry().get("factory.epic_base_branch").is_some());
    assert_eq!(config.get("factory.epic_base_branch"), Some(String::new()));
    assert!(config.list().contains(&("factory.epic_base_branch".to_string(), String::new())));

    config.set("factory.epic_base_branch", " staging ").unwrap();
    assert_eq!(config.get("factory.epic_base_branch"), Some("staging".to_string()));
    assert_eq!(config.factory().epic_base_branch.as_deref(), Some("staging"));
    assert!(config.list().contains(&(
        "factory.epic_base_branch".to_string(),
        "staging".to_string()
    )));

    config.save(&cas_dir).unwrap();
    let loaded = Config::load(&cas_dir).unwrap();
    assert_eq!(loaded.get("factory.epic_base_branch"), Some("staging".to_string()));
    assert_eq!(
        Config::configured_epic_base_branch(temp.path()).as_deref(),
        Some("staging"),
        "the runtime reader sees the value `cas config set` wrote"
    );

    config.set("factory.epic_base_branch", "  ").unwrap();
    assert_eq!(config.factory().epic_base_branch, None);
    assert_eq!(config.get("factory.epic_base_branch"), Some(String::new()));
}

/// cas-1a05: every factory key `cas config set` accepts is also returned by
/// `get` and `list`, and a set value reads back. The table is the settable
/// set; it must cover every registered `factory.*` key, so a key added to the
/// registry or to `set` without `get`/`list` fails here.
#[test]
fn every_settable_factory_key_round_trips_through_get_and_list_cas_1a05() {
    assert_eq!(Config::default().factory().spawn_min_free_gib, 25);
    assert_eq!(
        Config::default().get("factory.spawn_min_free_gib"),
        Some("25".into())
    );
    // (key, value to set, value get returns)
    let table: &[(&str, &str, &str)] = &[
        ("factory.slot_id", " second-supervisor ", "second-supervisor"),
        ("factory.artifacts_root", " /mnt/scratch/artifacts ", "/mnt/scratch/artifacts"),
        ("factory.scratch_root", " /mnt/rewind/cas ", "/mnt/rewind/cas"),
        ("factory.supervisor_only_mcp", " vercel, neon ", "vercel,neon"),
        ("factory.supervisor_only_env", " VERCEL_TOKEN, NEON_API_KEY ", "VERCEL_TOKEN,NEON_API_KEY"),
        ("factory.worker_credential_env", " GITHUB_TOKEN, VERCEL_TOKEN ", "GITHUB_TOKEN,VERCEL_TOKEN"),
        ("factory.worker_denied_clis", " vercel, netlify ", "vercel,netlify"),
        ("factory.worker_denied_mcp_tools", " *vercel*, mcp__neon__* ", "*vercel*,mcp__neon__*"),
        ("factory.supervisor_message_debounce_secs", "12", "12"),
        ("factory.supervisor_message_wakes_per_hour", "4", "4"),
        ("factory.message_max_chars", "3000", "3000"),
        ("factory.message_max_chars_escalation", "6000", "6000"),
        ("factory.note_max_chars", "1800", "1800"),
        ("factory.commander_mirror_enabled", "false", "false"),
        ("factory.max_concurrent_builders", "3", "3"),
        ("factory.build_guard.max_load_ratio", "2", "2.0"),
        ("factory.build_guard.max_cpu_pressure_percent", "85.5", "85.5"),
        ("factory.build_guard.max_memory_pressure_percent", "5.5", "5.5"),
        ("factory.build_guard.min_available_memory_gib", "4", "4"),
        ("factory.worker_memory_high_gib", "6", "6"),
        ("factory.render_max_fps", "12", "12"),
        ("factory.render_unfocused_fps", "3", "3"),
        ("factory.spawn_min_free_gib", "30", "30"),
        ("factory.prompt_retention_days", "14", "14"),
        ("factory.event_telemetry_retention_days", "30", "30"),
        ("factory.prompt_transcript_retention_days", "21", "21"),
        ("factory.supervisor_queue_retention_days", "28", "28"),
        ("factory.worker_build_jobs", "6", "6"),
        ("factory.cargo_build_jobs", "5", "5"),
        ("factory.nice_cargo", "false", "false"),
        ("factory.claude.worker_mcp", "inherit", "inherit"),
        ("factory.claude.remote_control", "true", "true"),
        ("factory.claude.fallback_model", " sonnet,haiku ", "sonnet,haiku"),
        ("factory.warn_stale_assignment", "false", "false"),
        ("factory.block_stale_assignment", "false", "false"),
        ("factory.stale_threshold_commits", "3", "3"),
        ("factory.stall_threshold_secs", "420", "420"),
        ("factory.context_recycle_threshold_percent", "85", "85"),
        ("factory.stall_after_secs", "900", "900"),
        ("factory.delivery_stalled_priority_secs", "120", "120"),
        ("factory.delivery_stalled_normal_secs", "240", "240"),
        ("factory.strict_cli", "true", "true"),
        ("factory.target_cache_high_watermark_percent", "90", "90"),
        ("factory.target_cache_low_watermark_percent", "70", "70"),
        ("factory.target_cache_min_idle_secs", "7200", "7200"),
        ("factory.target_cache_retention_count", "2", "2"),
        ("factory.epic_base_branch", "staging", "staging"),
        ("factory.ai_enrichment.enabled", "true", "true"),
        ("factory.ai_enrichment.endpoint", "http://127.0.0.1:11434/v1/responses", "http://127.0.0.1:11434/v1/responses"),
        ("factory.ai_enrichment.provider", "openai-compatible", "openai-compatible"),
        ("factory.ai_enrichment.api_key_env", "LOCAL_MODEL_KEY", "LOCAL_MODEL_KEY"),
        ("factory.ai_enrichment.model", "local-summary-1", "local-summary-1"),
        ("factory.ai_enrichment.effort", "low", "low"),
    ];
    let registry = meta::registry();
    // GH #1182: `factory.write_roots` is registered, readable and listed, but
    // operator-only: the generic setter refuses it and only the operator CLI
    // path writes it (see `factory_write_roots_are_operator_only_gh_1182`).
    const OPERATOR_ONLY: &[&str] = &["factory.write_roots"];
    for key in registry
        .all_keys()
        .into_iter()
        .filter(|key| key.starts_with("factory.") && !OPERATOR_ONLY.contains(key))
    {
        assert!(
            table.iter().any(|(settable, _, _)| *settable == key),
            "registered key {key} is missing from this round-trip table"
        );
    }

    let temp = TempDir::new().unwrap();
    let mut config = Config::default();
    // Unset, every registered key reads as its registry default, so
    // `config list --modified` stays empty on a fresh config.
    for key in registry.all_keys().into_iter().filter(|key| key.starts_with("factory.")) {
        assert_eq!(
            config.get(key).as_deref(),
            Some(registry.get(key).unwrap().default),
            "{key}: default read does not match the registry default"
        );
    }
    for (key, _, _) in table {
        assert!(config.get(key).is_some(), "{key}: settable but `get` does not know it");
        let listed: std::collections::HashMap<String, String> = config.list().into_iter().collect();
        // cargo_build_jobs is the accepted alias of worker_build_jobs; `list` shows the canonical key.
        let listed_key = if *key == "factory.cargo_build_jobs" { "factory.worker_build_jobs" } else { key };
        assert!(listed.contains_key(listed_key), "{key}: settable but `list` omits it");
    }
    for (key, value, expected) in table {
        config
            .set(key, value)
            .unwrap_or_else(|error| panic!("{key} = {value:?}: {error}"));
        assert_eq!(config.get(key).as_deref(), Some(*expected), "{key} after set");
        let listed_key = if *key == "factory.cargo_build_jobs" { "factory.worker_build_jobs" } else { key };
        let listed: std::collections::HashMap<String, String> = config.list().into_iter().collect();
        assert_eq!(listed.get(listed_key).map(String::as_str), Some(*expected), "{key} in list");
    }

    // The values survive a save and load.
    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    for (key, _, expected) in table {
        if *key == "factory.worker_build_jobs" {
            // Overwritten by the later cargo_build_jobs alias row.
            continue;
        }
        assert_eq!(loaded.get(key).as_deref(), Some(*expected), "{key} after reload");
    }
    assert_eq!(loaded.get("factory.worker_build_jobs").as_deref(), Some("5"));

    // Optional keys clear back to unset with an empty value.
    for key in ["factory.epic_base_branch", "factory.slot_id"] {
        config.set(key, "").unwrap();
        assert_eq!(config.get(key).as_deref(), Some(""), "{key} cleared");
    }
    config.set("factory.artifacts_root", "").unwrap();
    assert_eq!(config.factory().artifacts_root, None);
    assert_eq!(
        config.get("factory.artifacts_root").as_deref(),
        Some(FACTORY_ARTIFACTS_ROOT_DEFAULT),
        "an unset artifacts_root reads as its default"
    );
}

#[test]
fn factory_nice_cargo_round_trips_cas_f7b9() {
    let env = crate::test_support::TestEnvGuard::temp_home();
    let mut config = Config::default();
    let registry = meta::registry();
    registry.validate("factory.nice_cargo", "false").unwrap();
    config.set("factory.nice_cargo", "false").unwrap();
    assert!(!config.factory().nice_cargo);
    assert_eq!(config.get("factory.nice_cargo").as_deref(), Some("false"));
    assert!(config.list().contains(&("factory.nice_cargo".into(), "false".into())));
    config.save(env.home()).unwrap();
    let mut loaded = Config::load(env.home()).unwrap();
    assert!(!loaded.factory().nice_cargo);
    loaded.set("factory.nice_cargo", "true").unwrap();
    assert!(loaded.factory().nice_cargo);
    assert!(loaded.set("factory.nice_cargo", "invalid").is_err());
    assert!(loaded.factory().nice_cargo, "invalid input must preserve the value");
}

/// Discover fields from the serialized runtime configuration, independently
/// of the registry, so an omitted registry entry cannot hide from coverage.
#[test]
fn serialized_factory_fields_have_registry_entries_cas_f7b9() {
    // Populate fields omitted when empty/None. Retired compatibility fields
    // deliberately do not serialize and are not runtime settings.
    let factory = FactoryConfig {
        artifacts_root: Some("/mnt/factory-artifacts".into()),
        scratch_root: Some("/mnt/rewind/cas".into()),
        epic_base_branch: Some("main".into()),
        write_roots: vec!["/mnt/operator-approved".into()],
        ..FactoryConfig::default()
    };
    let config = Config { factory: Some(factory.clone()), ..Config::default() };
    let serialized = serde_json::to_value(factory).unwrap();
    fn collect_keys(prefix: &str, value: &serde_json::Value, keys: &mut Vec<String>) {
        if let Some(object) = value.as_object() {
            for (field, value) in object {
                collect_keys(&format!("{prefix}.{field}"), value, keys);
            }
        } else {
            keys.push(prefix.to_string());
        }
    }
    let mut keys = Vec::new();
    collect_keys("factory", &serialized, &mut keys);
    // Each exception must name an internal field and explain why operators
    // cannot configure it. Currently every serialized field is public.
    const INTERNAL: &[(&str, &str)] = &[];
    for (key, reason) in INTERNAL {
        assert!(!reason.trim().is_empty(), "{key}: internal exception needs a reason");
        assert!(keys.iter().any(|field| field == key), "{key}: stale internal exception");
    }
    let registry = meta::registry();
    let listed: std::collections::HashMap<_, _> = config.list().into_iter().collect();
    let mut missing = Vec::new();
    for key in keys {
        if INTERNAL.iter().any(|(internal, _)| *internal == key) {
            continue;
        }
        // The serialized legacy name remains settable; discovery lists its
        // canonical worker_build_jobs name.
        let canonical = if key == "factory.cargo_build_jobs" {
            "factory.worker_build_jobs"
        } else {
            &key
        };
        if registry.get(canonical).is_none() {
            missing.push(key);
            continue;
        }
        assert!(config.get(&key).is_some(), "{key}: missing get support");
        assert!(listed.contains_key(canonical), "{key}: missing list support");
        let pointer = format!("/{}", key.strip_prefix("factory.").unwrap().replace('.', "/"));
        let value = serialized.pointer(&pointer).unwrap();
        let expected = match value {
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Array(values) => values.iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>().join(","),
            _ => value.to_string(),
        };
        assert_eq!(config.get(&key).as_deref(), Some(expected.as_str()), "{key}: runtime field differs from get");
        assert_eq!(listed.get(canonical), Some(&expected), "{key}: runtime field differs from list");
    }
    assert!(missing.is_empty(), "serialized factory fields missing registry entries: {missing:?}");
}

/// GH #1182: `[factory] write_roots` lives in config.toml, is shown by `get`
/// and `list`, survives a save and load, and is preserved by any later
/// load-modify-save of another key; the generic setter (MCP, config editor,
/// reset, import) refuses it.
#[test]
fn factory_write_roots_are_operator_only_gh_1182() {
    let temp = TempDir::new().unwrap();
    let mut config = Config::default();
    assert_eq!(config.get("factory.write_roots").as_deref(), Some(""));
    assert_eq!(
        meta::registry().get("factory.write_roots").unwrap().examples(),
        vec!["~/Downloads,/mnt/models"],
        "QA F04: describe shows a real example, not item1,item2,item3"
    );
    let error = config.set("factory.write_roots", "/tmp").unwrap_err().to_string();
    assert!(error.contains("operator"), "{error}");
    assert!(config.factory().write_roots.is_empty());

    config.set_factory_write_roots(vec!["/mnt/a".into(), "/mnt/b".into()]);
    assert_eq!(config.get("factory.write_roots").as_deref(), Some("/mnt/a,/mnt/b"));
    let listed: std::collections::HashMap<String, String> = config.list().into_iter().collect();
    assert_eq!(listed.get("factory.write_roots").map(String::as_str), Some("/mnt/a,/mnt/b"));
    config.save(temp.path()).unwrap();
    let text = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(text.contains("write_roots = ["), "{text}");

    let mut loaded = Config::load(temp.path()).unwrap();
    loaded.set("factory.note_max_chars", "1800").unwrap();
    loaded.save(temp.path()).unwrap();
    let reloaded = Config::load(temp.path()).unwrap();
    assert_eq!(reloaded.factory().write_roots, vec!["/mnt/a".to_string(), "/mnt/b".to_string()]);

    let mut cleared = reloaded;
    cleared.set_factory_write_roots(Vec::new());
    cleared.save(temp.path()).unwrap();
    let text = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(!text.contains("write_roots"), "an empty list is not serialized: {text}");
}

#[test]
fn artifact_namespaces_distinguish_same_named_projects_and_share_store_aliases_cas_6ebf() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("one/project/.cas");
    let b = temp.path().join("two/project/.cas");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let base = temp.path().join("artifacts");
    let a_dir = project_factory_artifacts_root(&a, &base);
    let b_dir = project_factory_artifacts_root(&b, &base);
    assert_ne!(a_dir, b_dir);
    assert_eq!(a_dir.parent().unwrap(), base);
    let resolved = resolved_factory_artifact_paths(&a, base.to_str());
    assert_eq!(resolved.base, base);
    assert_eq!(resolved.project_root, a_dir);
    assert_eq!(resolved.task_dirs("cas-a4b1"), factory_task_artifact_dirs(&a, &base, "cas-a4b1"));
    assert_eq!(
        factory_task_artifact_dirs(&a, &base, "cas-a4b1")[0],
        a_dir.join("cas-a4b1")
    );
    assert_eq!(
        factory_task_artifact_dirs(&a, &base, "cas-a4b1")[1],
        base.join("cas-a4b1")
    );
    #[cfg(unix)]
    {
        let alias = temp.path().join("shared-store-alias");
        std::os::unix::fs::symlink(&a, &alias).unwrap();
        assert_eq!(project_factory_artifacts_root(&alias, &base), a_dir);
    }
}

#[test]
fn factory_supervisor_only_resource_settings_round_trip_and_list_gh_1047() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    for (key, value) in [
        ("factory.supervisor_only_mcp", "vercel,neon"),
        ("factory.supervisor_only_env", "VERCEL_TOKEN,NEON_API_KEY"),
    ] {
        assert!(registry().get(key).is_some(), "cas config list needs registered metadata");
        config.set(key, value).unwrap();
        assert_eq!(config.get(key).as_deref(), Some(value));
        assert!(config.list().contains(&(key.into(), value.into())));
    }
    config.save(temp.path()).unwrap();
    let loaded = Config::load(temp.path()).unwrap();
    assert_eq!(loaded.factory().worker_policy.supervisor_only_mcp, ["vercel", "neon"]);
    let shared = cas_mux::worker_resources::load_worker_policy(Some(temp.path())).unwrap();
    assert!(shared.denies_env("VERCEL_TOKEN") && shared.denies_server("neon"));
    assert!(!FactoryConfig::default().worker_policy.denies_server("vercel"));
}

#[test]
fn worker_credential_env_round_trip_and_shared_policy_cas_82bc() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    let key = "factory.worker_credential_env";
    assert!(registry().get(key).is_some());
    config.set(key, " GITHUB_TOKEN, VERCEL_TOKEN ").unwrap();
    assert_eq!(config.get(key).as_deref(), Some("GITHUB_TOKEN,VERCEL_TOKEN"));
    assert!(config.list().contains(&(key.into(), "GITHUB_TOKEN,VERCEL_TOKEN".into())));
    config.save(temp.path()).unwrap();
    let shared = cas_mux::worker_resources::load_worker_policy(Some(temp.path())).unwrap();
    assert_eq!(shared.worker_credential_env, ["GITHUB_TOKEN", "VERCEL_TOKEN"]);
    assert!(shared.is_empty(), "credential grants alone must not require strict MCP scope");
    config.set(key, "").unwrap();
    config.save(temp.path()).unwrap();
    assert!(cas_mux::worker_resources::load_worker_policy(Some(temp.path()))
        .unwrap().worker_credential_env.is_empty());
}

/// cas-4971: Jev was removed (cas-4971). A config file written by an older
/// release with a `[jev]` section must still load, keep its other settings,
/// and lose the retired section on the next save.
#[test]
fn retired_jev_section_loads_and_is_dropped_on_save_cas_4971() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("config.toml"),
        "[jev]\nenabled = true\nmodel = \"jev-1.13.0\"\nkey_file = \"/tmp/key\"\n\n\
         [jev.gate]\nshadow = true\n\n[qa]\nuser_facing_labels = [\"mobile\"]\n",
    )
    .unwrap();

    let loaded = Config::load(temp.path()).expect("a config with [jev] still loads");
    assert!(loaded.retired_jev.is_some(), "the retired section is recognised");
    assert_eq!(loaded.qa().user_facing_labels, vec!["mobile"]);
    assert_eq!(loaded.get("jev.enabled"), None, "no jev.* key is readable");
    assert!(loaded.list().iter().all(|(key, _)| !key.starts_with("jev.")));

    loaded.save(temp.path()).unwrap();
    let saved = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(!saved.contains("jev"), "the retired section is not written back: {saved}");
    let reloaded = Config::load(temp.path()).unwrap();
    assert!(reloaded.retired_jev.is_none());
    assert_eq!(reloaded.qa().user_facing_labels, vec!["mobile"]);
}

/// cas-fd0a: an older config carrying the retired post-merge sweep keys
/// still loads through `Config::load`, keeps its other settings, refuses the
/// retired keys for get/set, and loses them on the next save.
#[test]
fn retired_merge_sweep_keys_load_and_are_dropped_on_save_cas_fd0a() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("config.toml"),
        "[factory]\nmerge_sweep = true\nmerge_sweep_timeout_secs = 1800\nmerge_sweep_quiet_secs = 300\n\
         release_gate_home_dir = \"/home/cas-release-gate/base\"\nmax_concurrent_builders = 3\n\n\
         [factory.merge_sweep_env]\nDATABASE_URL = \"postgres://x\"\n",
    )
    .unwrap();

    let mut loaded = Config::load(temp.path()).expect("an old config with sweep keys still loads");
    assert_eq!(loaded.factory().max_concurrent_builders, 3);
    assert_eq!(
        loaded.factory().retired_keys(),
        vec!["merge_sweep", "merge_sweep_timeout_secs", "merge_sweep_quiet_secs", "release_gate_home_dir", "merge_sweep_env"]
    );
    assert_eq!(loaded.get("factory.merge_sweep"), None, "a retired key is not readable");
    assert!(loaded.set("factory.merge_sweep", "true").is_err(), "a retired key is not settable");
    assert!(loaded.list().iter().all(|(key, _)| !key.contains("merge_sweep")));

    loaded.save(temp.path()).unwrap();
    let saved = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(!saved.contains("merge_sweep"), "the retired keys are not written back: {saved}");
    assert!(!saved.contains("release_gate_home_dir"), "{saved}");
    assert!(Config::load(temp.path()).unwrap().factory().retired_keys().is_empty());
}

#[test]
fn unset_issue_repo_resolves_local_origin_without_persisting_cas_f398() {
    let _env = TestEnvGuard::new();
    let temp = TempDir::new().unwrap();
    let root = temp.path().join(".cas");
    std::fs::create_dir(&root).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "git@github.com:Richards-LLC/violet_ps.git",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(temp.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let before = "[issues.components]\nviolet = \"Richards-LLC/violet_ps\"\n";
    std::fs::write(root.join("config.toml"), before).unwrap();
    let config = Config::load(&root).unwrap();
    assert_eq!(
        config.get_resolved("issues.repo", &root).as_deref(),
        Some("Richards-LLC/violet_ps")
    );
    assert_eq!(
        config
            .issue_repo_registry_for_project(&root)
            .project
            .as_deref(),
        Some("Richards-LLC/violet_ps")
    );
    assert!(config.issues.as_ref().unwrap().repo.is_none());
    assert_eq!(
        std::fs::read_to_string(root.join("config.toml")).unwrap(),
        before
    );
    assert!(!toml::to_string(&config).unwrap().contains("repo ="));
}

// Real local Git repositories exercise the same boundary as config CLI reads.
fn issue_origin_fixture_cas_f398(origin: Option<&str>) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join(".cas");
    std::fs::create_dir(&root).unwrap();
    assert!(
        std::process::Command::new("git")
            .current_dir(temp.path())
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    if let Some(origin) = origin {
        assert!(
            std::process::Command::new("git")
                .current_dir(temp.path())
                .args(["remote", "add", "origin", origin])
                .status()
                .unwrap()
                .success()
        );
    }
    (temp, root)
}

#[test]
fn issue_repo_origin_forms_and_explicit_override_cas_f398() {
    let _env = TestEnvGuard::new();
    for origin in [
        "https://github.com/example/project.git",
        "git@github.com:example/project.git",
        "ssh://git@github.com/example/project.git",
    ] {
        let (_temp, root) = issue_origin_fixture_cas_f398(Some(origin));
        let mut config = Config::default();
        let inferred = config.issue_repo_registry_for_project(&root);
        assert_eq!(
            inferred.project.as_deref(),
            Some("example/project"),
            "{origin}"
        );
        assert_eq!(inferred.cassy, config.issue_repo_registry().cassy);
        assert_eq!(inferred.violet, config.issue_repo_registry().violet);
        assert_eq!(inferred.cloud, config.issue_repo_registry().cloud);
        config.set("issues.repo", " custom/override ").unwrap();
        assert_eq!(
            config.get_resolved("issues.repo", &root).as_deref(),
            Some("custom/override")
        );
        config.set("issues.repo", " ").unwrap();
        assert_eq!(
            config.get_resolved("issues.repo", &root).as_deref(),
            Some("example/project")
        );
        assert_eq!(config.get("issues.repo").as_deref(), Some(""));
        assert_eq!(
            config.get_resolved("history.github_repo", &root),
            config.get("history.github_repo")
        );
        assert_eq!(config.get_resolved("unknown.key", &root), None);
        assert!(!root.join("config.toml").exists());
    }
}

#[test]
fn unusable_origin_does_not_infer_component_or_parent_cas_f398() {
    let _env = TestEnvGuard::new();
    for origin in [
        None,
        Some("https://gitlab.com/example/project.git"),
        Some("https://github.com/example/project/extra"),
        Some("https://github.com/example/bad%20repo.git"),
        Some("https://github.com/example/.git"),
    ] {
        let (_temp, root) = issue_origin_fixture_cas_f398(origin);
        let mut config = Config::default();
        config
            .set("issues.components.violet", "example/violet")
            .unwrap();
        assert_eq!(
            config.get_resolved("issues.repo", &root).as_deref(),
            Some(""),
            "{origin:?}"
        );
        assert!(
            config
                .issue_repo_registry_for_project(&root)
                .project
                .is_none()
        );
    }
    let (parent, _) = issue_origin_fixture_cas_f398(Some("https://github.com/example/parent.git"));
    let child = parent.path().join("nested/.cas");
    std::fs::create_dir_all(&child).unwrap();
    assert!(
        Config::default()
            .issue_repo_registry_for_project(&child)
            .project
            .is_none()
    );
    let outside = TempDir::new().unwrap();
    assert!(
        Config::default()
            .issue_repo_registry_for_project(outside.path())
            .project
            .is_none()
    );
}

/// Round-trips, defaults and listing live in the table above; this covers
/// refusals. A refused value keeps the previous one, an empty value clears an
/// optional key, and persisted TOML is validated like `config set`.
/// cas-3f93: a relative scratch root would resolve against a spawn's cwd.
#[test]
fn factory_claude_and_scratch_root_refuse_invalid_values_cas_569f_940c_3f93() {
    let mut config = Config::default();
    for (key, good, bad) in [
        ("factory.claude.fallback_model", "sonnet", "sonnet\nhaiku"),
        ("factory.claude.remote_control", "true", "invalid"),
        ("factory.claude.worker_mcp", "inherit", "all"),
        ("factory.scratch_root", "/mnt/rewind/cas", "rewind/cas"),
    ] {
        config.set(key, good).unwrap();
        let error = config.set(key, bad).unwrap_err().to_string();
        assert_eq!(config.get(key).as_deref(), Some(good), "{key} kept after {bad:?}");
        if key == "factory.scratch_root" {
            assert!(error.contains("absolute"), "{error}");
        }
    }
    config.set("factory.claude.fallback_model", "").unwrap();
    config.set("factory.scratch_root", "").unwrap();
    assert!(config.factory().claude.fallback_model.is_none());
    assert_eq!(config.factory().scratch_root, None);
    for invalid in [
        "[factory.claude]\nfallback_model=\"sonnet\\nhaiku\"",
        "[factory.claude]\nworker_mcp='all'",
    ] {
        assert!(toml::from_str::<Config>(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn cas_2d63_build_guard_rejects_invalid_limits_without_mutating_config() {
    let mut config = Config::default();
    for (key, values) in [
        ("factory.build_guard.max_load_ratio", &["0", "-1", "NaN", "inf"][..]),
        ("factory.build_guard.max_cpu_pressure_percent", &["-1", "101", "NaN"][..]),
        ("factory.build_guard.max_memory_pressure_percent", &["-1", "101", "inf"][..]),
        ("factory.build_guard.min_available_memory_gib", &["-1", "65537"][..]),
    ] {
        let before = config.get(key);
        for value in values {
            assert!(config.set(key, value).is_err(), "{key}={value}");
            assert_eq!(config.get(key), before);
        }
    }
}

#[test]
fn qa_capture_intent_and_runtime_pointer_findings_cas_db23() {
    // Every capture field is also checked after direct deserialization; hand
    // editing TOML must not bypass the config-set validators.
    for (name, invalid) in [
        ("base_url", serde_json::json!("file:///secret")),
        ("serve", serde_json::json!("pnpm dev")),
        ("ready_path", serde_json::json!("https://other.example")),
        ("shots", serde_json::json!("../credentials")),
        ("credentials_env", serde_json::json!("/tmp/credentials")),
        ("login_path", serde_json::json!("//other.example/login")),
        ("success_url", serde_json::json!("/dashboard")),
        ("email_sel", serde_json::json!("")),
        ("password_sel", serde_json::json!("")),
        ("submit_sel", serde_json::json!("")),
        ("mask", serde_json::json!([""])),
        ("viewports", serde_json::json!(["desktop"])),
        ("max_shots", serde_json::json!(7)),
        ("timeout_secs", serde_json::json!(241)),
        ("max_html_mb", serde_json::json!(9)),
    ] {
        let mut value = serde_json::to_value(QaCaptureConfig::default()).unwrap();
        value[name] = invalid;
        let capture: QaCaptureConfig = serde_json::from_value(value).unwrap();
        assert!(capture.validate().unwrap_err().contains(&format!("qa.capture.{name}")));
    }
    let mut env = TestEnvGuard::temp_home();
    env.remove("PROJECT_QA_CREDENTIALS");
    let mut config = Config::default();
    assert!(config.qa().capture.is_none());
    config.set("qa.capture.max_shots", "3").unwrap();
    assert!(!config.qa().capture.unwrap().is_configured().unwrap());
    config.set("qa.capture.base_url", "http://localhost:4312").unwrap();
    assert!(config.qa().capture.unwrap().is_configured().unwrap());
    config.set("qa.capture.credentials_env", "PROJECT_QA_CREDENTIALS").unwrap();
    assert!(config.qa().capture.unwrap().is_configured().unwrap_err().contains("unset or empty"));

    let missing_file = env.home().join("missing-file");
    env.set("PROJECT_QA_CREDENTIALS", &missing_file);
    assert!(config.qa().capture.unwrap().is_configured().unwrap_err().contains("existing readable file"));
    let directory = env.home().to_path_buf();
    env.set("PROJECT_QA_CREDENTIALS", &directory);
    assert!(config.qa().capture.unwrap().is_configured().is_err());
    let file = env.home().join("qa-account");
    // Contents need not be valid credentials: this seam only checks the pointer.
    std::fs::write(&file, b"opaque fixture bytes").unwrap();
    env.set("PROJECT_QA_CREDENTIALS", &file);
    assert_eq!(config.qa().capture.unwrap().credential_file().unwrap(), Some(file.clone()));
    assert!(config.qa().capture.unwrap().is_configured().unwrap());
    let serialized = toml::to_string(&config).unwrap();
    assert!(!serialized.contains(file.to_str().unwrap()));
    assert!(!serialized.contains("opaque fixture bytes"));

    config.set("qa.capture.login_path", "/login").unwrap();
    assert!(config.qa().capture.unwrap().is_configured().unwrap_err().contains("success_url"));
    config.set("qa.capture.success_url", "^https?://[^/]+/dashboard").unwrap();
    assert!(config.qa().capture.unwrap().is_configured().unwrap());
    config.set("qa.capture.credentials_env", "").unwrap();
    assert!(config.qa().capture.unwrap().is_configured().unwrap_err().contains("credentials_env is required"));
    config.set("qa.capture.base_url", "").unwrap();
    assert!(!config.qa().capture.unwrap().is_configured().unwrap());
    config.set("qa.capture.serve", "pnpm dev --port {port}").unwrap();
    assert!(config.qa().capture.unwrap().is_configured().is_err());
    config.set("qa.capture.login_path", "").unwrap();
    assert!(config.qa().capture.unwrap().is_configured().unwrap());
    config.set("qa.capture.serve", "").unwrap();
    assert!(!config.qa().capture.unwrap().is_configured().unwrap());
}

#[test]
fn qa_batch_default_preserves_explicit_per_task_round_trip() {
    let env = TestEnvGuard::temp_home();
    let project = env.home().join("project/.cas");
    std::fs::create_dir_all(&project).unwrap();
    assert_eq!(Config::default().qa().mode, QaMode::Batch);
    let mut config = Config::load(&project).unwrap();
    assert_eq!(config.get("qa.mode").as_deref(), Some("batch"));
    config.set("qa.mode", "per_task").unwrap();
    config.save(&project).unwrap();
    assert_eq!(Config::load(&project).unwrap().get("qa.mode").as_deref(), Some("per_task"));
}

#[test]
fn qa_user_defaults_apply_without_project_section_and_project_keys_win() {
    let env = TestEnvGuard::temp_home();
    let host = env.home().join(".cas");
    let project = env.home().join("project/.cas");
    std::fs::create_dir_all(&host).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(host.join("config.toml"), "[qa]\nmode = \"batch\"\nindependent_pass = false\nevidence_gate = false\n[hooks]\ncapture_enabled = false\n").unwrap();
    let mut loaded = Config::load(&project).unwrap();
    assert_eq!(loaded.get("qa.mode").as_deref(), Some("batch"));
    assert!(!loaded.qa().independent_pass);
    assert!(!loaded.qa().evidence_gate);
    assert!(loaded.hooks.is_none(), "only qa defaults are inherited");
    loaded.set("tasks.commit_nudge_on_close", "true").unwrap();
    loaded.save(&project).unwrap();
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(project.join("config.toml")).unwrap()).unwrap();
    assert!(saved.get("qa").is_none(), "saving another section must not pin inherited QA defaults");
    std::fs::write(host.join("config.toml"), "[qa]\nmode = \"per_task\"\nindependent_pass = false\nevidence_gate = false\n").unwrap();
    assert_eq!(Config::load(&project).unwrap().get("qa.mode").as_deref(), Some("per_task"));
    std::fs::write(project.join("config.toml"), "[qa]\nmode = \"batch\"\nindependent_pass = true\n").unwrap();
    let overridden = Config::load(&project).unwrap();
    assert_eq!(overridden.get("qa.mode").as_deref(), Some("batch"));
    assert!(overridden.qa().independent_pass);
    assert!(!overridden.qa().evidence_gate, "a project key does not mask other user defaults");
    let mut overridden = overridden;
    overridden.save(&project).unwrap();
    assert!(!Config::load(&project).unwrap().qa().evidence_gate);
    overridden.set("qa.evidence_gate", "true").unwrap();
    overridden.save(&project).unwrap();
    assert!(Config::load(&project).unwrap().qa().evidence_gate,
        "an explicit built-in value overrides a different user value");
    std::fs::write(host.join("config.toml"), "[qa]\nmode='per_task'\n[qa.capture]\nbase_url='http://localhost:3000'\n").unwrap();
    let nested = env.home().join("nested/.cas");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("config.yaml"), "qa:\n  independent_pass: false\n").unwrap();
    let mut loaded = Config::load(&nested).unwrap();
    loaded.set("qa.capture.ready_path", "/ready").unwrap();
    loaded.save(&nested).unwrap();
    let reloaded = Config::load(&nested).unwrap();
    assert_eq!(reloaded.qa().mode, QaMode::PerTask, "YAML migration must not pin the mode");
    let capture = reloaded.qa().capture.unwrap();
    assert_eq!(capture.base_url, "http://localhost:3000");
    assert_eq!(capture.ready_path, "/ready");
    assert!(!reloaded.qa().independent_pass);
    std::fs::write(host.join("config.toml"), "[qa]\ntelemetry_sweep='scripts/sweep.sh'\npreflight_env_files=['USER_ENV']\n").unwrap();
    let mut loaded = Config::load(&nested).unwrap();
    loaded.set("qa.telemetry_sweep", "").unwrap();
    loaded.set("qa.preflight_env_files", "").unwrap();
    loaded.save(&nested).unwrap();
    let reloaded = Config::load(&nested).unwrap();
    assert!(reloaded.qa().telemetry_sweep.is_none());
    assert!(reloaded.qa().preflight_env_files.is_empty());
}
