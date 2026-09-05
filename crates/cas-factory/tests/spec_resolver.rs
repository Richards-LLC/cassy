//! Unit tests for the `spec_resolver` cascade.
//!
//! Each test drives one or more cascade layers in isolation or in combination,
//! asserting that later layers overwrite earlier ones correctly.

use std::io::Write as _;

use cas_factory::{
    ConfigSources, resolve_specs, resolve_supervisor_spec, worker_slot_cli_configured,
    worker_slot_effort_configured,
};
use cas_mux::{Effort, SupervisorCli, WorkerSpec};
use tempfile::NamedTempFile;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Write `content` to a temp file and return it (kept alive by the caller).
fn toml_file(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

/// Path to a location that will never exist (for "skip this layer" semantics).
fn nonexistent_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/cas-spec-resolver-test-nonexistent-99999999")
}

/// Convenience: resolve with only the JSON overrides layer active.
fn resolve_json(workers: usize, jsons: &[&str]) -> Vec<WorkerSpec> {
    let sources = ConfigSources {
        user_config: Some(nonexistent_path()),
        project_config: None,
        worker_spec_jsons: jsons.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    resolve_specs(workers, sources).unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
// Layer 1: built-in defaults
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn defaults_only_produces_n_identical_claude_high_specs() {
    let specs = resolve_specs(
        3,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: None,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs.len(), 3);
    for spec in &specs {
        assert_eq!(spec.cli, SupervisorCli::Claude);
        assert_eq!(spec.model, None);
        assert_eq!(spec.effort, Some(Effort::High));
        assert_eq!(spec.name, None);
    }
}

#[test]
fn zero_workers_returns_empty_vec() {
    let specs = resolve_specs(0, ConfigSources::default()).unwrap();
    assert!(specs.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// Layer 2: user config
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn user_config_cli_codex_applies_to_all_specs() {
    let user = toml_file(
        r#"
[factory.defaults]
cli = "codex"
"#,
    );

    let specs = resolve_specs(
        2,
        ConfigSources {
            user_config: Some(user.path().to_path_buf()),
            project_config: None,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs.len(), 2);
    for spec in &specs {
        assert_eq!(
            spec.cli,
            SupervisorCli::Codex,
            "user config cli=codex should apply"
        );
        assert_eq!(spec.model, None);
        assert_eq!(spec.effort, Some(Effort::High)); // built-in default preserved
    }
}

#[test]
fn user_config_model_and_effort_apply_to_all_specs() {
    let user = toml_file(
        r#"
[factory.defaults]
model = "gpt-5.5"
effort = "low"
"#,
    );

    let specs = resolve_specs(
        2,
        ConfigSources {
            user_config: Some(user.path().to_path_buf()),
            project_config: None,
            ..Default::default()
        },
    )
    .unwrap();

    for spec in &specs {
        assert_eq!(spec.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(spec.effort, Some(Effort::Low));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Layer 3: project config [factory.defaults]
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn project_defaults_overrides_user_defaults() {
    let user = toml_file(
        r#"
[factory.defaults]
cli = "codex"
effort = "low"
"#,
    );
    let project = toml_file(
        r#"
[factory.defaults]
cli = "claude"
effort = "medium"
"#,
    );

    let specs = resolve_specs(
        2,
        ConfigSources {
            user_config: Some(user.path().to_path_buf()),
            project_config: Some(project.path().to_path_buf()),
            ..Default::default()
        },
    )
    .unwrap();

    for spec in &specs {
        assert_eq!(
            spec.cli,
            SupervisorCli::Claude,
            "project should override user cli"
        );
        assert_eq!(
            spec.effort,
            Some(Effort::Medium),
            "project should override user effort"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Layer 4: project [[factory.workers]] (per-position)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn project_per_worker_overrides_defaults_by_position() {
    let project = toml_file(
        r#"
[factory.defaults]
cli = "claude"

[[factory.workers]]
name = "alice"
cli = "codex"
model = "gpt-5.5"

[[factory.workers]]
name = "bob"
effort = "minimal"
"#,
    );

    let specs = resolve_specs(
        3,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: Some(project.path().to_path_buf()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs.len(), 3);

    // Slot 0: alice — cli + model from [[workers]], effort from [defaults] (High)
    assert_eq!(specs[0].name.as_deref(), Some("alice"));
    assert_eq!(specs[0].cli, SupervisorCli::Codex);
    assert_eq!(specs[0].model.as_deref(), Some("gpt-5.5"));
    assert_eq!(specs[0].effort, Some(Effort::High));

    // Slot 1: bob — cli from [defaults], effort from [[workers]]
    assert_eq!(specs[1].name.as_deref(), Some("bob"));
    assert_eq!(specs[1].cli, SupervisorCli::Claude);
    assert_eq!(specs[1].effort, Some(Effort::Minimal));

    // Slot 2: no [[workers]] entry — pure defaults
    assert_eq!(specs[2].name, None);
    assert_eq!(specs[2].cli, SupervisorCli::Claude);
    assert_eq!(specs[2].effort, Some(Effort::High));
}

#[test]
fn parsed_config_detection_reports_defaults_and_per_worker_fields() {
    let user = toml_file(
        r#"
[factory.defaults]
cli = "codex"
"#,
    );
    let project = toml_file(
        r#"
[[factory.workers]]
name = "slot-zero"

[[factory.workers]]
name = "slot-one"
effort = "high"
"#,
    );
    let sources = ConfigSources {
        user_config: Some(user.path().to_path_buf()),
        project_config: Some(project.path().to_path_buf()),
        ..Default::default()
    };

    assert!(worker_slot_cli_configured(0, &sources).unwrap());
    assert!(!worker_slot_effort_configured(0, &sources).unwrap());
    assert!(worker_slot_effort_configured(1, &sources).unwrap());
}

#[test]
fn project_workers_exceeding_slot_count_are_ignored() {
    // 3 [[workers]] entries but only 2 slots — no panic, extra entries dropped.
    let project = toml_file(
        r#"
[[factory.workers]]
name = "a"

[[factory.workers]]
name = "b"

[[factory.workers]]
name = "c"
"#,
    );

    let specs = resolve_specs(
        2,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: Some(project.path().to_path_buf()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].name.as_deref(), Some("a"));
    assert_eq!(specs[1].name.as_deref(), Some("b"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Layer 5: CLI flags
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cli_flag_overrides_project_config() {
    let project = toml_file(
        r#"
[factory.defaults]
cli = "codex"
effort = "low"
"#,
    );

    let specs = resolve_specs(
        2,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: Some(project.path().to_path_buf()),
            cli_flag: Some(SupervisorCli::Claude),
            effort_flag: Some(Effort::Medium),
            ..Default::default()
        },
    )
    .unwrap();

    for spec in &specs {
        assert_eq!(
            spec.cli,
            SupervisorCli::Claude,
            "CLI flag overrides project cli"
        );
        assert_eq!(
            spec.effort,
            Some(Effort::Medium),
            "CLI flag overrides project effort"
        );
    }
}

#[test]
fn cli_model_flag_overrides_project_model() {
    let project = toml_file(
        r#"
[factory.defaults]
model = "gpt-5.5"
"#,
    );

    let specs = resolve_specs(
        1,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: Some(project.path().to_path_buf()),
            model_flag: Some("claude-opus-4-5".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs[0].model.as_deref(), Some("claude-opus-4-5"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Layer 6: --worker-spec JSON overrides
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn worker_spec_named_replaces_matching_slot() {
    // Project gives slot 0 the name "alice" with cli=claude.
    let project = toml_file(
        r#"
[[factory.workers]]
name = "alice"
cli = "claude"
"#,
    );

    let specs = resolve_specs(
        2,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: Some(project.path().to_path_buf()),
            worker_spec_jsons: vec![r#"{"name":"alice","cli":"codex"}"#.to_string()],
            ..Default::default()
        },
    )
    .unwrap();

    // alice's slot is updated to codex.
    assert_eq!(specs[0].name.as_deref(), Some("alice"));
    assert_eq!(specs[0].cli, SupervisorCli::Codex);
    // bob is untouched.
    assert_eq!(specs[1].cli, SupervisorCli::Claude);
}

#[test]
fn worker_spec_named_without_existing_slot_takes_cursor_position() {
    // No [[workers]] in project — all slots unnamed.
    // --worker-spec with name="alice" should take slot 0.
    let specs = resolve_json(2, &[r#"{"name":"alice","cli":"codex"}"#]);

    assert_eq!(specs[0].name.as_deref(), Some("alice"));
    assert_eq!(specs[0].cli, SupervisorCli::Codex);
    // Slot 1 unchanged.
    assert_eq!(specs[1].name, None);
    assert_eq!(specs[1].cli, SupervisorCli::Claude);
}

#[test]
fn two_worker_spec_overrides_apply_independently() {
    let specs = resolve_json(
        3,
        &[
            r#"{"name":"alice","cli":"codex"}"#,
            r#"{"name":"bob","model":"gpt-5.5","effort":"low"}"#,
        ],
    );

    assert_eq!(specs.len(), 3);

    // slot 0 → alice
    assert_eq!(specs[0].name.as_deref(), Some("alice"));
    assert_eq!(specs[0].cli, SupervisorCli::Codex);
    assert_eq!(specs[0].model, None);

    // slot 1 → bob
    assert_eq!(specs[1].name.as_deref(), Some("bob"));
    assert_eq!(specs[1].cli, SupervisorCli::Claude); // no cli override
    assert_eq!(specs[1].model.as_deref(), Some("gpt-5.5"));
    assert_eq!(specs[1].effort, Some(Effort::Low));

    // slot 2 → untouched defaults
    assert_eq!(specs[2].name, None);
    assert_eq!(specs[2].cli, SupervisorCli::Claude);
}

#[test]
fn worker_spec_unnamed_takes_cursor_slots_sequentially() {
    let specs = resolve_json(3, &[r#"{"cli":"codex"}"#, r#"{"effort":"minimal"}"#]);

    assert_eq!(specs[0].cli, SupervisorCli::Codex);
    assert_eq!(specs[1].effort, Some(Effort::Minimal));
    assert_eq!(specs[2].cli, SupervisorCli::Claude); // untouched
}

#[test]
fn worker_spec_overrides_beyond_slot_count_are_silently_dropped() {
    // 2 slots, 3 --worker-spec overrides → last one is ignored.
    let specs = resolve_json(
        2,
        &[
            r#"{"cli":"codex"}"#,
            r#"{"effort":"low"}"#,
            r#"{"model":"gpt-5.5"}"#,
        ],
    );

    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].cli, SupervisorCli::Codex);
    assert_eq!(specs[1].effort, Some(Effort::Low));
}

#[test]
fn invalid_json_worker_spec_returns_clear_error() {
    let result = resolve_specs(
        1,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            worker_spec_jsons: vec!["not-json".to_string()],
            ..Default::default()
        },
    );

    assert!(result.is_err(), "invalid JSON should produce an error");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("--worker-spec"),
        "error message should mention --worker-spec; got: {msg}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Cascade interaction: full 5-layer stack
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn full_cascade_later_layers_win() {
    let user = toml_file(
        r#"
[factory.defaults]
cli = "codex"
effort = "low"
"#,
    );
    let project = toml_file(
        r#"
[factory.defaults]
effort = "medium"

[[factory.workers]]
name = "w0"
cli = "codex"
"#,
    );

    let specs = resolve_specs(
        2,
        ConfigSources {
            user_config: Some(user.path().to_path_buf()),
            project_config: Some(project.path().to_path_buf()),
            // CLI flag overrides project effort.
            effort_flag: Some(Effort::XHigh),
            // JSON override overrides CLI flag for cli on slot 0.
            worker_spec_jsons: vec![r#"{"name":"w0","cli":"claude"}"#.to_string()],
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs.len(), 2);

    // Slot 0 (w0): user=codex, project=codex, cli=unchanged, json=claude → claude wins
    assert_eq!(specs[0].name.as_deref(), Some("w0"));
    assert_eq!(specs[0].cli, SupervisorCli::Claude);
    assert_eq!(specs[0].effort, Some(Effort::XHigh)); // CLI flag

    // Slot 1: user=codex, project defaults to codex (from user), CLI no override, json none
    assert_eq!(specs[1].cli, SupervisorCli::Codex); // from user, not overridden at project
    assert_eq!(specs[1].effort, Some(Effort::XHigh)); // CLI flag
}

// ─────────────────────────────────────────────────────────────────────────────
// Backwards compat: existing global worker_cli → N identical specs
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn global_cli_flag_fans_out_to_all_specs() {
    // This is the existing behavior: a single global --worker-cli applies to
    // every worker.  Callers that previously used worker_cli = Codex continue
    // to work by passing cli_flag = Some(Codex).
    let specs = resolve_specs(
        4,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            cli_flag: Some(SupervisorCli::Codex),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs.len(), 4);
    for spec in &specs {
        assert_eq!(spec.cli, SupervisorCli::Codex);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge cases / robustness
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn missing_user_config_file_is_skipped_silently() {
    // user_config points at a non-existent path → no error, built-in defaults used.
    let specs = resolve_specs(
        1,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: None,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs[0].cli, SupervisorCli::Claude);
    assert_eq!(specs[0].effort, Some(Effort::High));
}

#[test]
fn empty_project_config_file_is_harmless() {
    let project = toml_file(""); // no [factory] section at all
    let specs = resolve_specs(
        1,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: Some(project.path().to_path_buf()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(specs[0].cli, SupervisorCli::Claude);
    assert_eq!(specs[0].effort, Some(Effort::High));
}

#[test]
fn invalid_cli_value_in_toml_returns_error() {
    let project = toml_file(
        r#"
[factory.defaults]
cli = "gpt"
"#,
    );

    let result = resolve_specs(
        1,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: Some(project.path().to_path_buf()),
            ..Default::default()
        },
    );

    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("gpt"),
        "error should name the bad value; got: {msg}"
    );
}

#[test]
fn invalid_effort_value_in_toml_returns_error() {
    let project = toml_file(
        r#"
[factory.defaults]
effort = "extreme"
"#,
    );

    let result = resolve_specs(
        1,
        ConfigSources {
            user_config: Some(nonexistent_path()),
            project_config: Some(project.path().to_path_buf()),
            ..Default::default()
        },
    );

    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("extreme"),
        "error should name the bad value; got: {msg}"
    );
}

#[test]
fn all_effort_variants_roundtrip_through_toml() {
    for (toml_val, expected) in [
        ("minimal", Effort::Minimal),
        ("low", Effort::Low),
        ("medium", Effort::Medium),
        ("high", Effort::High),
        ("xhigh", Effort::XHigh),
    ] {
        let project = toml_file(&format!("[factory.defaults]\neffort = \"{toml_val}\"\n"));

        let specs = resolve_specs(
            1,
            ConfigSources {
                user_config: Some(nonexistent_path()),
                project_config: Some(project.path().to_path_buf()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            specs[0].effort,
            Some(expected),
            "effort {toml_val:?} failed to round-trip"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// resolve_supervisor_spec — cas-1948
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn supervisor_spec_defaults_to_supervisor_lane() {
    let spec = resolve_supervisor_spec(ConfigSources {
        user_config: Some(nonexistent_path()),
        project_config: None,
        ..Default::default()
    })
    .unwrap();

    assert_eq!(spec.cli, SupervisorCli::Claude);
    assert_eq!(spec.model.as_deref(), Some("claude-fable-5-1"));
    assert_eq!(spec.effort, Some(Effort::Medium));
}

#[test]
fn supervisor_spec_factory_supervisor_toml_overrides_defaults() {
    // [factory.defaults] sets codex/low; [factory.supervisor] overrides to claude/xhigh.
    let project = toml_file(
        r#"
[factory.defaults]
cli = "codex"
effort = "low"

[factory.supervisor]
cli = "claude"
effort = "xhigh"
"#,
    );

    let spec = resolve_supervisor_spec(ConfigSources {
        user_config: Some(nonexistent_path()),
        project_config: Some(project.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(
        spec.cli,
        SupervisorCli::Claude,
        "[factory.supervisor] cli should win"
    );
    assert_eq!(
        spec.effort,
        Some(Effort::XHigh),
        "[factory.supervisor] effort should win"
    );
}

#[test]
fn supervisor_spec_factory_supervisor_toml_model_override() {
    let project = toml_file(
        r#"
[factory.supervisor]
model = "claude-opus-5"
"#,
    );

    let spec = resolve_supervisor_spec(ConfigSources {
        user_config: Some(nonexistent_path()),
        project_config: Some(project.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(spec.model.as_deref(), Some("claude-opus-5"));
}

#[test]
fn supervisor_spec_cli_flag_overrides_toml_supervisor() {
    let project = toml_file(
        r#"
[factory.supervisor]
cli = "codex"
effort = "minimal"
"#,
    );

    let spec = resolve_supervisor_spec(ConfigSources {
        user_config: Some(nonexistent_path()),
        project_config: Some(project.path().to_path_buf()),
        // CLI flag (--supervisor-cli claude) overrides [factory.supervisor]
        cli_flag: Some(SupervisorCli::Claude),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(
        spec.cli,
        SupervisorCli::Claude,
        "CLI flag must override [factory.supervisor]"
    );
    // effort from toml is preserved (cli_flag doesn't affect effort)
    assert_eq!(spec.effort, Some(Effort::Minimal));
}

#[test]
fn supervisor_spec_json_override_wins_over_all_layers() {
    let project = toml_file(
        r#"
[factory.supervisor]
cli = "codex"
model = "gpt-5.5"
effort = "minimal"
"#,
    );

    let spec = resolve_supervisor_spec(ConfigSources {
        user_config: Some(nonexistent_path()),
        project_config: Some(project.path().to_path_buf()),
        cli_flag: Some(SupervisorCli::Codex),
        // --supervisor-spec JSON is layer 6 (wins over everything)
        supervisor_spec_json: Some(r#"{"cli":"claude","effort":"xhigh"}"#.to_string()),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(
        spec.cli,
        SupervisorCli::Claude,
        "--supervisor-spec JSON should win"
    );
    assert_eq!(
        spec.effort,
        Some(Effort::XHigh),
        "--supervisor-spec JSON should win"
    );
    // model not in JSON → falls through from [factory.supervisor]
    assert_eq!(spec.model.as_deref(), Some("gpt-5.5"));
}

#[test]
fn supervisor_spec_invalid_json_returns_error() {
    let result = resolve_supervisor_spec(ConfigSources {
        user_config: Some(nonexistent_path()),
        supervisor_spec_json: Some("not-json".to_string()),
        ..Default::default()
    });

    assert!(result.is_err(), "invalid JSON should produce an error");
}

#[test]
fn supervisor_spec_factory_workers_toml_does_not_affect_supervisor() {
    // [[factory.workers]] must not bleed into the supervisor spec.
    let project = toml_file(
        r#"
[[factory.workers]]
cli = "codex"
effort = "minimal"
"#,
    );

    let spec = resolve_supervisor_spec(ConfigSources {
        user_config: Some(nonexistent_path()),
        project_config: Some(project.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();

    // Supervisor should see the supervisor lane default, NOT the worker TOML.
    assert_eq!(
        spec.cli,
        SupervisorCli::Claude,
        "[[factory.workers]] must not affect supervisor"
    );
    assert_eq!(
        spec.effort,
        Some(Effort::Medium),
        "[[factory.workers]] must not affect supervisor"
    );
    assert_eq!(spec.model.as_deref(), Some("claude-fable-5-1"));
}
