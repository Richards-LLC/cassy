use crate::cli::update::*;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};

use crate::cli::Cli;
use crate::ui::components::OutputMode;

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
        unregistered: false,
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
    let row = output.lines().nth(1).expect("project table row");
    assert!(
        row.contains("timeout"),
        "most severe phase reason missing:\n{output}"
    );
}

#[test]
fn project_table_phase_headers_are_not_truncated_at_supported_widths() {
    let receipts = vec![ProjectRefreshReceipt {
        project: PathBuf::from(
            "/home/alice/projects/a-project-with-a-deliberately-long-name/another-project",
        ),
        unregistered: false,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Ok("up to date".to_owned()),
        skills: ProjectPhase::Ok("up to date".to_owned()),
        membership: ProjectPhase::Ok("up to date".to_owned()),
        cloud: ProjectPhase::Ok("up to date".to_owned()),
        details: String::new(),
        phase_details: Vec::new(),
    }];

    for mode in [OutputMode::Plain, OutputMode::Styled] {
        for width in [80, 120] {
            let output = render_project_table_at_width(&receipts, false, mode, width);
            let header = output.lines().next().expect("project table header");
            for truncated in ["migra…", "inde…", "skill…", "membe…", "clou…"] {
                assert!(
                    !header.contains(truncated),
                    "phase header truncated at {width} columns in {mode:?}: {header:?}"
                );
            }
            for label in ["project", "migr", "index", "skills", "member", "cloud"] {
                assert!(
                    header.contains(label),
                    "missing {label:?} header at {width} columns in {mode:?}: {header:?}"
                );
            }
        }
    }
}

#[test]
fn non_ok_phase_details_include_phase_summary_when_capture_is_empty() {
    let receipt = ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/demo"),
        unregistered: false,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Ok("up to date".to_owned()),
        skills: ProjectPhase::Ok("up to date".to_owned()),
        membership: ProjectPhase::Ok("up to date".to_owned()),
        cloud: ProjectPhase::Warning("12 queued".to_owned()),
        details: String::new(),
        phase_details: vec![
            (true, String::new()),
            (true, String::new()),
            (true, String::new()),
            (true, String::new()),
            (false, String::new()),
        ],
    };
    let mut warnings = RepeatedWarningCollector::default();

    let detail = render_project_phase_details(&receipt, false, &mut warnings, "projects/demo");

    assert!(
        detail.contains("[WARN] cloud: 12 queued"),
        "cloud phase summary missing from detail output: {detail:?}"
    );
}

#[test]
fn cloud_warning_summary_stays_under_the_non_ok_project_row() {
    let receipt = ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/demo"),
        unregistered: false,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Ok("up to date".to_owned()),
        skills: ProjectPhase::Ok("up to date".to_owned()),
        membership: ProjectPhase::Ok("up to date".to_owned()),
        cloud: ProjectPhase::Warning("4 queued".to_owned()),
        details: String::new(),
        phase_details: vec![
            (true, String::new()),
            (true, String::new()),
            (true, String::new()),
            (true, String::new()),
            (false, "[WARN] Push incomplete · 4 pending\n".to_owned()),
        ],
    };
    let mut warnings = RepeatedWarningCollector::default();

    let detail = render_project_phase_details(&receipt, false, &mut warnings, "projects/demo");

    assert!(
        detail.contains("[WARN] Push incomplete · 4 pending"),
        "cloud warning summary missing from detail output: {detail:?}"
    );
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

#[cfg(unix)]
#[test]
fn capture_phase_collects_both_stdout_and_stderr() {
    // OutputCapture redirects process-global descriptors. Keep this check
    // isolated from the other unit tests so their harness output cannot be
    // mistaken for phase output while the descriptors are redirected.
    static CAPTURE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = CAPTURE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("capture test lock should not be poisoned");

    let (value, output) = capture_phase(true, || {
        let stdout = b"captured stdout\n";
        let stderr = b"captured stderr\n";
        unsafe {
            let _ = libc::write(libc::STDOUT_FILENO, stdout.as_ptr().cast(), stdout.len());
            let _ = libc::write(libc::STDERR_FILENO, stderr.as_ptr().cast(), stderr.len());
        }
        42
    });

    assert_eq!(value, 42);
    assert!(output.contains("captured stdout"), "output was: {output:?}");
    assert!(output.contains("captured stderr"), "output was: {output:?}");
}

/// Create a directory that looks exactly like an initialized Cassy project:
/// a `.cas/` directory holding a `cas.db` file.
fn make_project(root: &Path) {
    let cas_root = root.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("create fixture .cas directory");
    std::fs::write(cas_root.join("cas.db"), b"").expect("create fixture store");
}

