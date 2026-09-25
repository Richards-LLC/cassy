//! Store write guard: rules and knowledge pages that name another registered
//! project do not belong in this project's store (skills audit M27/M83).
//!
//! ROOT CAUSE this exists for (cas-caae): a Gabber Studio branching rule
//! ("ALWAYS cut new branches … from `staging`") became the only proven rule in
//! cas-src's store and was synced unconditionally to
//! `.claude/rules/cas/rule-002.md`. cas-src has no `staging` branch. The rule
//! arrived through a team pull that matched on the per-store `rule-NNN` id,
//! but it is surfaced — and harms — through the local rule-file sync. So this
//! guard sits at the write surfaces an agent reaches (rule create, knowledge
//! write) and at the rule-file sync, where it keeps a foreign rule out of
//! Claude Code whichever path put it in the store.
//!
//! "Registered project" means a root in the host known-repos registry
//! (`~/.cas/cas.db`). Only distinctive names count: a multi-word slug such as
//! `gabber-studio`, matched as the slug itself (`gabber-studio`,
//! `gabber_studio`) or in title case (`Gabber Studio`). Single-word names
//! (`ozer`, `canvas`, `logging`) are ignored because they collide with
//! ordinary prose and paths. A rule or page that deliberately covers another
//! project opts in with a `project:<slug>` marker (a rule tag, or a knowledge
//! source).

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Prefix of the marker that declares an explicit cross-project scope.
pub const PROJECT_MARKER_PREFIX: &str = "project:";

/// Detects mentions of other registered projects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignProjectGuard {
    own: String,
    candidates: Vec<String>,
}

impl ForeignProjectGuard {
    /// Build a guard for `own_project` from a list of registered project names.
    /// Non-distinctive names and names related to the project itself (its own
    /// name, or a `<own>-suffix` copy such as a worktree) are dropped.
    pub fn new(own_project: &str, registered: impl IntoIterator<Item = String>) -> Self {
        let own = own_project.to_ascii_lowercase();
        let mut candidates: Vec<String> = registered
            .into_iter()
            .map(|name| name.to_ascii_lowercase())
            .filter(|name| is_distinctive_slug(name) && !related_to_own(name, &own))
            .collect();
        candidates.sort();
        candidates.dedup();
        Self { own, candidates }
    }

    /// Guard for the project rooted at `project_root`, reading the host
    /// known-repos registry. Any failure to read it yields an inert guard: the
    /// guard must never make a rule or page write fail for its own reasons.
    pub fn for_project_root(project_root: &Path) -> Self {
        let own = project_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        Self::new(own, registered_project_names())
    }

    /// Lower-cased name of the project this guard protects.
    pub fn own_project(&self) -> &str {
        &self.own
    }

    /// Registered project slugs this guard looks for.
    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }

    /// Registered projects named in any of `texts` and not acknowledged by a
    /// `project:<slug>` entry in `markers`.
    pub fn foreign_mentions(&self, texts: &[&str], markers: &[String]) -> Vec<String> {
        let acknowledged = acknowledged_projects(markers);
        self.candidates
            .iter()
            .filter(|slug| !acknowledged.contains(slug))
            .filter(|slug| texts.iter().any(|text| mentions_project(text, slug)))
            .cloned()
            .collect()
    }

    /// Refusal for a rule, or `None` when it may be written or synced here.
    pub fn check_rule(&self, content: &str, tags: &[String]) -> Option<ForeignProjectRefusal> {
        let projects = self.foreign_mentions(&[content], tags);
        (!projects.is_empty()).then(|| ForeignProjectRefusal {
            kind: "rule",
            own: self.own.clone(),
            projects,
            marker_field: "tags",
        })
    }

    /// Drop rules that name another registered project from a rule-file sync
    /// batch. `sync_all` then deletes any such rule's existing file as stale,
    /// so a foreign rule already in the store stops reaching Claude Code.
    pub fn retain_syncable_rules(&self, rules: &mut Vec<crate::types::Rule>) {
        if self.candidates.is_empty() {
            return;
        }
        rules.retain(|rule| self.check_rule(&rule.content, &rule.tags).is_none());
    }

    /// Refusal for a knowledge page, or `None` when it may be written here.
    pub fn check_knowledge_page(
        &self,
        title: &str,
        body: &str,
        sources: &[String],
    ) -> Option<ForeignProjectRefusal> {
        let projects = self.foreign_mentions(&[title, body], sources);
        (!projects.is_empty()).then(|| ForeignProjectRefusal {
            kind: "knowledge page",
            own: self.own.clone(),
            projects,
            marker_field: "sources",
        })
    }
}

