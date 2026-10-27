//! Doctor view and repair of account shared links (cas-e1f4, cas-05cee).
use std::path::Path;

use super::{Check, CheckStatus};
use crate::account_links::{self, short};
use crate::accounts::{self, Provider};
use crate::cli::Cli;

const NAME: &str = "account links";

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// One row per account whose shared links drifted, or one OK row.
pub(super) fn checks() -> Vec<Check> {
    let Some(home) = dirs::home_dir() else {
        return vec![Check::new(NAME, CheckStatus::Ok, "no home directory")];
    };
    checks_in(&home)
}

fn checks_in(home: &Path) -> Vec<Check> {
    if !Provider::ALL.iter().any(|provider| provider.base_dir(home).is_dir()) {
        return vec![Check::new(
            NAME,
            CheckStatus::Ok,
            "accounts do not share a base folder yet (`cas accounts link --dry-run` previews it)",
        )];
    }
    let survey = match account_links::survey(home) {
        Ok(survey) => survey,
        Err(error) => {
            return vec![Check::new(NAME, CheckStatus::Warning, format!("could not read accounts: {error:#}"))];
        }
    };
    let rows: Vec<Check> = survey
        .iter()
        .filter(|(_, _, status)| !status.is_clean())
        .map(|(provider, folder, status)| {
            let here = short(home, folder);
            let message = if let Some(retired) = &status.retired {
                let next = match &retired.canonical {
                    Some(canonical) => format!("use {}", short(home, canonical)),
                    None => "pick a named account (`cas accounts list`)".into(),
                };
                format!(
                    "{here}: retired account folder recreated ({}); {next}; `cas doctor --fix --yes` retires it again with a backup",
                    retired.reason
                )
            } else {
                let mut parts = Vec::new();
                if !status.missing.is_empty() {
                    parts.push(format!(
                        "{} ({})",
                        plural(status.missing.len(), "missing link"),
                        status.missing.join(", ")
                    ));
                }
                if !status.conflicts.is_empty() {
                    parts.push(format!(
                        "{} not linked ({})",
                        plural(status.conflicts.len(), "real item"),
                        status.conflicts.join(", ")
                    ));
                }
                let fix = if status.conflicts.is_empty() {
                    "run `cas doctor --fix`"
                } else {
                    "run `cas doctor --fix` to preview, `--fix --yes` to back up and link"
                };
                format!(
                    "{here}: {} to ~/.cas/{}-base; {fix}",
                    parts.join(", "),
                    provider.name()
                )
            };
            Check::new(NAME, CheckStatus::Warning, message)
        })
        .collect();
    if rows.is_empty() {
        return vec![Check::new(NAME, CheckStatus::Ok, "every account links its shared items to the base")];
    }
    rows
}

/// `--fix`: link empty slots, sweep abandoned logins, and preview (or with
/// consent apply) `cas accounts link` for real items and recreated folders.
pub(super) fn autofix(yes: bool, cli: &Cli) -> Vec<Check> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    autofix_in(&home, |summary| super::confirm_doctor_consent(cli, yes, summary))
}

