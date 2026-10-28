//! cas-e1f4: one shared base folder for every account, on fixture homes.
use super::*;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn login(dir: &Path, email: &str) {
    write(&dir.join(".credentials.json"), "{\"claudeAiOauth\":{}}");
    write(
        &dir.join(".claude.json"),
        &format!("{{\"oauthAccount\":{{\"emailAddress\":\"{email}\"}}}}"),
    );
}

#[test]
fn discover_follows_directory_symlinks_and_skips_unreadable_entries() {
    let home = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    for provider in Provider::ALL {
        let linked = home.path().join(format!("{}linked", provider.prefix()));
        std::os::unix::fs::symlink(target.path(), &linked).unwrap();
        let broken = home.path().join(format!("{}broken", provider.prefix()));
        std::os::unix::fs::symlink(home.path().join("missing"), broken).unwrap();
        let unreadable = home.path().join(format!("{}loop", provider.prefix()));
        std::os::unix::fs::symlink(&unreadable, &unreadable).unwrap();
        write(&home.path().join(format!("{}not-a-directory", provider.prefix())), "file");
        std::os::unix::fs::symlink(target.path(), home.path().join(format!("{}linked-copy", provider.prefix()))).unwrap();
        let inside = home.path().join(format!("{}target", provider.name()));
        std::fs::create_dir_all(&inside).unwrap();
        for alias in ["inside", "inside-copy"] {
            std::os::unix::fs::symlink(&inside, home.path().join(format!("{}{alias}", provider.prefix()))).unwrap();
        }
        // Write planning must process each physical target just once, including
        // targets outside HOME; all four aliases remain launchable.
        assert_eq!(plan(provider, home.path()).unwrap().accounts.len(), 2);
        let rows = discover(provider, home.path()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row.path == linked));
        for alias in ["inside", "inside-copy", "linked", "linked-copy"] {
            assert!(crate::account_registry::resolve_existing(provider, home.path(), alias).is_ok());
        }
        assert!(discover(provider, &home.path().join("missing")).unwrap().is_empty());
    }
    let not_directory = home.path().join("file");
    write(&not_directory, "file");
    assert!(discover(Provider::Claude, &not_directory).unwrap().is_empty());
}

/// A home with a default account, an alt, two named accounts and a typo.
fn claude_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    // ~/.claude: the default login (read from ~/.claude.json) duplicates a@x.com.
    let main = h.join(".claude");
    write(&main.join(".credentials.json"), "{}");
    write(
        &h.join(".claude.json"),
        "{\"oauthAccount\":{\"emailAddress\":\"a@x.com\"}}",
    );
    write(&main.join("commands/a.md"), "A");
    write(&main.join("skills/s/SKILL.md"), "S");
    write(&main.join("settings.json"), "{\"main\":1}");
    // ~/.claude-alt: a unique login b@x.com, a conflicting settings.json.
    let alt = h.join(".claude-alt");
    login(&alt, "b@x.com");
    write(&alt.join("settings.json"), "{\"alt\":2}");
    write(&alt.join("agents/x.md"), "X");
    // A named account whose /finish command exists nowhere else; its skills
    // already link to the old main.
    let a = h.join(".claude-a@x.com");
    login(&a, "a@x.com");
    write(&a.join("commands/finish.md"), "F");
    write(&a.join("commands/a.md"), "A");
    std::os::unix::fs::symlink(main.join("skills"), a.join("skills")).unwrap();
    write(&a.join("sessions/s1.json"), "per-account");
    // Project memory indexes differ per account; plugin caches and registries
    // differ too.
    write(&main.join("projects/-repo/memory/MEMORY.md"), "- [one](one.md)\n");
    write(&a.join("projects/-repo/memory/MEMORY.md"), "- [one](one.md)\n- [two](two.md)\n");
    write(&a.join("projects/-repo/memory/two.md"), "two");
    write(&main.join("plugins/marketplaces/m/README.md"), "old cache");
    write(&main.join("plugins/installed_plugins.json"), "{\"plugins\":{\"x@m\":[1]}}");
    write(&a.join("plugins/installed_plugins.json"), "{\"plugins\":{\"y@m\":[2]}}");
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(&a.join("plugins/marketplaces/m/README.md"), "new cache");
    // A second named account with nothing shared yet.
    login(&h.join(".claude-c@x.com"), "c@x.com");
    // A typo directory with no login.
    write(&h.join(".claude-a@x.cmo/projects/p/t.jsonl"), "typo transcript");
    // Not an account.
    write(&h.join(".claude-a@x.com.lock/.claude.json"), "{}");
    // An empty shared folder, as ~/.claude/workflows is on soundwave (QA F01).
    std::fs::create_dir_all(main.join("workflows")).unwrap();
    home
}