/// Why a write was refused; `Display` is the actionable error text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignProjectRefusal {
    kind: &'static str,
    own: String,
    /// The registered projects the content names.
    pub projects: Vec<String>,
    marker_field: &'static str,
}

impl std::fmt::Display for ForeignProjectRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = self.projects.join(", ");
        let markers = self
            .projects
            .iter()
            .map(|project| format!("{PROJECT_MARKER_PREFIX}{project}"))
            .collect::<Vec<_>>()
            .join(",");
        write!(
            f,
            "Refused: this {kind} names another registered project ({names}), but this store belongs to '{own}'. \
             Cassy stores are per project, so a {kind} about {names} surfaces in every '{own}' session as if it applied here. \
             Write it from the {names} checkout instead. If it deliberately covers {names} from '{own}', \
             add `{markers}` to `{field}` and retry.",
            kind = self.kind,
            own = self.own,
            field = self.marker_field,
        )
    }
}

/// Slugs acknowledged by `project:<slug>` markers (case-insensitive).
pub fn acknowledged_projects(markers: &[String]) -> Vec<String> {
    markers
        .iter()
        .filter_map(|marker| {
            let marker = marker.trim();
            let prefix = marker.get(..PROJECT_MARKER_PREFIX.len())?;
            prefix.eq_ignore_ascii_case(PROJECT_MARKER_PREFIX).then(|| {
                marker[PROJECT_MARKER_PREFIX.len()..]
                    .trim()
                    .to_ascii_lowercase()
            })
        })
        .filter(|slug| !slug.is_empty())
        .collect()
}

fn slug_tokens(slug: &str) -> Vec<&str> {
    slug.split(['-', '_']).collect()
}

/// A name distinctive enough to match in free text: at least two
/// non-empty, not-all-numeric tokens, and not a hidden/temp directory.
fn is_distinctive_slug(name: &str) -> bool {
    if name.starts_with('.') || name.len() < 5 {
        return false;
    }
    let tokens = slug_tokens(name);
    tokens.len() >= 2
        && tokens.iter().all(|token| !token.is_empty())
        && !tokens
            .iter()
            .all(|token| token.chars().all(|c| c.is_ascii_digit()))
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The project's own name, or a `<name>-suffix` / `<name>_suffix` relative
/// of it in either direction (worktree copies, `-wt-*` checkouts).
fn related_to_own(name: &str, own: &str) -> bool {
    if own.is_empty() {
        return false;
    }
    let extends = |long: &str, short: &str| {
        long.strip_prefix(short)
            .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('_'))
    };
    name == own || extends(name, own) || extends(own, name)
}

fn is_boundary(c: Option<char>) -> bool {
    c.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
}

fn contains_bounded(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(start, matched)| {
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + matched.len()..].chars().next();
        is_boundary(before) && is_boundary(after)
    })
}

