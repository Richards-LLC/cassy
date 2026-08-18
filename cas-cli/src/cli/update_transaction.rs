//! Update transaction system for atomic updates with rollback support
//!
//! Provides a transaction-like wrapper around `cas update` operations that:
//! - Computes all changes upfront for dry-run preview
//! - Creates backups before applying changes
//! - Rolls back all changes on any failure
//! - Shows unified diffs for file changes

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use similar::{ChangeTag, TextDiff};

use crate::error::CasError;
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

/// A single file change to be applied
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path to the file (relative to project root for display)
    pub path: PathBuf,
    /// Original content (None if file doesn't exist)
    pub old_content: Option<String>,
    /// New content (None if file should be deleted)
    pub new_content: Option<String>,
    /// Human-readable description of the change
    pub description: String,
}

impl FileChange {
    /// Create a new file (didn't exist before)
    pub fn create(path: PathBuf, content: String, description: impl Into<String>) -> Self {
        Self {
            path,
            old_content: None,
            new_content: Some(content),
            description: description.into(),
        }
    }

    /// Modify an existing file
    pub fn modify(path: PathBuf, old: String, new: String, description: impl Into<String>) -> Self {
        Self {
            path,
            old_content: Some(old),
            new_content: Some(new),
            description: description.into(),
        }
    }

    /// Delete an existing file
    pub fn delete(path: PathBuf, old: String, description: impl Into<String>) -> Self {
        Self {
            path,
            old_content: Some(old),
            new_content: None,
            description: description.into(),
        }
    }

    /// Check if this change would actually modify anything
    pub fn has_changes(&self) -> bool {
        match (&self.old_content, &self.new_content) {
            (Some(old), Some(new)) => old != new,
            (None, Some(_)) => true, // Creating new file
            (Some(_), None) => true, // Deleting file
            (None, None) => false,   // No-op
        }
    }

    /// Get the change type as a string
    pub fn change_type(&self) -> &'static str {
        match (&self.old_content, &self.new_content) {
            (None, Some(_)) => "create",
            (Some(_), Some(_)) => "modify",
            (Some(_), None) => "delete",
            (None, None) => "none",
        }
    }

    /// Generate unified diff output
    pub fn unified_diff(&self) -> String {
        let old = self.old_content.as_deref().unwrap_or("");
        let new = self.new_content.as_deref().unwrap_or("");

        let diff = TextDiff::from_lines(old, new);
        let mut output = String::new();

        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            output.push_str(sign);
            output.push_str(change.value());
            if !change.value().ends_with('\n') {
                output.push('\n');
            }
        }

        output
    }

    /// Print colored diff to the given formatter
    pub fn print_diff(&self, fmt: &mut Formatter) -> io::Result<()> {
        let path_display = self.path.display();
        let success_color = fmt.theme().palette.status_success;
        let error_color = fmt.theme().palette.status_error;
        let accent_color = fmt.theme().palette.accent;
        let muted_color = fmt.theme().palette.text_muted;
        let primary_color = fmt.theme().palette.text_primary;

        match (&self.old_content, &self.new_content) {
            (None, Some(_)) => {
                fmt.write_colored("+++", success_color)?;
                fmt.write_raw(" ")?;
                fmt.write_colored(&path_display.to_string(), accent_color)?;
                fmt.write_raw(" (new file)")?;
                fmt.newline()?;
            }
            (Some(_), None) => {
                fmt.write_colored("---", error_color)?;
                fmt.write_raw(" ")?;
                fmt.write_colored(&path_display.to_string(), accent_color)?;
                fmt.write_raw(" (deleted)")?;
                fmt.newline()?;
            }
            (Some(_), Some(_)) => {
                fmt.write_colored("---", error_color)?;
                fmt.write_raw(" ")?;
                fmt.write_colored(&format!("a/{path_display}"), accent_color)?;
                fmt.newline()?;
                fmt.write_colored("+++", success_color)?;
                fmt.write_raw(" ")?;
                fmt.write_colored(&format!("b/{path_display}"), accent_color)?;
                fmt.newline()?;
            }
            (None, None) => return Ok(()),
        }

        let old = self.old_content.as_deref().unwrap_or("");
        let new = self.new_content.as_deref().unwrap_or("");

        let diff = TextDiff::from_lines(old, new);

        for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
            if idx > 0 {
                fmt.write_colored("...", muted_color)?;
                fmt.newline()?;
            }
            for op in group {
                for change in diff.iter_changes(op) {
                    let (sign, color) = match change.tag() {
                        ChangeTag::Delete => ("-", error_color),
                        ChangeTag::Insert => ("+", success_color),
                        ChangeTag::Equal => (" ", primary_color),
                    };
                    fmt.write_colored(sign, color)?;
                    fmt.write_colored(change.value(), color)?;
                    if change.missing_newline() {
                        fmt.newline()?;
                    }
                }
            }
        }
        fmt.newline()
    }
}

