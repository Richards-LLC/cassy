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

        // Claude's non-secret email can identify a duplicate. Codex keeps
        // explicit aliases distinct rather than opening auth.json to infer it.
        let typo = h.join(format!("{}a@x.cmo", provider.prefix()));
        login(provider, &typo, "a@x.com");
        let ensured = ensure(provider, h, &typo).unwrap();
        if provider == Provider::Codex {
            assert!(ensured.status.retired.is_none());
            assert_eq!(ensured.linked.len(), provider.shared().len());
            assert_eq!(survey(h).unwrap().iter().filter(|(_, _, s)| s.retired.is_some()).count(), 1);
            continue;
        }
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
fn a_launch_repair_touches_only_the_launched_folder_and_keeps_it_logged_in() {
    for provider in Provider::ALL {
        let (home, account) = linked_home(provider);
        let h = home.path();
        let p = provider.prefix();
        // The full `cas accounts link` plan would rename this default folder
        // (a unique login) and merge another account's real item.
        let default = h.join(format!(".{}", provider.name()));
        login(provider, &default, "new@y.com");
        write(&default.join("agents/theirs.md"), "default");
        let other = h.join(format!("{p}c@x.com"));
        login(provider, &other, "c@x.com");
        write(&other.join("agents/theirs.md"), "other");
        // The launched folder: one real item with a file the base lacks.
        std::fs::remove_file(account.join("agents")).unwrap();
        write(&account.join("agents/mine.md"), "mine");
        let credential = std::fs::read(account.join(provider.credentials())).unwrap();
        // An earlier run's backup holds this second and the next two.
        let now = chrono::Utc::now();
        for offset in 0..3 {
            let stamp = (now + chrono::Duration::seconds(offset)).format("%Y%m%dT%H%M%SZ");
            std::fs::create_dir_all(h.join(format!(".cas/accounts-backup/{stamp}-{}", provider.name()))).unwrap();
        }

        let mut asked = Vec::new();
        let mut record = |question: &str| {
            asked.push(question.to_string());
            true
        };
        let launched = prelaunch_with(provider, h, &account, Some(&mut record as &mut dyn FnMut(&str) -> bool));
        assert_eq!(launched, account);
        assert_eq!(asked.len(), 1);
        assert!(asked[0].contains(&format!("agents in ~/{p}a@x.com only")), "{asked:?}");
        assert!(account.is_dir(), "the launched folder stays in place");
        assert_eq!(std::fs::read(account.join(provider.credentials())).unwrap(), credential);
        assert_eq!(std::fs::read_link(account.join("agents")).unwrap(), provider.base_dir(h).join("agents"));
        assert!(provider.base_dir(h).join("agents/mine.md").is_file());
        assert!(default.join("agents/theirs.md").is_file() && !default.join("agents").is_symlink(), "default untouched");
        assert!(other.join("agents/theirs.md").is_file() && !other.join("agents").is_symlink(), "other account untouched");
        let backup = std::fs::read_dir(h.join(".cas/accounts-backup"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.join("restore.sh").is_file())
            .expect("a backup with a restore script");
        assert!(backup.to_string_lossy().ends_with(&format!("-2-{}", provider.name())), "{}", backup.display());

        // Folder changes or a merge across accounts: no offer, nothing moved.
        for folder in [&default, &other] {
            if folder == &other {
                write(&provider.base_dir(h).join("agents/theirs.md"), "base copy differs");
            }
            let mut asked = false;
            let mut record = |_: &str| {
                asked = true;
                true
            };
            prelaunch_with(provider, h, folder, Some(&mut record as &mut dyn FnMut(&str) -> bool));
            assert!(!asked, "{} needs doctor, not a launch repair", folder.display());
            assert!(folder.join("agents/theirs.md").is_file());
        }
    }
}
