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
    use crate::cli::init::{CAS_SECTION_BEGIN, CAS_SECTION_END, build_cas_section};

    let claude_md_path = project_root.join("CLAUDE.md");
    let new_section = build_cas_section();

    if claude_md_path.exists() {
        let content = std::fs::read_to_string(&claude_md_path).ok()?;

        // Check for marked section
        if let (Some(begin_pos), Some(end_pos)) = (
            content.find(CAS_SECTION_BEGIN),
            content.find(CAS_SECTION_END),
        ) {
            let before = &content[..begin_pos];
            let after = &content[end_pos + CAS_SECTION_END.len()..];
            let new_content = format!(
                "{}{}{}",
                before.trim_end(),
                if before.is_empty() { "" } else { "\n" },
                new_section
            );
            let new_content = format!("{new_content}{after}");

            if new_content != content {
                return Some(FileChange::modify(
                    std::path::PathBuf::from("CLAUDE.md"),
                    content,
                    new_content,
                    "Update Cassy section in CLAUDE.md",
                ));
            }
        } else if content.contains("IMPORTANT: USE Cassy FOR TASK AND MEMORY MANAGEMENT") {
            // Migration from old format
            let new_content = if content.starts_with("# IMPORTANT: USE Cassy") {
                if let Some(pos) = content.find("---\n\n") {
                    format!("{}\n\n{}", new_section, &content[pos + 5..])
                } else if let Some(pos) = content.find("---\n") {
                    format!("{}\n\n{}", new_section, &content[pos + 4..])
                } else {
                    format!("{new_section}\n\n{content}")
                }
            } else {
                format!("{new_section}\n\n{content}")
            };
            return Some(FileChange::modify(
                std::path::PathBuf::from("CLAUDE.md"),
                content,
                new_content,
                "Migrate Cassy section format in CLAUDE.md",
            ));
        } else {
            // Prepend new section
            let new_content = format!("{new_section}\n\n{content}");
            return Some(FileChange::modify(
                std::path::PathBuf::from("CLAUDE.md"),
                content,
                new_content,
                "Add Cassy section to CLAUDE.md",
            ));
        }
    } else {
        // Create new file
        return Some(FileChange::create(
            std::path::PathBuf::from("CLAUDE.md"),
            format!("{new_section}\n"),
            "Create CLAUDE.md with Cassy section",
        ));
    }

    None
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
