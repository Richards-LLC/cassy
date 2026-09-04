//! Refuse to mint a cloud identity for a throwaway checkout (GH #701).
//!
//! # What went wrong
//!
//! Every Cassy root gets a canonical id, and any root that syncs pushes rows
//! into the account's shared buckets. Test harnesses, proxy probes and
//! `mktemp`-style scratch directories are Cassy roots too, so they minted
//! identities and pushed. The foreign-row census in GH #701 caught three of
//! them by name — `mecha-proxy-probe.Tz6bxx` (a probe with a random suffix),
//! `fresh-proxy` (a root whose database has no `tasks` table at all) — sitting
//! alongside real projects as sources of contamination, and their rows come
//! back down on every pull.
//!
//! # The rule, and why it is shaped to under-fire
//!
//! Blocking a real project's sync is far worse than letting one probe through,
//! so this classifier is deliberately conservative:
//!
//!  * **An explicit `[project] canonical_id` pin always wins.** Someone stated
//!    this project's identity on purpose; that is the escape hatch, and it is
//!    the reason the guard can be strict about everything else.
//!  * Otherwise a root is ephemeral when it lives under a temp directory, or
//!    its folder name carries a `mktemp` random suffix, or its database has no
//!    `tasks` table.
//!
//! Nothing here deletes or rewrites anything. It only declines to push.

use std::path::Path;

/// Whether a project root is durable enough to own a cloud identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDurability {
    /// Sync normally.
    Durable,
    /// Do not push. `reason` is shown to the operator with the pin escape hatch.
    Ephemeral { reason: String },
}

impl ProjectDurability {
    pub fn is_ephemeral(&self) -> bool {
        matches!(self, ProjectDurability::Ephemeral { .. })
    }

    /// Operator-facing explanation, including the one command that overrides it.
    pub fn explain(&self, project_id: &str) -> Option<String> {
        match self {
            ProjectDurability::Durable => None,
            ProjectDurability::Ephemeral { reason } => Some(format!(
                "Refusing to sync `{project_id}`: {reason}. Throwaway checkouts that push \
                 into the account's shared buckets are the source of the cross-project rows \
                 in GH #701. If this really is a project you want synced, state so with \
                 `cas cloud project set <id>` and the sync will proceed."
            )),
        }
    }
}

/// Directories whose descendants are scratch space by construction.
const TEMP_ROOTS: &[&str] = &["/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp"];

/// Pure classifier — no IO, so every branch is testable.
///
/// `project_dir` is the project directory (the parent of `.cas`), `has_pin` is
/// whether `[project] canonical_id` is set, and `has_tasks_table` is whether
/// the local database carries a `tasks` table (`None` when the database is
/// absent or unreadable, which is *not* treated as evidence either way).
pub fn classify(
    project_dir: &Path,
    has_pin: bool,
    has_tasks_table: Option<bool>,
) -> ProjectDurability {
    if has_pin {
        return ProjectDurability::Durable;
    }

    if let Some(root) = temp_root_of(project_dir) {
        return ProjectDurability::Ephemeral {
            reason: format!("it lives under the scratch directory {root}"),
        };
    }

    let name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if let Some(suffix) = mktemp_suffix(name) {
        return ProjectDurability::Ephemeral {
            reason: format!(
                "its directory name `{name}` ends in the random suffix `{suffix}`, the shape \
                 `mktemp` produces"
            ),
        };
    }

    if has_tasks_table == Some(false) {
        return ProjectDurability::Ephemeral {
            reason: "its database has no `tasks` table, so it has never held real work".to_string(),
        };
    }

    ProjectDurability::Durable
}

/// IO wrapper around [`classify`]. `cas_root` is the `.cas` directory.
pub fn classify_project_root(cas_root: &Path) -> ProjectDurability {
    let project_dir = cas_root.parent().unwrap_or(cas_root);
    let has_pin = super::config::canonical_id_from_config_toml(cas_root).is_some();
    classify(project_dir, has_pin, has_tasks_table(cas_root))
}

fn has_tasks_table(cas_root: &Path) -> Option<bool> {
    let db_path = cas_root.join("cas.db");
    if !db_path.is_file() {
        return None;
    }
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .ok()
    .map(|count| count > 0)
}

