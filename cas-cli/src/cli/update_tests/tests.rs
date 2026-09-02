use crate::cli::update::*;

use std::path::{Path, PathBuf};

use crate::cli::Cli;

#[test]
fn test_is_newer() {
    assert!(is_newer("0.3.0", "0.2.0"));
    assert!(is_newer("0.2.1", "0.2.0"));
    assert!(is_newer("1.0.0", "0.9.9"));
    assert!(!is_newer("0.2.0", "0.2.0"));
    assert!(!is_newer("0.1.0", "0.2.0"));
    assert!(is_newer("v0.3.0", "0.2.0"));
    assert!(is_newer("0.3.0", "v0.2.0"));
}

#[test]
fn project_table_plain_uses_phase_glyphs_and_compact_project_names() {
    let receipts = vec![ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/demo"),
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Warning("busy".to_owned()),
        skills: ProjectPhase::Skipped("not installed".to_owned()),
        membership: ProjectPhase::Ok("2 memberships".to_owned()),
        cloud: ProjectPhase::Failed("timeout".to_owned()),
        details: String::new(),
        phase_details: Vec::new(),
    }];

    let output = render_project_table_plain(&receipts, false);

    assert!(output.contains("projects/demo"), "output was:\n{output}");
    assert!(output.contains("✓"), "output was:\n{output}");
    assert!(output.contains("⚠"), "output was:\n{output}");
    assert!(output.contains("✗"), "output was:\n{output}");
    assert!(output.contains("–"), "output was:\n{output}");
    assert!(!output.contains("/home/alice"), "output was:\n{output}");
}

#[test]
fn repeated_warnings_render_once_with_affected_project_count() {
    let mut warnings = RepeatedWarningCollector::default();
    warnings.record_builtin_paths("projects/one", [".claude/skills/cas-worker/SKILL.md"]);
    warnings.record_builtin_paths("projects/two", [".claude/skills/cas-worker/SKILL.md"]);
    warnings.record("Push incomplete; queued rows remain", "projects/one");
    warnings.record("Push incomplete; queued rows remain", "projects/two");

    let output = warnings.render(false);

    assert_eq!(
        output
            .matches("Cassy-managed builtin paths already tracked")
            .count(),
        1,
        "output was:\n{output}"
    );
    assert!(output.contains("2 projects"), "output was:\n{output}");
    assert_eq!(
        output
            .matches("Push incomplete; queued rows remain")
            .count(),
        1
    );
    assert_eq!(
        output.matches(".claude/skills/cas-worker/SKILL.md").count(),
        1
    );
}

#[test]
fn post_swap_command_targets_install_destination_path() {
    let install_destination = Path::new("/usr/local/bin/cas");
    let command = build_post_swap_command(install_destination, "3.7.7", true);

    assert_eq!(command.get_program(), "/usr/local/bin/cas");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["update", "--post-swap", "--from", "3.7.7", "--json"]);
}

#[test]
fn strip_deleted_suffix_from_linux_process_path() {
    assert_eq!(
        strip_deleted_suffix(PathBuf::from("/usr/local/bin/cas (deleted)")),
        PathBuf::from("/usr/local/bin/cas")
    );
    assert_eq!(
        strip_deleted_suffix(PathBuf::from("/usr/local/bin/cas")),
        PathBuf::from("/usr/local/bin/cas")
    );
}

#[test]
#[cfg(unix)]
fn post_swap_hook_invokes_the_installed_binary() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let installed_binary = temp_dir.path().join("cas-new");
    fs::write(
        &installed_binary,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n",
    )
    .expect("write fake installed binary");
    let mut permissions = fs::metadata(&installed_binary)
        .expect("stat fake installed binary")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&installed_binary, permissions).expect("make fake binary executable");

    run_post_swap_hook(&installed_binary, "3.7.7", true)
        .expect("post-swap hook should run successfully");

    let args = fs::read_to_string(installed_binary.with_extension("args"))
        .expect("fake binary should receive the post-swap arguments");
    assert_eq!(args, "update\n--post-swap\n--from\n3.7.7\n--json\n");
}

#[test]
fn post_swap_flags_parse_as_a_hidden_terminal_mode() {
    let parsed = crate::cli::try_parse_from_with_wordmark([
        "cas",
        "update",
        "--post-swap",
        "--from",
        "3.7.7",
    ])
    .expect("post-swap flags must parse");

    let Some(crate::cli::Commands::Update(args)) = parsed.command else {
        panic!("expected update command");
    };
    assert!(args.post_swap);
    assert_eq!(args.from.as_deref(), Some("3.7.7"));
}

#[test]
fn post_swap_mode_is_a_terminal_update_path() {
    let _guard = crate::test_support::TestEnvGuard::temp_home();
    let args = UpdateArgs {
        check: false,
        version: None,
        yes: false,
        schema_only: false,
        sync: false,
        user: false,
        dry_run: false,
        keep_backup: false,
        all_projects: false,
        post_swap: true,
        from: Some("3.7.7".to_owned()),
    };
    let cli = Cli {
        json: true,
        full: false,
        verbose: false,
        command: None,
    };

    execute(&args, &cli, None).expect("post-swap mode must not re-enter binary update");
}
