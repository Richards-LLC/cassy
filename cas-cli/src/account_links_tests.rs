//! cas-05cee: launch and doctor link checks on fixture homes, both providers.
use super::*;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A login for `email` in `dir`, as each provider stores it.
fn login(provider: Provider, dir: &Path, email: &str) {
    use base64::Engine as _;
    match provider {
        Provider::Claude => {
            write(&dir.join(".credentials.json"), "{\"secret\":1}");
            write(
                &dir.join(".claude.json"),
                &format!("{{\"oauthAccount\":{{\"emailAddress\":\"{email}\"}}}}"),
            );
        }
        Provider::Codex => {
            let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(format!("{{\"email\":\"{email}\"}}"));
            write(&dir.join("auth.json"), &format!("{{\"tokens\":{{\"id_token\":\"h.{payload}.s\"}}}}"));
        }
    }
}

/// A linked home: the base holds every shared item, `a@x.com` is linked.
fn linked_home(provider: Provider) -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let base = provider.base_dir(home.path());
    for item in provider.shared() {
        if item.contains('.') {
            write(&base.join(item), "shared");
        } else {
            write(&base.join(item).join("kept.md"), "shared");
        }
    }
    let account = home.path().join(format!("{}a@x.com", provider.prefix()));
    login(provider, &account, "a@x.com");
    accounts::link_new_account(provider, home.path(), &account).unwrap();
    (home, account)
}

#[test]
fn ensure_links_empty_slots_keeps_real_items_and_credentials() {
    for provider in Provider::ALL {
        let (home, account) = linked_home(provider);
        let h = home.path();
        // A session recreated the folder: every slot is empty except one real file.
        let credentials: Vec<_> = std::fs::read_dir(&account)
            .unwrap()
            .flatten()
            .filter(|e| !e.file_type().unwrap().is_symlink())
            .map(|e| (e.path(), std::fs::read(e.path()).unwrap()))
            .collect();
        for item in provider.shared() {
            std::fs::remove_file(account.join(item)).unwrap();
        }
        let real = provider.shared()[0];
        write(&account.join(real).join("local.md"), "mine");

        let ensured = ensure(provider, h, &account).unwrap();
        assert_eq!(ensured.linked.len(), provider.shared().len() - 1, "{provider:?}");
        assert_eq!(ensured.status.conflicts, [real]);
        assert!(ensured.status.missing.is_empty());
        for item in &ensured.linked {
            assert_eq!(std::fs::read_link(account.join(item)).unwrap(), provider.base_dir(h).join(item));
        }
        assert_eq!(std::fs::read_to_string(account.join(real).join("local.md")).unwrap(), "mine");
        for (path, bytes) in &credentials {
            assert_eq!(&std::fs::read(path).unwrap(), bytes, "{} touched", path.display());
        }
        let lines = notices(provider, h, &account, &ensured);
        let base = format!("~/.cas/{}-base", provider.name());
        assert_eq!(
            lines,
            [
                format!(
                    "Linked {} shared items in ~/{}a@x.com from {base}",
                    provider.shared().len() - 1,
                    provider.prefix()
                ),
                format!(
                    "Warning: 1 shared item in ~/{}a@x.com not linked to {base}: {real}. Fix: cas doctor --fix --yes",
                    provider.prefix()
                ),
            ]
        );

        // Nothing missing: the launch check is a few stat calls.
        std::fs::remove_dir_all(account.join(real)).unwrap();
        ensure(provider, h, &account).unwrap();
        let started = std::time::Instant::now();
        let clean = ensure(provider, h, &account).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert!(clean.linked.is_empty() && clean.status.is_clean(), "{clean:?}");
    }
}

#[test]
fn recreated_retired_folders_are_named_and_never_linked() {
    for provider in Provider::ALL {
        let (home, _) = linked_home(provider);
        let h = home.path();
        let default = h.join(format!(".{}", provider.name()));
        std::fs::create_dir_all(&default).unwrap();
        let ensured = ensure(provider, h, &default).unwrap();
        assert!(ensured.linked.is_empty(), "a retired folder stays unlinked");
        assert_eq!(std::fs::read_dir(&default).unwrap().count(), 0);
        assert_eq!(
            notices(provider, h, &default, &ensured),
            [format!(
                "Warning: ~/.{} is a retired account folder that came back (it has no login); pick a named account: cas accounts list",
                provider.name()
            )]
        );

        // The same login as a named account: point at it.
        let typo = h.join(format!("{}a@x.cmo", provider.prefix()));
        login(provider, &typo, "a@x.com");
        let ensured = ensure(provider, h, &typo).unwrap();
        assert_eq!(
            notices(provider, h, &typo, &ensured),
            [format!(
                "Warning: ~/{p}a@x.cmo is a retired account folder that came back (its login a@x.com duplicates ~/{p}a@x.com); use ~/{p}a@x.com: cas {} a@x.com",
                provider.name(),
                p = provider.prefix()
            )]
        );
        let survey = survey(h).unwrap();
        assert_eq!(survey.iter().filter(|(_, _, s)| s.retired.is_some()).count(), 2);
    }
}

#[test]
fn a_repair_in_the_same_second_as_a_link_gets_its_own_backup() {
    let (home, account) = linked_home(Provider::Claude);
    let h = home.path();
    std::fs::remove_file(account.join("agents")).unwrap();
    write(&account.join("agents/mine.md"), "mine");
    // The backup the link just wrote, for this second and the next two.
    let now = chrono::Utc::now();
    for offset in 0..3 {
        let stamp = (now + chrono::Duration::seconds(offset)).format("%Y%m%dT%H%M%SZ");
        write(&h.join(format!(".cas/accounts-backup/{stamp}-claude/items/.claude-a@x.com/agents/old.md")), "old");
    }
    let applied = repair(Provider::Claude, h).unwrap().unwrap();
    assert!(applied.backup.to_string_lossy().ends_with("-2-claude"), "{}", applied.backup.display());
    let backed_up = applied.backup.join("items").join(".claude-a@x.com").join("agents");
    assert!(backed_up.join("mine.md").is_file(), "the real folder is in this run's backup");
    assert!(std::fs::read_link(account.join("agents")).is_ok());
}
