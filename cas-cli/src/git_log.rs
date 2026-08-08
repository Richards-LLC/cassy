//! NUL-safe parsers for `git log` porcelain output.
//!
//! Factored out of the codemap staleness check (`hooks::handlers::handlers_events::codemap`)
//! so the structural git-history walker (`crate::history`) reuses it rather
//! than forking a second, subtly-different parser (spec §1.9: "git-log-since-watermark —
//! factor out, reuse").
//!
//! Everything here takes `-z` (NUL-delimited) output. That is not decoration:
//! without it a path containing a newline, a quote or a non-UTF-8 byte comes
//! back C-quoted and the naive line parser silently produces a wrong path.
//!
//! # Wire shapes (verified against git, not assumed)
//!
//! `--name-status -z`:
//! - ordinary: `STATUS\0path\0`
//! - rename/copy: `R100\0old\0new\0` (likewise `C###`)
//!
//! `--numstat -z`:
//! - ordinary: `ins\tdel\tpath\0`
//! - rename/copy: `ins\tdel\t\0old\0new\0`
//! - binary files report `-` for both counts, which is preserved as `None`
//!   rather than coerced to `0` — "binary" and "changed nothing" are different
//!   facts.
//!
//! **`--name-status` and `--numstat` cannot be combined in one `git log`**: the
//! later flag wins and the other's data is silently absent. Callers needing
//! both must run two passes and join on the (new) path.

/// One entry from `git log --name-status -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameStatusEntry {
    /// Raw git status letter, similarity score stripped: `A`, `M`, `D`, `R`,
    /// `C`, `T`, `U`, `X`.
    pub status: String,
    /// The path as of this commit (the *new* path for renames/copies).
    pub path: String,
    /// Source path, renames and copies only.
    pub old_path: Option<String>,
}

/// One entry from `git log --numstat -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumstatEntry {
    /// `None` for binary files (git prints `-`).
    pub insertions: Option<i64>,
    /// `None` for binary files.
    pub deletions: Option<i64>,
    /// The path as of this commit (the *new* path for renames/copies).
    pub path: String,
    /// Source path, renames and copies only.
    pub old_path: Option<String>,
}

/// Strip the record noise git leaves between commits (`\n`, `\r`) from a token.
fn clean(token: &str) -> &str {
    token.trim_matches(|c: char| c == '\n' || c == '\r')
}

/// Parse `git log --name-status -z` output.
///
/// Unknown or malformed tokens are skipped rather than aborting the parse: a
/// single odd entry must not cost the caller the whole commit range.
pub fn parse_name_status_z(raw: &str) -> Vec<NameStatusEntry> {
    let parts: Vec<&str> = raw.split('\0').collect();
    let mut entries = Vec::new();
    let mut i = 0;

    while i < parts.len() {
        let status_token = clean(parts[i]);
        if status_token.is_empty() {
            i += 1;
            continue;
        }

        // A status token is a letter optionally followed by a similarity score.
        let Some(letter) = status_token.chars().next() else {
            i += 1;
            continue;
        };
        if !letter.is_ascii_alphabetic() {
            // Not a status field — most likely the format header of the next
            // commit, which the caller is responsible for splitting off.
            i += 1;
            continue;
        }

        let is_pair = matches!(letter, 'R' | 'C');
        let needed = if is_pair { 2 } else { 1 };
        if i + needed >= parts.len() {
            break;
        }

        if is_pair {
            let old = clean(parts[i + 1]).trim();
            let new = clean(parts[i + 2]).trim();
            if !old.is_empty() && !new.is_empty() {
                entries.push(NameStatusEntry {
                    status: letter.to_string(),
                    path: new.to_string(),
                    old_path: Some(old.to_string()),
                });
            }
            i += 3;
        } else {
            let path = clean(parts[i + 1]).trim();
            if !path.is_empty() {
                entries.push(NameStatusEntry {
                    status: letter.to_string(),
                    path: path.to_string(),
                    old_path: None,
                });
            }
            i += 2;
        }
    }

    entries
}