#[test]
fn discovery_finds_sibling_projects_below_a_parent_that_is_itself_a_project() {
    // The regression: `~/.cas` exists on every real host, and the scan used to
    // stop at the first ancestor carrying a `.cas` directory. That made the
    // filesystem walk a no-op — home was recorded, the walk returned, and home
    // was then dropped as "not a project", so nothing was ever discovered.
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    let workspace = home.join("Workspace");
    make_project(&workspace);
    make_project(&workspace.join("alpha"));
    make_project(&workspace.join("beta"));
    make_project(&workspace.join("nested").join("deep"));

    let mut scanned = BTreeSet::new();
    scan_for_projects(&home, MAX_SCAN_DEPTH, &mut scanned);

    for expected in [
        workspace.clone(),
        workspace.join("alpha"),
        workspace.join("beta"),
        workspace.join("nested").join("deep"),
    ] {
        assert!(
            scanned.contains(&expected),
            "{} missing from discovery: {:?}",
            expected.display(),
            scanned
        );
    }
    assert!(
        scanned.contains(&home),
        "the scanner should see home before discovery classifies host state"
    );
}

#[test]
fn discovery_never_returns_the_user_level_store_as_a_project() {
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    make_project(&home.join("Workspace"));

    let discovery = discover_local_projects(Some(&home.join(".cas")));

    assert_eq!(user_level_store_root(), Some(home.join(".cas")));
    assert!(
        !discovery.projects.contains(&home),
        "the user-level store must never be counted as a project: {:?}",
        discovery.projects
    );
}

#[test]
fn discovery_does_not_descend_into_cas_internal_directories() {
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    let project = home.join("Workspace").join("demo");
    make_project(&project);
    let worktree = project.join(".cas").join("worktrees").join("lane-1");
    make_project(&worktree);
    let backup = project.join(".cas").join("backup").join("20260904_000000");
    std::fs::create_dir_all(&backup).expect("create fixture backup");
    std::fs::write(backup.join("cas.db"), b"").expect("create fixture backup store");

    let mut scanned = BTreeSet::new();
    scan_for_projects(&home, MAX_SCAN_DEPTH, &mut scanned);

    assert!(scanned.contains(&project));
    assert!(
        !scanned.contains(&worktree),
        "factory worktrees under .cas/ are not separate projects: {:?}",
        scanned
    );
    assert!(
        !scanned.iter().any(|path| path.starts_with(&backup)),
        "migration backups under .cas/ are not projects: {:?}",
        scanned
    );
}

#[test]
fn discovery_separates_registered_projects_from_scan_only_and_storeless_ones() {
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    let workspace = home.join("Workspace");
    let with_store = workspace.join("with-store");
    make_project(&with_store);
    let storeless = workspace.join("storeless");
    std::fs::create_dir_all(storeless.join(".cas")).expect("create storeless fixture");
    crate::store::known_repos::ensure_host_schema().expect("bootstrap known-repos schema");
    crate::store::known_repos::register_repo_strict(&with_store)
        .expect("register temp-root fixture");

    let discovery = discover_local_projects(None);

    assert!(
        !discovery.projects.contains(&with_store),
        "a temp-root project must not be refreshed by host discovery: {:?}",
        discovery.projects
    );
    assert!(
        discovery
            .skipped_unregistered
            .iter()
            .any(|skip| skip.project == with_store && skip.reason.contains("temp")),
        "a registered temp-root project must be reported with its exclusion reason: {:?}",
        discovery.skipped_unregistered
    );
    assert!(
        !discovery.unregistered.contains(&with_store),
        "a registered temp-root project must not be relabeled as scan-only: {:?}",
        discovery.unregistered
    );
    assert!(
        discovery
            .skipped_unregistered
            .iter()
            .any(|skip| skip.project == storeless),
        "a scan-only directory with no cas.db must be listed, not silently dropped: {:?}",
        discovery.skipped_unregistered
    );
    assert!(
        !discovery.projects.contains(&storeless),
        "a directory with no store has nothing to migrate: {:?}",
        discovery.projects
    );
}