#[test]
fn cas_e1f4_dry_run_lists_planned_links_merges_and_conflicts() {
    let home = claude_home();
    let h = home.path();
    let plan = plan(Provider::Claude, h).unwrap();
    assert!(plan.create_base);
    let labels: Vec<_> = plan.accounts.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(
        labels,
        [".claude", ".claude-alt", ".claude-a@x.cmo", ".claude-a@x.com", ".claude-c@x.com"]
    );
    assert!(plan.dir_changes.contains(&DirChange::Retire {
        label: ".claude".into(),
        reason: "login a@x.com duplicates .claude-a@x.com".into(),
    }));
    assert!(plan.dir_changes.contains(&DirChange::Rename {
        label: ".claude-alt".into(),
        to: ".claude-b@x.com".into(),
    }));
    assert!(plan.dir_changes.iter().any(|change| matches!(
        change,
        DirChange::Retire { label, .. } if label == ".claude-a@x.cmo"
    )));
    let merged: Vec<_> = plan
        .merges
        .iter()
        .map(|m| (m.account.as_str(), m.to.as_str(), m.conflict))
        .collect();
    assert!(merged.contains(&(".claude-a@x.com", "commands/finish.md", false)));
    assert!(merged.contains(&(".claude", "settings.json", false)));
    assert!(merged.contains(&(".claude-alt", "settings.json.from-claude-alt", true)));
    assert!(merged.contains(&(".claude-a@x.cmo", "projects/p/t.jsonl", false)), "a stray's files are merged, never discarded");
    assert!(
        !merged.iter().any(|(_, to, _)| to.starts_with("commands/a.md.from")),
        "identical files are not conflicts"
    );
    let kinds: Vec<_> = plan.merges.iter().map(|m| (m.rel.as_str(), m.kind)).collect();
    assert!(kinds.contains(&("projects/-repo/memory/MEMORY.md", MergeKind::UnionLines)));
    assert!(kinds.contains(&("plugins/installed_plugins.json", MergeKind::JsonUnion)));
    assert!(kinds.contains(&("plugins/marketplaces/m/README.md", MergeKind::Newest)));
    assert_eq!(
        plan.conflicts().map(|m| m.rel.as_str()).collect::<Vec<_>>(),
        ["settings.json"],
        "only a file a person must judge is a conflict"
    );
    assert!(
        !plan.merges.iter().any(|m| m.rel.contains("credentials") || m.rel.contains("sessions")),
        "login and runtime state are never merged"
    );
    let links: Vec<_> = plan
        .links
        .iter()
        .map(|l| (l.account.as_str(), l.item.as_str()))
        .collect();
    assert!(links.contains(&(".claude-a@x.com", "commands")));
    assert!(links.contains(&(".claude-a@x.com", "skills")));
    assert!(links.contains(&(".claude-b@x.com", "settings.json")), "links land at the renamed dir");
    assert!(links.contains(&(".claude-c@x.com", "commands")));

    let text = render_plan(&plan, h);
    // Wrapped continuation lines read as one line for these checks.
    let flat = text.replace("\n  ", " ");
    // pin: the dry run's verdict and every kind of change.
    assert!(text.starts_with("[PLAN] claude: "), "{text}");
    assert!(text.contains("from 5 account dirs"), "{text}");
    assert!(flat.contains("Retire ~/.claude: login a@x.com duplicates .claude-a@x.com"), "{text}");
    assert!(flat.contains("Rename ~/.claude-alt to ~/.claude-b@x.com: unique login"), "{text}");
    // cas-e1f4 QA F03: the conflict line says which copy every account now reads.
    assert!(flat.contains("Conflict settings.json: every account now reads ~/.claude's copy; ~/.claude-alt's is kept beside it as settings.json.from-claude-alt"), "{text}");
    // cas-e1f4 QA F01: an empty shared folder is created and linked in the first run.
    assert_eq!(plan.empty_dirs, ["workflows"]);
    assert!(flat.contains("Create empty shared folders in the base: workflows"), "{text}");
    assert!(links.contains(&(".claude-c@x.com", "workflows")));
    assert!(flat.contains("Merge commands from ~/.claude-a@x.com: 1 file"), "{text}");
    assert!(text.contains("Link ~/.claude-a@x.com: "), "{text}");
    assert!(text.lines().all(|line| line.chars().count() <= 80), "{text}");

    // The dry run wrote nothing.
    assert!(!Provider::Claude.base_dir(h).exists());
    assert!(h.join(".claude").is_dir());
}

