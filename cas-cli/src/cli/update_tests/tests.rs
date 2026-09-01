use crate::cli::update::*;

use std::path::Path;

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
fn post_swap_command_targets_installed_binary_and_passes_previous_version() {
    let command = build_post_swap_command(Path::new("/tmp/cas-new"), "3.7.7", true);

    assert_eq!(command.get_program(), "/tmp/cas-new");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["update", "--post-swap", "--from", "3.7.7", "--json"]);
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
