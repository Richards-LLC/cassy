use std::io;
use std::path::Path;

use similar::{ChangeTag, TextDiff};

use crate::builtins::{preview_all_builtins, preview_all_codex_builtins};
use crate::cli::Cli;
use crate::cli::update_transaction::{FileChange, MigrationChange, UpdateTransaction};
use crate::migration::MigrationStatus;
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

pub(crate) fn compute_claude_md_change(project_root: &Path) -> Option<FileChange> {
    use crate::cli::init::{ClaudeMdPlan, plan_claude_md};

    // Same decision `update_claude_md` applies, so the dry run cannot promise
    // a change (e.g. adding a block below an ancestor that has it) that apply
    // will not make.
    let path = std::path::PathBuf::from("CLAUDE.md");
    match plan_claude_md(project_root).ok()? {
        ClaudeMdPlan::Unchanged => None,
        ClaudeMdPlan::Create { content } => Some(FileChange::create(
            path,
            content,
            "Create CLAUDE.md with Cassy section",
        )),
        ClaudeMdPlan::Modify {
            old,
            new,
            description,
        } => Some(FileChange::modify(path, old, new, description)),
        ClaudeMdPlan::Delete { old } => Some(FileChange::delete(
            path,
            old,
            "Delete CLAUDE.md: it held only a Cassy section an ancestor CLAUDE.md already carries",
        )),
    }
}

/// Compute what Cassy skill changes would be made (without applying)
pub(crate) fn compute_cas_skill_change(project_root: &Path) -> Option<FileChange> {
    use crate::cli::init::{CAS_SKILL, is_old_cas_skill, is_skill_managed_by_cas};

    let skill_path = project_root.join(".claude/skills/cas/SKILL.md");
    let skill_content = CAS_SKILL;

    if skill_path.exists() {
        let existing = std::fs::read_to_string(&skill_path).ok()?;

        if existing == skill_content {
            return None; // No change needed
        }

        // Only update if managed by Cassy or old format
        if is_skill_managed_by_cas(&existing) || is_old_cas_skill(&existing) {
            return Some(FileChange::modify(
                std::path::PathBuf::from(".claude/skills/cas/SKILL.md"),
                existing,
                skill_content.to_string(),
                "Update Cassy skill definition",
            ));
        }

        None // User-customized, don't touch
    } else {
        Some(FileChange::create(
            std::path::PathBuf::from(".claude/skills/cas/SKILL.md"),
            skill_content.to_string(),
            "Create Cassy skill definition",
        ))
    }
}

/// Build an UpdateTransaction with all pending changes
pub(crate) fn build_update_transaction(
    project_root: &Path,
    cas_dir: &Path,
    status: &MigrationStatus,
    keep_backup: bool,
) -> UpdateTransaction {
    let mut tx = UpdateTransaction::new(project_root, cas_dir).keep_backup(keep_backup);

    // Add pending migrations
    for migration in &status.pending {
        tx.add_migration(MigrationChange {
            id: migration.id,
            name: migration.name.to_string(),
            sql: migration.up.iter().map(|s| s.to_string()).collect(),
            description: migration.description.to_string(),
        });
    }

    // Compute CLAUDE.md changes
    if let Some(change) = compute_claude_md_change(project_root) {
        tx.add_file_change(change);
    }

    // Compute Cassy skill changes
    if let Some(change) = compute_cas_skill_change(project_root) {
        tx.add_file_change(change);
    }

    tx
}

/// Render a colored diff using Formatter
fn render_diff(
    fmt: &mut Formatter,
    prefix: &str,
    path: &str,
    old_content: &str,
    new_content: &str,
) -> io::Result<()> {
    let error_color = fmt.theme().palette.status_error;
    let success_color = fmt.theme().palette.status_success;
    let accent_color = fmt.theme().palette.accent;
    let muted_color = fmt.theme().palette.text_muted;
    let primary_color = fmt.theme().palette.text_primary;

    fmt.write_colored("---", error_color)?;
    fmt.write_raw(" ")?;
    fmt.write_colored(&format!("a/{prefix}{path}"), accent_color)?;
    fmt.newline()?;
    fmt.write_colored("+++", success_color)?;
    fmt.write_raw(" ")?;
    fmt.write_colored(&format!("b/{prefix}{path}"), accent_color)?;
    fmt.newline()?;

    let diff = TextDiff::from_lines(old_content, new_content);
    for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if idx > 0 {
            fmt.write_colored("...", muted_color)?;
            fmt.newline()?;
        }
        for op in group {
            for ch in diff.iter_changes(op) {
                let (sign, color) = match ch.tag() {
                    ChangeTag::Delete => ("-", error_color),
                    ChangeTag::Insert => ("+", success_color),
                    ChangeTag::Equal => (" ", primary_color),
                };
                fmt.write_colored(sign, color)?;
                fmt.write_colored(ch.value(), color)?;
                if ch.missing_newline() {
                    fmt.newline()?;
                }
            }
        }
    }
    fmt.newline()
}