#[test]
fn schema_status_json_names_the_store_it_migrated() {
    let line = schema_status_json(
        Some(Path::new("/home/alice/projects/demo/.cas")),
        r#""schema_status":"updated","current_version":248"#,
    );

    let parsed: serde_json::Value = serde_json::from_str(&line).expect("schema status is JSON");
    assert_eq!(parsed["store"], "/home/alice/projects/demo/.cas");
    assert_eq!(parsed["schema_status"], "updated");
    assert_eq!(parsed["current_version"], 248);

    let unset = schema_status_json(None, r#""schema_status":"not_initialized""#);
    let parsed: serde_json::Value = serde_json::from_str(&unset).expect("schema status is JSON");
    assert!(parsed["store"].is_null(), "line was: {unset}");
}

#[test]
fn update_banner_reports_projects_failures_and_skipped_unregistered_stores() {
    let banner = update_banner_text(&RefreshReport {
        project_count: 29,
        failed_count: 1,
        skipped_unregistered: 2,
        elapsed: std::time::Duration::from_secs(3),
    });

    assert!(banner.contains("29 projects refreshed"), "banner: {banner}");
    assert!(banner.contains("1 failed"), "banner: {banner}");
    assert!(
        banner.contains("2 unregistered store(s) not refreshed"),
        "banner: {banner}"
    );

    let clean = update_banner_text(&RefreshReport {
        project_count: 29,
        failed_count: 0,
        skipped_unregistered: 0,
        elapsed: std::time::Duration::from_secs(3),
    });
    assert!(
        !clean.contains("unregistered"),
        "a clean run stays quiet: {clean}"
    );
}

#[test]
fn refresh_receipt_names_each_skipped_project_and_why_it_was_not_refreshed() {
    let receipt = project_refresh_receipt_json(
        &[],
        &ProjectPhase::Ok("up to date".to_string()),
        &[SkippedProject {
            project: PathBuf::from("/tmp/container-copy"),
            reason: "its .cas has no [project] canonical_id pin and no git origin remote".to_string(),
        }],
    );

    assert_eq!(receipt["skipped_unregistered"][0]["project"], "/tmp/container-copy");
    assert!(receipt["skipped_unregistered"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("no git origin remote"));
}

#[test]
fn register_flag_parses_and_targets_a_project_path() {
    let parsed = crate::cli::try_parse_from_with_wordmark([
        "cas",
        "update",
        "--register",
        "/home/alice/projects/demo",
    ])
    .expect("--register must parse");

    let Some(crate::cli::Commands::Update(args)) = parsed.command else {
        panic!("expected update command");
    };
    assert_eq!(
        args.register.as_deref(),
        Some(Path::new("/home/alice/projects/demo"))
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
        register: None,
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

// =============================================================================
// cas-91ba: the post-install phases must run on the NEWLY installed binary.
// Installing 3.15.2 from 3.15.1 refreshed with the 3.15.1 image: 16 projects
// instead of 43, no user_level_store, and gabber-studio's ledger wedge still
// reported — a second `cas update` was needed to converge.
// =============================================================================

#[cfg(unix)]
fn write_stub_binary(path: &Path, body: &str) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, body).expect("write stub binary");
    let mut permissions = fs::metadata(path).expect("stat stub binary").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make stub binary executable");
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_runs_on_the_installed_binary_and_reports_its_version() {
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let installed_binary = temp_dir.path().join("cas-new");
    // The stub stands in for the freshly installed binary: it records its argv
    // and prints the refresh receipt the real child would print.
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n\
         printf '%s' '{\"refresh_binary_version\":\"9.9.9-stub\",\"projects\":[],\"user_level_store\":{\"status\":\"ok\"},\"skipped_unregistered\":[]}'\n",
    );

    let receipt = run_post_swap_refresh(&installed_binary, "3.15.1", "9.9.9-stub", true)
        .expect("the post-swap refresh must run the installed binary");

    assert_eq!(
        receipt["refresh_binary_version"], "9.9.9-stub",
        "the receipt must name the binary that actually refreshed: {receipt}"
    );
    let args = std::fs::read_to_string(installed_binary.with_extension("args"))
        .expect("stub binary should have recorded its arguments");
    assert!(args.contains("--post-swap"), "{args}");
    assert!(args.contains("3.15.1"), "{args}");
    assert!(args.contains("--json"), "{args}");
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_failure_tells_the_operator_to_run_update_again() {
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let missing = temp_dir.path().join("cas-not-installed");

    let error = run_post_swap_refresh(&missing, "3.15.1", "3.15.2", true)
        .expect_err("an unusable installed binary must not read as a successful refresh");
    let message = format!("{error:#}");
    assert!(
        message.contains("cas update"),
        "the operator must be told to run cas update again: {message}"
    );
}

#[test]
fn refresh_receipt_names_the_binary_that_ran_it() {
    let receipt = project_refresh_receipt_json(&[], &ProjectPhase::Ok(String::new()), &[]);

    assert_eq!(
        receipt["refresh_binary_version"],
        env!("CARGO_PKG_VERSION"),
        "every refresh receipt must name the binary version that produced it: {receipt}"
    );
}

#[test]
fn combined_receipt_merges_the_installed_binary_refresh_into_one_document() {
    let refresh = serde_json::json!({
        "refresh_binary_version": "3.15.2",
        "projects": [],
        "user_level_store": {"status": "ok"},
    });

    let combined = combined_update_receipt("3.15.2", true, Some(&refresh));
    assert_eq!(combined["binary_updated"], true);
    assert_eq!(combined["version"], "3.15.2");
    assert_eq!(
        combined["refresh_binary_version"], "3.15.2",
        "the single receipt must state which image ran the refresh: {combined}"
    );
    assert!(combined["user_level_store"].is_object(), "{combined}");

    // No swap: no refresh receipt to merge, and no stale version claimed.
    let solo = combined_update_receipt("3.15.2", false, None);
    assert_eq!(solo["binary_updated"], false);
    assert!(solo.get("refresh_binary_version").is_none(), "{solo}");
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_rejects_a_child_reporting_a_different_version() {
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let installed_binary = temp_dir.path().join("cas-stale");
    // A stale `cas` answering instead of the binary we just installed.
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh\nprintf '%s' '{\"refresh_binary_version\":\"3.15.1\",\"projects\":[]}'\n",
    );

    let error = run_post_swap_refresh(&installed_binary, "3.15.0", "3.15.2", true)
        .expect_err("a refresh performed by the wrong version must not read as converged");
    let message = format!("{error:#}");
    assert!(message.contains("3.15.1") && message.contains("3.15.2"), "{message}");
    assert!(message.contains("cas update"), "{message}");
}

#[test]
fn version_verification_accepts_only_the_installed_version() {
    let binary = Path::new("/usr/local/bin/cas");
    assert!(verify_refresh_binary_version(Some("3.15.2"), "3.15.2", binary).is_ok());
    assert!(verify_refresh_binary_version(Some(" 3.15.2 "), "3.15.2", binary).is_ok());
    assert!(verify_refresh_binary_version(Some("3.15.1"), "3.15.2", binary).is_err());
    assert!(
        verify_refresh_binary_version(None, "3.15.2", binary).is_err(),
        "a child that reports no version proves nothing"
    );
}

#[test]
fn reported_version_is_the_semver_not_the_build_date() {
    // The real shapes `cas --version` prints.
    assert_eq!(
        parse_reported_version("cas 3.15.5 (a94b6ac 2026-09-04)\n").as_deref(),
        Some("3.15.5")
    );
    assert_eq!(
        parse_reported_version("cas 2.27.0 (9b52e17-dirty 2026-07-16)\n").as_deref(),
        Some("2.27.0")
    );
    assert_eq!(parse_reported_version("cas 3.15.5\n").as_deref(), Some("3.15.5"));
    assert_eq!(parse_reported_version("v3.15.5").as_deref(), Some("3.15.5"));
    assert_eq!(
        parse_reported_version("cas 9.99.0-rc.1 (a94b6ac 2026-09-04)").as_deref(),
        Some("9.99.0-rc.1")
    );

    // Nothing that is a version means we cannot vouch for the child.
    assert_eq!(parse_reported_version("cas (a94b6ac 2026-09-04)"), None);
    assert_eq!(parse_reported_version(""), None);
    assert_eq!(parse_reported_version("2026-09-04"), None);
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_accepts_the_real_version_line_and_still_rejects_a_stale_binary() {
    let temp_dir = tempfile::tempdir().expect("create post-swap version test directory");

    // The interactive path asks the installed binary for its version.
    let installed_binary = temp_dir.path().join("cas-current");
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then \
         printf 'cas 3.15.5 (a94b6ac 2026-09-04)\\n'; fi\nexit 0\n",
    );
    run_post_swap_refresh(&installed_binary, "3.15.4", "3.15.5", false)
        .expect("a correct refresh under the installed binary must read as converged");

    let stale_binary = temp_dir.path().join("cas-stale");
    write_stub_binary(
        &stale_binary,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then \
         printf 'cas 3.15.4 (0000000 2026-09-03)\\n'; fi\nexit 0\n",
    );
    let error = run_post_swap_refresh(&stale_binary, "3.15.4", "3.15.5", false)
        .expect_err("a refresh performed by the wrong version must not read as converged");
    let message = format!("{error:#}");
    assert!(message.contains("3.15.4") && message.contains("3.15.5"), "{message}");
}

#[test]
fn skipped_refresh_receipt_says_the_update_did_not_converge() {
    let receipt = skipped_refresh_receipt(
        "3.15.2",
        "binary updated to 3.15.2; refresh did not run — run `cas update` again",
    );

    assert_eq!(receipt["binary_updated"], true);
    assert_eq!(receipt["version"], "3.15.2");
    assert!(
        receipt["refresh_binary_version"].is_null(),
        "nothing refreshed, so no version may be claimed: {receipt}"
    );
    assert_eq!(receipt["refresh_status"], "skipped");
    assert!(
        receipt["message"].as_str().unwrap().contains("cas update"),
        "{receipt}"
    );
}