#[test]
fn cas_e1f4_apply_links_shared_items_keeps_logins_merges_and_backs_up() {
    let home = claude_home();
    let h = home.path();
    let plan = plan(Provider::Claude, h).unwrap();
    let applied = apply(&plan, h, "20261016T150000Z").unwrap();
    let base = Provider::Claude.base_dir(h);

    // The per-account /finish command is now in the base and in every account.
    // pin: operator-facing dry-run and receipt wording.
    assert_eq!(std::fs::read_to_string(base.join("commands/finish.md")).unwrap(), "F");
    for account in [".claude-a@x.com", ".claude-b@x.com", ".claude-c@x.com"] {
        let dir = h.join(account);
        assert_eq!(std::fs::read_link(dir.join("commands")).unwrap(), base.join("commands"));
        assert_eq!(
            std::fs::read_to_string(dir.join("commands/finish.md")).unwrap(),
            "F",
            "{account}"
        );
        assert!(dir.join(".credentials.json").is_file(), "{account} keeps its login");
        assert!(
            !std::fs::symlink_metadata(dir.join(".credentials.json")).unwrap().file_type().is_symlink()
        );
    }
    // pin: operator-facing dry-run and receipt wording.
    assert_eq!(std::fs::read_to_string(base.join("settings.json")).unwrap(), "{\"main\":1}");
    assert_eq!(
        std::fs::read_to_string(base.join("settings.json.from-claude-alt")).unwrap(),
        "{\"alt\":2}"
    );
    // pin: operator-facing dry-run and receipt wording.
    assert_eq!(std::fs::read_to_string(base.join("skills/s/SKILL.md")).unwrap(), "S");
    assert_eq!(
        std::fs::read_to_string(base.join("projects/-repo/memory/MEMORY.md")).unwrap(),
        "- [one](one.md)\n- [two](two.md)\n",
        "both accounts' memories stay indexed"
    );
    // pin: merged file contents the operator relies on.
    assert_eq!(std::fs::read_to_string(base.join("projects/-repo/memory/two.md")).unwrap(), "two");
    assert_eq!(
        std::fs::read_to_string(base.join("plugins/marketplaces/m/README.md")).unwrap(),
        "new cache",
        "the newest regenerable cache wins"
    );
    let registry: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(base.join("plugins/installed_plugins.json")).unwrap(),
    )
    .unwrap();
    assert!(registry["plugins"]["x@m"].is_array() && registry["plugins"]["y@m"].is_array());
    assert!(!base.join("plugins/marketplaces/m/README.md.from-claude-a@x.com").exists());
    // pin: operator-facing dry-run and receipt wording.
    assert_eq!(std::fs::read_to_string(base.join("projects/p/t.jsonl")).unwrap(), "typo transcript");
    assert_eq!(
        std::fs::read_to_string(h.join(".claude-a@x.com/sessions/s1.json")).unwrap(),
        "per-account"
    );
    // Retired, renamed and stray directories.
    assert!(!h.join(".claude").exists());
    assert!(!h.join(".claude-alt").exists());
    assert!(!h.join(".claude-a@x.cmo").exists());
    assert!(h.join(".claude-b@x.com/.credentials.json").is_file());
    // Everything replaced is in the backup, with a restore script.
    assert!(applied.backup.join("dirs/.claude/.credentials.json").is_file());
    assert!(applied.backup.join("dirs/.claude-a@x.cmo/projects/p/t.jsonl").is_file());
    assert_eq!(
        std::fs::read_to_string(applied.backup.join("items/.claude-a@x.com/commands/finish.md"))
            .unwrap(),
        "F"
    );
    assert!(applied.restore.is_file());
    assert_eq!(applied.conflicts, 1);
}