/// Whether `text` names `slug` (already lower-case).
fn mentions_project(text: &str, slug: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let hyphen = slug.replace('_', "-");
    let underscore = slug.replace('-', "_");
    if contains_bounded(&lower, &hyphen) || contains_bounded(&lower, &underscore) {
        return true;
    }
    let title = slug_tokens(slug)
        .iter()
        .map(|token| {
            let mut chars = token.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    contains_bounded(text, &title)
}

/// Whether `db` is listed in `CAS_TEST_PROTECTED_DBS`. The guard is a reader
/// of the host registry, and a test run under
/// `scripts/check-real-store-untouched.sh` must not open a real store at all.
fn is_protected_test_db(db: &Path) -> bool {
    let Some(protected) = std::env::var_os(cas_store::shared_db::PROTECTED_DBS_ENV) else {
        return false;
    };
    let canonical = db.canonicalize().unwrap_or_else(|_| db.to_path_buf());
    std::env::split_paths(&protected)
        .filter(|entry| !entry.as_os_str().is_empty())
        .map(|entry| {
            if entry.is_dir() {
                entry.join("cas.db")
            } else {
                entry
            }
        })
        .any(|entry| entry.canonicalize().unwrap_or(entry) == canonical)
}

/// Project names from the host known-repos registry, read-only. Paths under a
/// `.cas` directory (worktrees, artifacts) or the system temp directory are
/// copies, not projects, and are skipped.
fn registered_project_names() -> Vec<String> {
    let db = crate::store::known_repos::host_cas_dir().join("cas.db");
    if !db.is_file() || is_protected_test_db(&db) {
        return Vec::new();
    }
    let Ok(connection) = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let _ = connection.busy_timeout(Duration::from_millis(50));
    let Ok(mut statement) = connection.prepare("SELECT path FROM known_repos") else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    let temp = std::env::temp_dir();
    let temp = temp.canonicalize().unwrap_or(temp);
    rows.filter_map(Result::ok)
        .map(PathBuf::from)
        .filter(|path| !path.components().any(|c| c.as_os_str() == ".cas"))
        .filter(|path| !path.starts_with(&temp))
        .filter_map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(own: &str) -> ForeignProjectGuard {
        ForeignProjectGuard::new(
            own,
            [
                "gabber-studio",
                "cas-src",
                "petra-stella-cloud",
                "ozer",
                "canvas",
                ".tmpAbC123",
                "2022",
                "gabber-studio-wt-utf8",
            ]
            .map(String::from),
        )
    }

    const GABBER_RULE: &str =
        "Gabber Studio branching: ALWAYS cut new branches from `staging`, never from `develop`.";

    #[test]
    fn only_distinctive_names_other_than_own_are_candidates() {
        let g = guard("cas-src");
        assert_eq!(
            g.candidates(),
            [
                "gabber-studio",
                "gabber-studio-wt-utf8",
                "petra-stella-cloud"
            ]
        );
        // A `<own>-suffix` checkout of the project itself is not foreign.
        let g = guard("gabber-studio");
        assert_eq!(g.candidates(), ["cas-src", "petra-stella-cloud"]);
    }

    /// The M27 rule that reached cas-src is refused there, and allowed in
    /// gabber-studio's own store.
    #[test]
    fn gabber_rule_is_foreign_in_cas_src_only() {
        let refusal = guard("cas-src")
            .check_rule(GABBER_RULE, &[])
            .expect("refused in cas-src");
        assert_eq!(refusal.projects, ["gabber-studio"]);
        assert!(
            guard("gabber-studio")
                .check_rule(GABBER_RULE, &[])
                .is_none()
        );
    }

    #[test]
    fn slug_forms_match_at_word_boundaries_only() {
        let g = guard("cas-src");
        for text in [
            "see ~/Petrastella/gabber-studio/docs",
            "the GABBER_STUDIO env",
            "Gabber Studio's staging branch",
            "petra-stella-cloud relay",
        ] {
            assert!(!g.foreign_mentions(&[text], &[]).is_empty(), "{text}");
        }
        for text in [
            "gabber studio in lower-case prose is not a name",
            "my-gabber-studio-fork",
            "gabber-studioX",
            "Petrastella company notes",
            "an ozer note on the canvas",
        ] {
            assert!(g.foreign_mentions(&[text], &[]).is_empty(), "{text}");
        }
    }

    #[test]
    fn project_marker_acknowledges_an_explicit_scope() {
        let g = guard("cas-src");
        let tags = vec!["git".to_string(), "Project:Gabber-Studio".to_string()];
        assert!(g.check_rule(GABBER_RULE, &tags).is_none());
        // A marker for a different project acknowledges nothing here.
        let tags = vec!["project:petra-stella-cloud".to_string()];
        assert!(g.check_rule(GABBER_RULE, &tags).is_some());
    }

    #[test]
    fn knowledge_page_title_and_body_are_both_checked() {
        let g = guard("cas-src");
        assert!(
            g.check_knowledge_page("Project gabber-studio codemap", "body", &[])
                .is_some()
        );
        assert!(
            g.check_knowledge_page("PostHog", "Used by Gabber Studio.", &[])
                .is_some()
        );
        let sources = vec!["project:gabber-studio".to_string()];
        assert!(
            g.check_knowledge_page("PostHog", "Used by Gabber Studio.", &sources)
                .is_none()
        );
    }

    #[test]
    fn refusal_message_is_actionable() {
        let text = guard("cas-src")
            .check_rule(GABBER_RULE, &[])
            .unwrap()
            .to_string();
        assert!(text.contains("gabber-studio"), "{text}");
        assert!(text.contains("'cas-src'"), "{text}");
        assert!(text.contains("`project:gabber-studio`"), "{text}");
        assert!(text.contains("`tags`"), "{text}");
    }

    #[test]
    fn empty_registry_is_inert() {
        let g = ForeignProjectGuard::new("cas-src", Vec::new());
        assert!(g.check_rule(GABBER_RULE, &[]).is_none());
    }
}