/// A database migration change
#[derive(Debug, Clone)]
pub struct MigrationChange {
    /// Migration ID
    pub id: u32,
    /// Migration name
    pub name: String,
    /// SQL statements to execute
    pub sql: Vec<String>,
    /// Human-readable description
    pub description: String,
}

impl MigrationChange {
    /// Print the migration details
    pub fn print(&self, fmt: &mut Formatter) -> io::Result<()> {
        fmt.write_raw(&format!("  {:>3}. ", self.id))?;
        fmt.write_accent(&self.name)?;
        fmt.write_raw(" [migration]")?;
        fmt.newline()?;
        fmt.write_raw("       ")?;
        fmt.write_muted(&self.description)?;
        fmt.newline()?;
        for sql in &self.sql {
            let preview = if sql.len() > 60 {
                format!("{}...", &sql[..57])
            } else {
                sql.clone()
            };
            fmt.write_raw("       ")?;
            fmt.write_muted(&preview)?;
            fmt.newline()?;
        }
        Ok(())
    }
}

/// Transaction state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    /// Initial state, collecting changes
    Pending,
    /// Backups created, ready to apply
    BackedUp,
    /// Changes applied successfully
    Committed,
    /// Changes rolled back
    RolledBack,
    /// Transaction failed
    Failed,
}

/// Update transaction for atomic updates
pub struct UpdateTransaction {
    /// Project root directory
    project_root: PathBuf,
    /// Cassy directory (.cas)
    cas_dir: PathBuf,
    /// Backup directory for this transaction
    backup_dir: Option<PathBuf>,
    /// File changes to apply
    file_changes: Vec<FileChange>,
    /// Database migrations to apply
    migrations: Vec<MigrationChange>,
    /// Current state
    state: TransactionState,
    /// Backed up file paths (original path -> backup path)
    backups: HashMap<PathBuf, PathBuf>,
    /// Whether to keep backups on success
    keep_backup: bool,
}