/// Parse `git log --numstat -z` output.
pub fn parse_numstat_z(raw: &str) -> Vec<NumstatEntry> {
    let parts: Vec<&str> = raw.split('\0').collect();
    let mut entries = Vec::new();
    let mut i = 0;

    while i < parts.len() {
        let token = clean(parts[i]);
        if token.is_empty() {
            i += 1;
            continue;
        }

        // `ins\tdel\tpath` (ordinary) or `ins\tdel\t` (rename: paths follow).
        let mut fields = token.splitn(3, '\t');
        let (Some(ins), Some(dels), Some(rest)) = (fields.next(), fields.next(), fields.next())
        else {
            i += 1;
            continue;
        };

        let count = |v: &str| -> Option<i64> { v.trim().parse::<i64>().ok() };
        let insertions = count(ins);
        let deletions = count(dels);
        // A leading token that parses as neither a count nor `-` is not a
        // numstat row (e.g. a commit header the caller left in).
        if insertions.is_none() && ins.trim() != "-" {
            i += 1;
            continue;
        }

        let rest = rest.trim();
        if rest.is_empty() {
            // Rename/copy form: the old and new paths are the next two tokens.
            if i + 2 >= parts.len() {
                break;
            }
            let old = clean(parts[i + 1]).trim();
            let new = clean(parts[i + 2]).trim();
            if !old.is_empty() && !new.is_empty() {
                entries.push(NumstatEntry {
                    insertions,
                    deletions,
                    path: new.to_string(),
                    old_path: Some(old.to_string()),
                });
            }
            i += 3;
        } else {
            entries.push(NumstatEntry {
                insertions,
                deletions,
                path: rest.to_string(),
                old_path: None,
            });
            i += 1;
        }
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ordinary_name_status_entries() {
        let raw = "A\0src/new.rs\0M\0src/old.rs\0D\0src/gone.rs\0";
        let entries = parse_name_status_z(raw);
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.status.as_str(), e.path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("A", "src/new.rs"),
                ("M", "src/old.rs"),
                ("D", "src/gone.rs"),
            ]
        );
        assert!(entries.iter().all(|e| e.old_path.is_none()));
    }

    /// The three-token rename form is the one a line-based parser gets wrong.
    #[test]
    fn parses_rename_pairs_with_similarity_scores() {
        let raw = "R100\0old/a.rs\0new/a.rs\0C075\0src/b.rs\0src/c.rs\0M\0after.rs\0";
        let entries = parse_name_status_z(raw);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].status, "R");
        assert_eq!(entries[0].old_path.as_deref(), Some("old/a.rs"));
        assert_eq!(entries[0].path, "new/a.rs");
        assert_eq!(entries[1].status, "C");
        assert_eq!(entries[1].old_path.as_deref(), Some("src/b.rs"));
        // Crucially, the entry AFTER a rename is still aligned.
        assert_eq!(entries[2].status, "M");
        assert_eq!(entries[2].path, "after.rs");
    }

    /// Paths containing a newline are exactly why `-z` is used; the parser must
    /// not treat the embedded newline as a record boundary.
    #[test]
    fn preserves_paths_with_embedded_newlines() {
        let raw = "A\0weird\nname.rs\0";
        let entries = parse_name_status_z(raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "weird\nname.rs");
    }

    #[test]
    fn skips_inter_commit_newlines_in_name_status() {
        let raw = "\nA\0a.rs\0\nM\0b.rs\0";
        let entries = parse_name_status_z(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "a.rs");
        assert_eq!(entries[1].path, "b.rs");
    }

    #[test]
    fn truncated_name_status_output_does_not_panic() {
        assert!(parse_name_status_z("R100\0only-old\0").is_empty());
        assert!(parse_name_status_z("A\0").is_empty());
        assert!(parse_name_status_z("").is_empty());
    }

    #[test]
    fn parses_ordinary_numstat_entries() {
        let raw = "3\t1\tsrc/a.rs\0120\t0\tsrc/b.rs\0";
        let entries = parse_numstat_z(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].insertions, Some(3));
        assert_eq!(entries[0].deletions, Some(1));
        assert_eq!(entries[0].path, "src/a.rs");
        assert_eq!(entries[1].insertions, Some(120));
    }

    #[test]
    fn parses_numstat_rename_form() {
        let raw = "2\t0\t\0docs/old.md\0docs/new.md\05\t5\tsrc/after.rs\0";
        let entries = parse_numstat_z(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "docs/new.md");
        assert_eq!(entries[0].old_path.as_deref(), Some("docs/old.md"));
        assert_eq!(entries[0].insertions, Some(2));
        // Alignment survives the three-token rename.
        assert_eq!(entries[1].path, "src/after.rs");
    }

    /// Binary files must stay `None`, not become 0.
    #[test]
    fn binary_numstat_counts_stay_none() {
        let entries = parse_numstat_z("-\t-\tlogo.png\0");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].insertions.is_none());
        assert!(entries[0].deletions.is_none());
        assert_eq!(entries[0].path, "logo.png");
    }

    #[test]
    fn numstat_ignores_non_numstat_tokens() {
        // A stray commit header must not become a bogus file row.
        let raw = "deadbeef\tnot\tnumstat-ish\0";
        assert!(parse_numstat_z(raw).is_empty());
        assert!(parse_numstat_z("").is_empty());
    }
}