/// Enhanced dry-run that shows migrations, file diffs, and builtin changes
pub(crate) fn show_enhanced_dry_run(
    tx: &UpdateTransaction,
    status: &MigrationStatus,
    claude_dir: &std::path::Path,
    codex_dir: &std::path::Path,
    cli: &Cli,
) -> anyhow::Result<()> {
    // Get builtin changes
    let builtin_changes = preview_all_builtins(claude_dir).unwrap_or_default();
    let codex_builtin_changes = if codex_dir.exists() {
        preview_all_codex_builtins(codex_dir).unwrap_or_default()
    } else {
        Vec::new()
    };

    if cli.json {
        // JSON output for programmatic use
        let pending_json: Vec<String> = status
            .pending
            .iter()
            .map(|m| {
                format!(
                    r#"{{"id":{},"name":"{}","subsystem":"{}","description":"{}"}}"#,
                    m.id, m.name, m.subsystem, m.description
                )
            })
            .collect();

        let file_changes_json: Vec<String> = tx
            .file_changes()
            .iter()
            .map(|c| {
                format!(
                    r#"{{"path":"{}","type":"{}","description":"{}"}}"#,
                    c.path.display(),
                    c.change_type(),
                    c.description
                )
            })
            .collect();

        let builtin_changes_json: Vec<String> = builtin_changes
            .iter()
            .map(|c| {
                format!(
                    r#"{{"path":"{}","type":"{}"}}"#,
                    c.path,
                    if c.is_new { "create" } else { "modify" }
                )
            })
            .collect();

        let codex_builtin_changes_json: Vec<String> = codex_builtin_changes
            .iter()
            .map(|c| {
                format!(
                    r#"{{"path":"{}","type":"{}"}}"#,
                    c.path,
                    if c.is_new { "create" } else { "modify" }
                )
            })
            .collect();

        println!(
            r#"{{"dry_run":true,"current_version":{},"latest_version":{},"pending_migrations":{},"file_changes":{},"builtin_changes":{},"codex_builtin_changes":{},"migrations":[{}],"files":[{}],"builtins":[{}],"codex_builtins":[{}]}}"#,
            status.current_version,
            status.latest_version,
            status.pending.len(),
            tx.file_change_count(),
            builtin_changes.len(),
            codex_builtin_changes.len(),
            pending_json.join(","),
            file_changes_json.join(","),
            builtin_changes_json.join(","),
            codex_builtin_changes_json.join(",")
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut out = io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme);

    // Use the transaction's dry-run display which shows diffs
    tx.print_dry_run(&mut fmt)?;

    // Show builtin changes
    if !builtin_changes.is_empty() {
        fmt.newline()?;
        fmt.write_bold(&format!(
            "Built-in Changes ({} files)",
            builtin_changes.len()
        ))?;
        fmt.newline()?;
        fmt.newline()?;

        let (new_files, modified_files): (Vec<_>, Vec<_>) =
            builtin_changes.iter().partition(|c| c.is_new);

        let success_color = fmt.theme().palette.status_success;
        let warning_color = fmt.theme().palette.status_warning;

        if !new_files.is_empty() {
            fmt.write_colored("  \u{25CF} ", success_color)?;
            fmt.write_raw("New built-ins:")?;
            fmt.newline()?;
            for change in &new_files {
                fmt.write_colored("    + ", success_color)?;
                fmt.write_raw(&format!(".claude/{}", change.path))?;
                fmt.newline()?;
            }
            fmt.newline()?;
        }

        if !modified_files.is_empty() {
            fmt.write_colored("  \u{25CF} ", warning_color)?;
            fmt.write_raw("Modified built-ins:")?;
            fmt.newline()?;
            for change in &modified_files {
                fmt.write_colored("    ~ ", warning_color)?;
                fmt.write_raw(&format!(".claude/{}", change.path))?;
                fmt.newline()?;
            }
            fmt.newline()?;

            // Show diffs
            fmt.subheading("Diffs:")?;
            fmt.newline()?;
            for change in &modified_files {
                render_diff(
                    &mut fmt,
                    ".claude/",
                    &change.path,
                    &change.old_content,
                    &change.new_content,
                )?;
            }
        }
    }

    if !codex_builtin_changes.is_empty() {
        fmt.newline()?;
        fmt.write_bold(&format!(
            "Codex Built-in Changes ({} files)",
            codex_builtin_changes.len()
        ))?;
        fmt.newline()?;
        fmt.newline()?;

        let (new_files, modified_files): (Vec<_>, Vec<_>) =
            codex_builtin_changes.iter().partition(|c| c.is_new);

        let success_color = fmt.theme().palette.status_success;
        let warning_color = fmt.theme().palette.status_warning;

        if !new_files.is_empty() {
            fmt.write_colored("  \u{25CF} ", success_color)?;
            fmt.write_raw("New built-ins:")?;
            fmt.newline()?;
            for change in &new_files {
                fmt.write_colored("    + ", success_color)?;
                fmt.write_raw(&format!(".codex/{}", change.path))?;
                fmt.newline()?;
            }
            fmt.newline()?;
        }

        if !modified_files.is_empty() {
            fmt.write_colored("  \u{25CF} ", warning_color)?;
            fmt.write_raw("Modified built-ins:")?;
            fmt.newline()?;
            for change in &modified_files {
                fmt.write_colored("    ~ ", warning_color)?;
                fmt.write_raw(&format!(".codex/{}", change.path))?;
                fmt.newline()?;
            }
            fmt.newline()?;

            // Show diffs
            fmt.subheading("Diffs:")?;
            fmt.newline()?;
            for change in &modified_files {
                render_diff(
                    &mut fmt,
                    ".codex/",
                    &change.path,
                    &change.old_content,
                    &change.new_content,
                )?;
            }
        }
    }

    if tx.has_changes() || !builtin_changes.is_empty() || !codex_builtin_changes.is_empty() {
        fmt.write_raw("Run ")?;
        fmt.write_accent("cas update --schema-only")?;
        fmt.write_raw(" to apply these changes.")?;
        fmt.newline()?;
    } else {
        fmt.success("No changes to apply")?;
    }

    Ok(())
}

#[cfg(test)]
mod claude_md_preview_tests {
    use super::compute_claude_md_change;
    use crate::cli::init::update_claude_md;
    use crate::test_support::TestEnvGuard;
    use std::fs;

    /// Skills audit L6 F6: `cas update --dry-run` promised to add a block below
    /// an ancestor that already had one, which apply never did. The preview
    /// and apply now share one plan; pin that they agree.
    #[test]
    fn dry_run_matches_apply_below_an_ancestor_block() {
        TestEnvGuard::run_with_temp_home(|home| {
            update_claude_md(home).unwrap();

            // No CLAUDE.md below the ancestor: neither side creates one.
            let bare = home.join("bare");
            fs::create_dir_all(&bare).unwrap();
            assert!(compute_claude_md_change(&bare).is_none());
            assert!(!update_claude_md(&bare).unwrap());
            assert!(!bare.join("CLAUDE.md").exists());

            // A duplicate block with other content: both prune it.
            let dup = home.join("dup");
            fs::create_dir_all(&dup).unwrap();
            let original = format!(
                "{}\n\n# Dup\n",
                fs::read_to_string(home.join("CLAUDE.md"))
                    .unwrap()
                    .trim_end()
            );
            fs::write(dup.join("CLAUDE.md"), &original).unwrap();
            let change = compute_claude_md_change(&dup).expect("preview prunes");
            assert_eq!(change.new_content.as_deref(), Some("# Dup\n"));
            assert!(update_claude_md(&dup).unwrap());
            assert_eq!(
                fs::read_to_string(dup.join("CLAUDE.md")).unwrap(),
                "# Dup\n"
            );

            // A block-only duplicate: both delete the file.
            let only = home.join("only");
            fs::create_dir_all(&only).unwrap();
            fs::copy(home.join("CLAUDE.md"), only.join("CLAUDE.md")).unwrap();
            let change = compute_claude_md_change(&only).expect("preview deletes");
            assert!(change.new_content.is_none(), "preview must plan a delete");
            assert!(update_claude_md(&only).unwrap());
            assert!(!only.join("CLAUDE.md").exists());
        });
    }
}