#[test]
fn cas_e1f4_new_account_comes_out_linked_and_doctor_flags_drift() {
    let home = claude_home();
    let h = home.path();
    apply(&plan(Provider::Claude, h).unwrap(), h, "20261016T150002Z").unwrap();
    let fresh = h.join(".claude-d@x.com");
    std::fs::create_dir_all(&fresh).unwrap();
    let linked = link_new_account(Provider::Claude, h, &fresh).unwrap();
    assert!(linked.contains(&"commands".to_string()), "{linked:?}");
    assert_eq!(
        std::fs::read_to_string(fresh.join("commands/finish.md")).unwrap(),
        "F"
    );
    assert!(!fresh.join(".credentials.json").exists(), "no login is shared");

    // An account whose commands became a real directory again drifts.
    let c = h.join(".claude-c@x.com/commands");
    std::fs::remove_file(&c).unwrap();
    write(&c.join("local.md"), "L");
    let drifted = drift(Provider::Claude, h).unwrap();
    assert_eq!(
        drifted,
        [Drift {
            provider: Provider::Claude,
            account: ".claude-c@x.com".into(),
            item: "commands".into(),
        }]
    );
    // The next plan merges it and relinks.
    let plan = plan(Provider::Claude, h).unwrap();
    assert!(plan.merges.iter().any(|m| m.to == "commands/local.md"));
    assert!(plan.links.iter().any(|l| l.account == ".claude-c@x.com" && l.item == "commands"));
}

#[test]
fn cas_e1f4_codex_retires_the_default_home_by_its_token_email() {
    use base64::Engine as _;
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let token = |email: &str| {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!("{{\"email\":\"{email}\"}}"));
        format!("{{\"tokens\":{{\"id_token\":\"h.{payload}.s\"}}}}")
    };
    write(&h.join(".codex/auth.json"), &token("P@x.com"));
    write(&h.join(".codex/config.toml"), "model = \"x\"\n");
    write(&h.join(".codex/AGENTS.md"), "shared");
    write(&h.join(".codex/installation_id"), "id-main");
    write(&h.join(".codex-p@x.com/auth.json"), &token("p@x.com"));
    write(&h.join(".codex-p@x.com/installation_id"), "id-p");
    std::os::unix::fs::symlink(h.join(".codex/config.toml"), h.join(".codex-p@x.com/config.toml"))
        .unwrap();
    assert_eq!(
        Provider::Codex.login_email(h, &h.join(".codex")).as_deref(),
        Some("p@x.com")
    );
    let plan = plan(Provider::Codex, h).unwrap();
    assert_eq!(
        plan.dir_changes,
        [DirChange::Retire {
            label: ".codex".into(),
            reason: "login p@x.com duplicates .codex-p@x.com".into(),
        }]
    );
    apply(&plan, h, "20261016T150003Z").unwrap();
    let base = Provider::Codex.base_dir(h);
    let p = h.join(".codex-p@x.com");
    assert_eq!(std::fs::read_link(p.join("config.toml")).unwrap(), base.join("config.toml"));
    assert_eq!(std::fs::read_to_string(p.join("AGENTS.md")).unwrap(), "shared");
    assert_eq!(std::fs::read_to_string(p.join("installation_id")).unwrap(), "id-p");
    assert!(super::plan(Provider::Codex, h).unwrap().is_empty());
}