impl UpdateTransaction {
    /// Create a new update transaction
    pub fn new(project_root: &Path, cas_dir: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            cas_dir: cas_dir.to_path_buf(),
            backup_dir: None,
            file_changes: Vec::new(),
            migrations: Vec::new(),
            state: TransactionState::Pending,
            backups: HashMap::new(),
            keep_backup: false,
        }
    }

    /// Set whether to keep backups on success
    pub fn keep_backup(mut self, keep: bool) -> Self {
        self.keep_backup = keep;
        self
    }

    /// Add a file change to the transaction
    pub fn add_file_change(&mut self, change: FileChange) {
        if change.has_changes() {
            self.file_changes.push(change);
        }
    }

    /// Add a migration to the transaction
    pub fn add_migration(&mut self, migration: MigrationChange) {
        self.migrations.push(migration);
    }

    /// Check if there are any changes to apply
    pub fn has_changes(&self) -> bool {
        !self.file_changes.is_empty() || !self.migrations.is_empty()
    }

    /// Get the number of file changes
    pub fn file_change_count(&self) -> usize {
        self.file_changes.len()
    }

    /// Get the number of migrations
    pub fn migration_count(&self) -> usize {
        self.migrations.len()
    }

    /// Get all file changes
    pub fn file_changes(&self) -> &[FileChange] {
        &self.file_changes
    }

    /// Get all migrations
    pub fn migrations(&self) -> &[MigrationChange] {
        &self.migrations
    }

    /// Print dry-run preview of all changes
    pub fn print_dry_run(&self, fmt: &mut Formatter) -> io::Result<()> {
        if self.migrations.is_empty() && self.file_changes.is_empty() {
            fmt.success("No changes to apply")?;
            return Ok(());
        }

        fmt.subheading("Update Preview (dry run)")?;
        let sep = "=".repeat(50);
        fmt.write_raw(&sep)?;
        fmt.newline()?;
        fmt.newline()?;

        // Show migrations
        if !self.migrations.is_empty() {
            fmt.write_bold(&format!(
                "Schema Migrations ({} pending)",
                self.migrations.len()
            ))?;
            fmt.newline()?;
            fmt.newline()?;
            for migration in &self.migrations {
                migration.print(fmt)?;
            }
            fmt.newline()?;
        }

        // Show file changes grouped by type
        if !self.file_changes.is_empty() {
            fmt.write_bold(&format!("File Changes ({} files)", self.file_changes.len()))?;
            fmt.newline()?;
            fmt.newline()?;

            // Group by directory for cleaner output
            let mut creates = Vec::new();
            let mut modifies = Vec::new();
            let mut deletes = Vec::new();

            for change in &self.file_changes {
                match change.change_type() {
                    "create" => creates.push(change),
                    "modify" => modifies.push(change),
                    "delete" => deletes.push(change),
                    _ => {}
                }
            }

            let success_color = fmt.theme().palette.status_success;
            let warning_color = fmt.theme().palette.status_warning;
            let error_color = fmt.theme().palette.status_error;

            if !creates.is_empty() {
                fmt.write_colored("  \u{25CF} ", success_color)?;
                fmt.write_raw("New files:")?;
                fmt.newline()?;
                for change in creates {
                    fmt.write_colored("    + ", success_color)?;
                    fmt.write_raw(&change.path.display().to_string())?;
                    fmt.newline()?;
                }
                fmt.newline()?;
            }

            if !modifies.is_empty() {
                fmt.write_colored("  \u{25CF} ", warning_color)?;
                fmt.write_raw("Modified files:")?;
                fmt.newline()?;
                for change in &modifies {
                    fmt.write_colored("    ~ ", warning_color)?;
                    fmt.write_raw(&change.path.display().to_string())?;
                    fmt.newline()?;
                }
                fmt.newline()?;

                // Show diffs for modified files
                fmt.subheading("Diffs:")?;
                fmt.newline()?;
                for change in modifies {
                    change.print_diff(fmt)?;
                }
            }

            if !deletes.is_empty() {
                fmt.write_colored("  \u{25CF} ", error_color)?;
                fmt.write_raw("Deleted files:")?;
                fmt.newline()?;
                for change in deletes {
                    fmt.write_colored("    - ", error_color)?;
                    fmt.write_raw(&change.path.display().to_string())?;
                    fmt.newline()?;
                }
                fmt.newline()?;
            }
        }

        fmt.write_raw("Run ")?;
        fmt.write_accent("cas update --schema-only")?;
        fmt.write_raw(" to apply these changes.")?;
        fmt.newline()
    }

    /// Create backups of all files that will be modified
    pub fn backup(&mut self) -> Result<(), CasError> {
        if self.state != TransactionState::Pending {
            return Err(CasError::InvalidState(
                "Transaction not in pending state".to_string(),
            ));
        }

        // Create backup directory with timestamp
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let backup_dir = self.cas_dir.join("backup").join(timestamp.to_string());
        fs::create_dir_all(&backup_dir)?;

        // Backup database
        let db_path = self.cas_dir.join("cas.db");
        if db_path.exists() && !self.migrations.is_empty() {
            let backup_db = backup_dir.join("cas.db");
            fs::copy(&db_path, &backup_db)?;
            self.backups.insert(db_path, backup_db);
        }

        // Backup files that will be modified or deleted
        for change in &self.file_changes {
            if change.old_content.is_some() {
                let full_path = if change.path.is_absolute() {
                    change.path.clone()
                } else {
                    self.project_root.join(&change.path)
                };

                if full_path.exists() {
                    // Create relative backup path
                    let rel_path = full_path
                        .strip_prefix(&self.project_root)
                        .unwrap_or(&change.path);
                    let backup_path = backup_dir.join(rel_path);

                    if let Some(parent) = backup_path.parent() {
                        fs::create_dir_all(parent)?;
                    }

                    fs::copy(&full_path, &backup_path)?;
                    self.backups.insert(full_path, backup_path);
                }
            }
        }

        self.backup_dir = Some(backup_dir);
        self.state = TransactionState::BackedUp;
        Ok(())
    }

    /// Apply all file changes (migrations are handled separately)
    pub fn apply_file_changes(&mut self) -> Result<(), CasError> {
        if self.state != TransactionState::BackedUp {
            return Err(CasError::InvalidState(
                "Transaction not backed up".to_string(),
            ));
        }

        for change in &self.file_changes {
            let full_path = if change.path.is_absolute() {
                change.path.clone()
            } else {
                self.project_root.join(&change.path)
            };

            match &change.new_content {
                Some(content) => {
                    // Create parent directories if needed
                    if let Some(parent) = full_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&full_path, content)?;
                }
                None => {
                    // Delete file if it exists
                    if full_path.exists() {
                        fs::remove_file(&full_path)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Mark transaction as committed and optionally cleanup backups
    pub fn commit(&mut self) -> Result<(), CasError> {
        self.state = TransactionState::Committed;

        // Remove backup directory unless keep_backup is set
        if !self.keep_backup {
            if let Some(ref backup_dir) = self.backup_dir {
                if backup_dir.exists() {
                    fs::remove_dir_all(backup_dir)?;
                }
            }
        }

        Ok(())
    }

    /// Rollback all changes from backups
    pub fn rollback(&mut self) -> Result<(), CasError> {
        if self.state == TransactionState::RolledBack {
            return Ok(());
        }

        let mut errors = Vec::new();

        // Restore all backed up files
        for (original, backup) in &self.backups {
            if backup.exists() {
                if let Err(e) = fs::copy(backup, original) {
                    errors.push(format!("Failed to restore {}: {}", original.display(), e));
                }
            }
        }

        // Delete any newly created files
        for change in &self.file_changes {
            if change.old_content.is_none() {
                // This was a new file, delete it
                let full_path = if change.path.is_absolute() {
                    change.path.clone()
                } else {
                    self.project_root.join(&change.path)
                };

                if full_path.exists() {
                    if let Err(e) = fs::remove_file(&full_path) {
                        errors.push(format!(
                            "Failed to remove new file {}: {}",
                            full_path.display(),
                            e
                        ));
                    }
                }
            }
        }

        self.state = TransactionState::RolledBack;

        if errors.is_empty() {
            Ok(())
        } else {
            Err(CasError::RollbackFailed(errors.join("; ")))
        }
    }

    /// Get the backup directory path
    pub fn backup_dir(&self) -> Option<&Path> {
        self.backup_dir.as_deref()
    }

    /// Get current state
    pub fn state(&self) -> TransactionState {
        self.state
    }
}

impl Drop for UpdateTransaction {
    fn drop(&mut self) {
        // If transaction was backed up but not committed or rolled back,
        // attempt automatic rollback
        if self.state == TransactionState::BackedUp {
            let mut err = io::stderr();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut err, theme);
            let _ =
                fmt.warning("Transaction dropped without commit/rollback, attempting rollback...");
            if let Err(e) = self.rollback() {
                let _ = fmt.error(&format!("Rollback failed: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cli::update_transaction::*;
    use tempfile::TempDir;

    #[test]
    fn test_file_change_has_changes() {
        let create = FileChange::create(PathBuf::from("test.txt"), "content".to_string(), "test");
        assert!(create.has_changes());

        let modify = FileChange::modify(
            PathBuf::from("test.txt"),
            "old".to_string(),
            "new".to_string(),
            "test",
        );
        assert!(modify.has_changes());

        let no_change = FileChange::modify(
            PathBuf::from("test.txt"),
            "same".to_string(),
            "same".to_string(),
            "test",
        );
        assert!(!no_change.has_changes());

        let delete = FileChange::delete(PathBuf::from("test.txt"), "content".to_string(), "test");
        assert!(delete.has_changes());
    }

    #[test]
    fn test_unified_diff() {
        let change = FileChange::modify(
            PathBuf::from("test.txt"),
            "line1\nline2\nline3\n".to_string(),
            "line1\nmodified\nline3\n".to_string(),
            "test",
        );

        let diff = change.unified_diff();
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+modified"));
    }

    #[test]
    fn test_transaction_backup_and_rollback() {
        let temp = TempDir::new().unwrap();
        let project_root = temp.path();
        let cas_dir = project_root.join(".cas");
        fs::create_dir_all(&cas_dir).unwrap();

        // Create a file to modify
        let test_file = project_root.join("test.txt");
        fs::write(&test_file, "original content").unwrap();

        let mut tx = UpdateTransaction::new(project_root, &cas_dir);

        tx.add_file_change(FileChange::modify(
            PathBuf::from("test.txt"),
            "original content".to_string(),
            "new content".to_string(),
            "test",
        ));

        // Backup
        tx.backup().unwrap();
        assert_eq!(tx.state(), TransactionState::BackedUp);

        // Apply changes
        tx.apply_file_changes().unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "new content");

        // Rollback
        tx.rollback().unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "original content");
        assert_eq!(tx.state(), TransactionState::RolledBack);
    }

    #[test]
    fn test_transaction_commit() {
        let temp = TempDir::new().unwrap();
        let project_root = temp.path();
        let cas_dir = project_root.join(".cas");
        fs::create_dir_all(&cas_dir).unwrap();

        // Create a file to modify
        let test_file = project_root.join("test.txt");
        fs::write(&test_file, "original").unwrap();

        let mut tx = UpdateTransaction::new(project_root, &cas_dir);

        tx.add_file_change(FileChange::modify(
            PathBuf::from("test.txt"),
            "original".to_string(),
            "modified".to_string(),
            "test",
        ));

        tx.backup().unwrap();
        let backup_dir = tx.backup_dir().unwrap().to_path_buf();
        assert!(backup_dir.exists());

        tx.apply_file_changes().unwrap();
        tx.commit().unwrap();

        // Backup should be cleaned up
        assert!(!backup_dir.exists());
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "modified");
    }

    #[test]
    fn test_automatic_rollback_on_drop() {
        let temp = TempDir::new().unwrap();
        let project_root = temp.path();
        let cas_dir = project_root.join(".cas");
        fs::create_dir_all(&cas_dir).unwrap();

        // Create a file to modify
        let test_file = project_root.join("test.txt");
        fs::write(&test_file, "original content").unwrap();

        // Scope block to test Drop behavior
        {
            let mut tx = UpdateTransaction::new(project_root, &cas_dir);

            tx.add_file_change(FileChange::modify(
                PathBuf::from("test.txt"),
                "original content".to_string(),
                "modified content".to_string(),
                "test",
            ));

            // Backup and apply changes
            tx.backup().unwrap();
            assert_eq!(tx.state(), TransactionState::BackedUp);
            tx.apply_file_changes().unwrap();

            // Verify file was modified
            assert_eq!(fs::read_to_string(&test_file).unwrap(), "modified content");

            // Drop tx here WITHOUT calling commit() or rollback()
            // The Drop impl should automatically rollback
        }

        // After Drop, file should be restored to original content
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "original content");
    }
}