fn autofix_in(home: &Path, mut confirm: impl FnMut(&str) -> bool) -> Vec<Check> {
    // The existing safe account repair: abandoned login staging folders.
    crate::account_lifecycle::sweep_stale_logins(home);
    let survey = match account_links::survey(home) {
        Ok(survey) => survey,
        Err(error) => {
            return vec![Check::new("auto-fix", CheckStatus::Warning, format!("account links: {error:#}"))];
        }
    };
    let mut rows = Vec::new();
    let mut needs_link: Vec<Provider> = Vec::new();
    for (provider, folder, status) in &survey {
        if (status.retired.is_some() || !status.conflicts.is_empty()) && !needs_link.contains(provider) {
            needs_link.push(*provider);
        }
        if status.retired.is_some() || status.missing.is_empty() {
            continue;
        }
        match account_links::ensure(*provider, home, folder) {
            Ok(ensured) if !ensured.linked.is_empty() => rows.push(Check::new(
                "auto-fix",
                CheckStatus::Ok,
                format!(
                    "fixed: account links — linked {} in {} from ~/.cas/{}-base ({})",
                    plural(ensured.linked.len(), "shared item"),
                    short(home, folder),
                    provider.name(),
                    ensured.linked.join(", ")
                ),
            )),
            Ok(_) => {}
            Err(error) => rows.push(Check::new(
                "auto-fix",
                CheckStatus::Warning,
                format!("account links: could not link {}: {error:#}", short(home, folder)),
            )),
        }
    }
    for provider in needs_link {
        let plan = match accounts::plan(provider, home) {
            Ok(plan) if !plan.is_empty() => plan,
            Ok(_) => continue,
            Err(error) => {
                rows.push(Check::new("auto-fix", CheckStatus::Warning, format!("account links: {error:#}")));
                continue;
            }
        };
        let mut preview: Vec<String> = plan
            .dir_changes
            .iter()
            .map(|change| match change {
                accounts::DirChange::Retire { label, reason } => format!("retire ~/{label} ({reason})"),
                accounts::DirChange::Rename { label, to } => format!("rename ~/{label} to ~/{to}"),
            })
            .collect();
        let mut per_account: Vec<(String, Vec<String>)> = Vec::new();
        for link in plan.links.iter().filter(|link| link.was != "missing") {
            match per_account.iter_mut().find(|(account, _)| account == &link.account) {
                Some((_, items)) => items.push(link.item.clone()),
                None => per_account.push((link.account.clone(), vec![link.item.clone()])),
            }
        }
        preview.extend(
            per_account
                .into_iter()
                .map(|(account, items)| format!("replace {} in ~/{account} with links", items.join(", "))),
        );
        let summary = format!(
            "account links ({}): {}; the replaced items are merged into the base and backed up with a restore script.",
            provider.name(),
            preview.join("; ")
        );
        if !confirm(&summary) {
            rows.push(Check::new(
                "auto-fix",
                CheckStatus::Info,
                format!("{summary} Dry run only; re-run `cas doctor --fix --yes` to apply."),
            ));
            continue;
        }
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        rows.push(match accounts::apply(&plan, home, &stamp) {
            Ok(applied) => Check::new(
                "auto-fix",
                CheckStatus::Ok,
                format!(
                    "fixed: account links ({}) — {}, {}, {}; backup {}; undo: bash {}",
                    provider.name(),
                    plural(applied.linked, "link"),
                    plural(applied.dir_changes, "folder change"),
                    plural(applied.merged, "file merged"),
                    short(home, &applied.backup),
                    applied.restore.display()
                ),
            ),
            Err(error) => Check::new("auto-fix", CheckStatus::Warning, format!("account links ({}): {error:#}", provider.name())),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Snapshot of every regular file under `dir` (credentials included).
    fn files(dir: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let kind = entry.file_type().unwrap();
            if kind.is_file() {
                found.push((entry.path(), std::fs::read(entry.path()).unwrap()));
            }
        }
        found.sort();
        found
    }

    #[test]
    fn doctor_reports_drift_links_missing_and_backs_up_real_items_only_with_consent() {
        for provider in Provider::ALL {
            let home = tempfile::tempdir().unwrap();
            let h = home.path();
            let base = provider.base_dir(h);
            let shared = provider.shared();
            for item in &shared[..2] {
                if item.contains('.') {
                    write(&base.join(item), "shared");
                } else {
                    write(&base.join(item).join("kept.md"), "shared");
                }
            }
            let account = h.join(format!("{}a@x.com", provider.prefix()));
            // Credentials and private entries with the slots empty, plus one
            // real folder in a shared slot.
            let (private, login) = match provider {
                Provider::Claude => (
                    vec![".credentials.json", ".claude.json"],
                    "{\"oauthAccount\":{\"emailAddress\":\"a@x.com\"}}",
                ),
                Provider::Codex => (vec!["auth.json"], "{\"tokens\":{}}"),
            };
            for name in &private {
                write(&account.join(name), login);
            }
            write(&account.join(shared[1]).join("mine.md"), "mine");
            // A retired default folder that came back with no login.
            std::fs::create_dir_all(h.join(format!(".{}", provider.name()))).unwrap();
            let credentials = files(&account);

            let rows: Vec<String> = checks_in(h).into_iter().map(|row| row.message).collect();
            let p = provider.prefix();
            assert_eq!(
                rows,
                [
                    format!(
                        "~/.{}: retired account folder recreated (it has no login); pick a named account (`cas accounts list`); `cas doctor --fix --yes` retires it again with a backup",
                        provider.name()
                    ),
                    format!(
                        "~/{p}a@x.com: 1 missing link ({}), 1 real item not linked ({}) to ~/.cas/{}-base; run `cas doctor --fix` to preview, `--fix --yes` to back up and link",
                        shared[0],
                        shared[1],
                        provider.name()
                    ),
                ]
            );

            // Plain --fix: the empty slot is linked, the real item only previewed.
            let rows = autofix_in(h, |_| false);
            assert_eq!(rows.len(), 2, "{:?}", rows.iter().map(|r| &r.message).collect::<Vec<_>>());
            assert!(rows[0].message.starts_with(&format!("fixed: account links — linked 1 shared item in ~/{p}a@x.com")));
            assert!(rows[1].message.contains("Dry run only"), "{}", rows[1].message);
            assert!(rows[1].message.contains(&format!("replace {} in ~/{p}a@x.com with links", shared[1])));
            assert_eq!(std::fs::read_link(account.join(shared[0])).unwrap(), base.join(shared[0]));
            assert!(account.join(shared[1]).join("mine.md").is_file());
            assert!(!h.join(".cas/accounts-backup").exists());

            // --yes: backed up, merged, linked, and the default folder retired.
            let rows = autofix_in(h, |_| true);
            assert_eq!(rows.len(), 1);
            let applied = &rows[0].message;
            assert!(applied.starts_with(&format!("fixed: account links ({})", provider.name())), "{applied}");
            let restore = applied.rsplit("undo: bash ").next().unwrap();
            assert!(Path::new(restore).is_file(), "{applied}");
            assert_eq!(std::fs::read_link(account.join(shared[1])).unwrap(), base.join(shared[1]));
            assert_eq!(std::fs::read_to_string(base.join(shared[1]).join("mine.md")).unwrap(), "mine");
            assert!(!h.join(format!(".{}", provider.name())).exists());
            assert_eq!(files(&account), credentials, "credentials untouched");
            assert_eq!(
                checks_in(h).into_iter().map(|row| row.message).collect::<Vec<_>>(),
                ["every account links its shared items to the base"]
            );
        }
    }
}