/// Every path under `home` except Cassy's own state, with its bytes or link
/// target, for byte-for-byte restore checks.
fn snapshot(home: &Path) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for entry in walkdir::WalkDir::new(home).follow_links(false).sort_by_file_name() {
        let entry = entry.unwrap();
        let rel = entry.path().strip_prefix(home).unwrap().to_string_lossy().into_owned();
        if rel.is_empty() || rel == ".cas" || rel.starts_with(".cas/") {
            continue;
        }
        let kind = entry.file_type();
        let value = if kind.is_symlink() {
            format!("link {}", std::fs::read_link(entry.path()).unwrap().display())
        } else if kind.is_dir() {
            "dir".to_string()
        } else {
            format!("file {}", String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap()))
        };
        found.insert(rel, value);
    }
    found
}

#[test]
fn cas_e1f4_qa_f01_second_run_is_a_no_op_with_empty_shared_folders() {
    let home = claude_home();
    let h = home.path();
    // Codex homes with empty agents/ and memories/, as on soundwave.
    write(&h.join(".codex/config.toml"), "model = \"x\"\n");
    std::fs::create_dir_all(h.join(".codex/agents")).unwrap();
    std::fs::create_dir_all(h.join(".codex/memories")).unwrap();
    write(&h.join(".codex-p@x.com/auth.json"), "{}");
    for provider in Provider::ALL {
        apply(&plan(provider, h).unwrap(), h, "20261016T170000Z").unwrap();
        let again = plan(provider, h).unwrap();
        assert!(again.is_empty(), "{}", render_plan(&again, h));
        // pin: an idempotent rerun says so.
        assert!(render_plan(&again, h).starts_with("[OK] "), "{}", render_plan(&again, h));
        assert!(drift(provider, h).unwrap().is_empty(), "doctor is quiet right after one apply");
    }
    assert!(std::fs::read_link(h.join(".claude-a@x.com/workflows")).is_ok());
    assert!(std::fs::read_link(h.join(".codex-p@x.com/memories")).is_ok());
}

#[test]
fn cas_e1f4_qa_f02_restore_is_byte_for_byte_on_every_path() {
    // The soundwave layout (retire path) ...
    let home = claude_home();
    let h = home.path();
    let before = snapshot(h);
    let applied = apply(&plan(Provider::Claude, h).unwrap(), h, "20261016T170001Z").unwrap();
    assert!(std::process::Command::new("bash").arg(&applied.restore).status().unwrap().success());
    assert_eq!(snapshot(h), before);

    // ... and the rename path: ~/.claude holds a unique login, and its own
    // .claude.json differs from ~/.claude.json.
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    write(&h.join(".claude.json"), "{\"oauthAccount\":{\"emailAddress\":\"unique@x.com\"},\"homeLevel\":true}");
    write(&h.join(".claude/.claude.json"), "{\"oauthAccount\":{\"emailAddress\":\"unique@x.com\"},\"insideDir\":true}");
    write(&h.join(".claude/.credentials.json"), "CRED-default");
    write(&h.join(".claude/commands/a.md"), "default-a");
    login(&h.join(".claude-other@x.com"), "other@x.com");
    write(&h.join(".claude-other@x.com/commands/a.md"), "other-a");
    let before = snapshot(h);
    let plan = plan(Provider::Claude, h).unwrap();
    assert!(plan.dir_changes.contains(&DirChange::Rename {
        label: ".claude".into(),
        to: ".claude-unique@x.com".into(),
    }));
    // pin: the dry run says the login file is copied in and how undo restores it.
    assert!(render_plan(&plan, h).replace("\n  ", " ").contains(
        "~/.claude.json is copied in as its login file (its own .claude.json kept as .claude.json.pre-link; undo restores it)"
    ));
    let applied = apply(&plan, h, "20261016T170002Z").unwrap();
    let renamed = h.join(".claude-unique@x.com");
    assert_eq!(
        std::fs::read(renamed.join(".claude.json")).unwrap(),
        std::fs::read(h.join(".claude.json")).unwrap(),
        "the renamed account reads the default login"
    );
    // pin: the renamed account keeps its own credentials.
    assert_eq!(std::fs::read_to_string(renamed.join(".credentials.json")).unwrap(), "CRED-default");
    assert!(std::process::Command::new("bash").arg(&applied.restore).status().unwrap().success());
    assert_eq!(snapshot(h), before, "every login file comes back byte for byte");
}