fn temp_root_of(project_dir: &Path) -> Option<&'static str> {
    let text = project_dir.to_string_lossy().into_owned();
    TEMP_ROOTS.iter().copied().find(|root| {
        // Prefix match on a path *component* boundary, so `/tmpfoo` is not
        // mistaken for a child of `/tmp`.
        text == *root || text.starts_with(&format!("{root}/"))
    })
}

/// The trailing random token `mktemp`-style helpers append: `name.Tz6bxx`,
/// `.tmpXXXXXX`.
///
/// Required to mix upper and lower case, which is what makes it random rather
/// than a version or a file extension — `next.js`, `app.v2` and `foo.bar` all
/// stay durable.
fn mktemp_suffix(name: &str) -> Option<&str> {
    if let Some(rest) = name.strip_prefix(".tmp")
        && rest.len() >= 6
        && rest.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Some(rest);
    }
    let (stem, suffix) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let random_shaped = (6..=12).contains(&suffix.len())
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
        && suffix.chars().any(|c| c.is_ascii_uppercase())
        && suffix.chars().any(|c| c.is_ascii_lowercase());
    random_shaped.then_some(suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    /// The three roots GH #701 names by measurement.
    #[test]
    fn the_probe_roots_named_in_the_issue_are_refused() {
        let probe = classify(&dir("/var/tmp/mecha-proxy-probe.Tz6bxx"), false, Some(true));
        assert!(probe.is_ephemeral(), "{probe:?}");

        // Same shape outside a temp directory: the name alone is enough.
        let named = classify(
            &dir("/home/dev/work/mecha-proxy-probe.Tz6bxx"),
            false,
            Some(true),
        );
        assert!(named.is_ephemeral(), "{named:?}");

        let fresh_proxy = classify(&dir("/home/dev/fresh-proxy"), false, Some(false));
        assert!(fresh_proxy.is_ephemeral(), "{fresh_proxy:?}");
    }

    #[test]
    fn mktemp_scratch_directories_are_refused() {
        assert!(classify(&dir("/tmp/.tmplhQJF8"), false, Some(true)).is_ephemeral());
        assert!(classify(&dir("/tmp/anything"), false, Some(true)).is_ephemeral());
    }

    /// The escape hatch. A stated identity is a human decision and outranks
    /// every heuristic here — including the temp-directory one, because
    /// integration suites legitimately pin a scratch root.
    #[test]
    fn an_explicit_pin_always_wins() {
        assert_eq!(
            classify(&dir("/tmp/.tmplhQJF8"), true, Some(false)),
            ProjectDurability::Durable
        );
    }

    /// Blocking a real project is the expensive failure, so the false-positive
    /// surface is what this test guards.
    #[test]
    fn real_projects_are_never_refused() {
        for path in [
            "/home/dev/Petrastella/cas-src",
            "/home/dev/Petrastella/gabber-studio",
            "/home/dev/soundwave-config",
            "/home/dev/Richards LLC/Accounting",
            // Names with dots that are versions or extensions, not randomness.
            "/home/dev/next.js",
            "/home/dev/app.v2",
            "/home/dev/site.github.io",
            // A directory that merely starts with the letters "tmp".
            "/tmpfoo/real-project",
        ] {
            assert_eq!(
                classify(&dir(path), false, Some(true)),
                ProjectDurability::Durable,
                "{path} must stay syncable"
            );
        }
    }

    /// An unreadable or absent database is not evidence of a probe.
    #[test]
    fn an_unknown_database_is_not_treated_as_ephemeral() {
        assert_eq!(
            classify(&dir("/home/dev/real-project"), false, None),
            ProjectDurability::Durable
        );
    }

    #[test]
    fn an_unpinned_store_without_a_git_remote_is_ephemeral() {
        let verdict = classify(&dir("/home/dev/container"), false, Some(true));
        assert!(
            verdict.is_ephemeral(),
            "a bare folder identity must never mint a cloud bucket: {verdict:?}"
        );
    }

    #[test]
    fn the_refusal_names_the_override() {
        let verdict = classify(&dir("/home/dev/fresh-proxy"), false, Some(false));
        let message = verdict.explain("fresh-proxy").expect("ephemeral explains");
        assert!(message.contains("cas cloud project set"), "{message}");
        assert!(message.contains("no `tasks` table"), "{message}");
        assert_eq!(ProjectDurability::Durable.explain("cas-src"), None);
    }
}
