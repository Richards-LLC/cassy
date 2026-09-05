//! CAS spec syncing
//!
//! Syncs approved specs to .cas/specs/ as markdown files.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CoreError;
use cas_types::{Spec, SpecStatus};

/// Syncs CAS specs to filesystem
pub struct SpecSyncer {
    target_dir: PathBuf,
}

/// Report of sync operation
#[derive(Debug, Default)]
pub struct SpecSyncReport {
    /// Number of specs synced
    pub synced: usize,
    /// Number of stale files removed
    pub removed: usize,
    /// Specs that were synced
    pub synced_ids: Vec<String>,
    /// Files that were removed
    pub removed_ids: Vec<String>,
}

impl SpecSyncer {
    /// Create a new syncer
    pub fn new(target_dir: PathBuf) -> Self {
        Self { target_dir }
    }

    /// Create a syncer with default settings
    pub fn with_defaults(cas_dir: &Path) -> Self {
        Self {
            target_dir: cas_dir.join("specs"),
        }
    }

    /// Check if a spec should be synced (approved status)
    pub fn is_approved(&self, spec: &Spec) -> bool {
        spec.status == SpecStatus::Approved
    }

    /// Generate the markdown content for a spec
    pub fn generate_spec_md(&self, spec: &Spec) -> String {
        let mut content = String::new();

        // YAML frontmatter
        content.push_str("---\n");
        content.push_str(&format!("id: {}\n", spec.id));
        content.push_str(&format!("title: {}\n", escape_yaml(&spec.title)));
        content.push_str(&format!("spec_type: {}\n", spec.spec_type));
        content.push_str(&format!("status: {}\n", spec.status));
        content.push_str(&format!("version: {}\n", spec.version));

        if let Some(ref task_id) = spec.task_id {
            content.push_str(&format!("task_id: {task_id}\n"));
        }

        if let Some(ref approved_at) = spec.approved_at {
            content.push_str(&format!("approved_at: {}\n", approved_at.to_rfc3339()));
        }

        if let Some(ref approved_by) = spec.approved_by {
            content.push_str(&format!("approved_by: {approved_by}\n"));
        }

        if !spec.tags.is_empty() {
            content.push_str("tags:\n");
            for tag in &spec.tags {
                content.push_str(&format!("  - {}\n", escape_yaml(tag)));
            }
        }

        content.push_str("---\n\n");

        // Title
        content.push_str(&format!("# {}\n\n", spec.title));

        // Summary section
        if !spec.summary.is_empty() {
            content.push_str("## Summary\n\n");
            content.push_str(&spec.summary);
            content.push_str("\n\n");
        }

        // Goals section
        if !spec.goals.is_empty() {
            content.push_str("## Goals\n\n");
            for goal in &spec.goals {
                content.push_str(&format!("- {goal}\n"));
            }
            content.push('\n');
        }

        // Scope section (in-scope and out-of-scope)
        if !spec.in_scope.is_empty() || !spec.out_of_scope.is_empty() {
            content.push_str("## Scope\n\n");

            if !spec.in_scope.is_empty() {
                content.push_str("### In Scope\n\n");
                for item in &spec.in_scope {
                    content.push_str(&format!("- {item}\n"));
                }
                content.push('\n');
            }

            if !spec.out_of_scope.is_empty() {
                content.push_str("### Out of Scope\n\n");
                for item in &spec.out_of_scope {
                    content.push_str(&format!("- {item}\n"));
                }
                content.push('\n');
            }
        }

        // Users section
        if !spec.users.is_empty() {
            content.push_str("## Users\n\n");
            for user in &spec.users {
                content.push_str(&format!("- {user}\n"));
            }
            content.push('\n');
        }

        // Technical Requirements section
        if !spec.technical_requirements.is_empty() {
            content.push_str("## Technical Requirements\n\n");
            for req in &spec.technical_requirements {
                content.push_str(&format!("- {req}\n"));
            }
            content.push('\n');
        }

        // Acceptance Criteria section
        if !spec.acceptance_criteria.is_empty() {
            content.push_str("## Acceptance Criteria\n\n");
            for criteria in &spec.acceptance_criteria {
                content.push_str(&format!("- {criteria}\n"));
            }
            content.push('\n');
        }

        // Design Notes section
        if !spec.design_notes.is_empty() {
            content.push_str("## Design Notes\n\n");
            content.push_str(&spec.design_notes);
            content.push_str("\n\n");
        }

        // Additional Notes section
        if !spec.additional_notes.is_empty() {
            content.push_str("## Additional Notes\n\n");
            content.push_str(&spec.additional_notes);
            content.push('\n');
        }

        content
    }

    /// Sync a single spec to target directory
    ///
    /// Returns true if the spec was synced, false if it wasn't approved
    pub fn sync_spec(&self, spec: &Spec) -> Result<bool, CoreError> {
        let filepath = self.target_dir.join(format!("{}.md", spec.id));

        if !self.is_approved(spec) {
            // If spec exists but is no longer approved, remove it
            if filepath.exists() {
                fs::remove_file(&filepath)?;
            }
            return Ok(false);
        }

        fs::create_dir_all(&self.target_dir)?;

        let content = self.generate_spec_md(spec);
        fs::write(&filepath, content)?;

        Ok(true)
    }

    /// Sync all approved specs and remove stale files
    pub fn sync_all(&self, specs: &[Spec]) -> Result<SpecSyncReport, CoreError> {
        let mut report = SpecSyncReport::default();

        // Collect IDs of approved specs
        let approved_ids: HashSet<_> = specs
            .iter()
            .filter(|s| self.is_approved(s))
            .map(|s| s.id.clone())
            .collect();

        // Sync approved specs
        for spec in specs {
            if self.sync_spec(spec)? {
                report.synced += 1;
                report.synced_ids.push(spec.id.clone());
            }
        }

        // Remove stale files (only spec-* prefixed files that we manage)
        if self.target_dir.exists() {
            for entry in fs::read_dir(&self.target_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default();

                    // Only remove spec-* prefixed files that we manage
                    if stem.starts_with("spec-") && !approved_ids.contains(stem) {
                        fs::remove_file(&path)?;
                        report.removed += 1;
                        report.removed_ids.push(stem.to_string());
                    }
                }
            }
        }

        Ok(report)
    }

    /// Remove a specific spec file
    pub fn remove_spec(&self, spec_id: &str) -> Result<(), CoreError> {
        let filepath = self.target_dir.join(format!("{spec_id}.md"));
        if filepath.exists() {
            fs::remove_file(filepath)?;
        }
        Ok(())
    }

    /// Get the target directory path
    pub fn target_dir(&self) -> &Path {
        &self.target_dir
    }

    /// List all synced spec files
    pub fn list_synced(&self) -> Result<Vec<String>, CoreError> {
        let mut ids = Vec::new();

        if self.target_dir.exists() {
            for entry in fs::read_dir(&self.target_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if stem.starts_with("spec-") {
                            ids.push(stem.to_string());
                        }
                    }
                }
            }
        }

        ids.sort();
        Ok(ids)
    }
}

/// Escape a string for YAML.
///
/// Delegates to the single shared serializer (cas-d731). This copy escaped
/// backslashes while the skill writers' copy did not — the drift that proved
/// hand-rolled escaping had stopped being one policy.
fn escape_yaml(s: &str) -> String {
    crate::sync::yaml_scalar(s)
}

#[cfg(test)]
mod tests {
    use crate::sync::specs::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn create_test_spec(id: &str, title: &str, approved: bool) -> Spec {
        let mut spec = Spec::new(id.to_string(), title.to_string());
        if approved {
            spec.status = SpecStatus::Approved;
            spec.approved_at = Some(Utc::now());
            spec.approved_by = Some("test-user".to_string());
        }
        spec
    }

    #[test]
    fn test_is_approved() {
        let syncer = SpecSyncer::new(PathBuf::from("/tmp/test"));

        let approved = create_test_spec("spec-001", "Approved Spec", true);
        let draft = create_test_spec("spec-002", "Draft Spec", false);

        assert!(syncer.is_approved(&approved));
        assert!(!syncer.is_approved(&draft));
    }

    #[test]
    fn test_sync_spec() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("specs");
        let syncer = SpecSyncer::new(target.clone());

        let spec = create_test_spec("spec-001", "Test Spec", true);

        // Sync the spec
        assert!(syncer.sync_spec(&spec).unwrap());

        // Check file was created
        let filepath = target.join("spec-001.md");
        assert!(filepath.exists());

        let content = fs::read_to_string(&filepath).unwrap();
        assert!(content.contains("id: spec-001"));
        assert!(content.contains("title: Test Spec"));
        assert!(content.contains("status: approved"));
    }

    #[test]
    fn test_sync_spec_with_content() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("specs");
        let syncer = SpecSyncer::new(target.clone());

        let mut spec = create_test_spec("spec-002", "Full Spec", true);
        spec.summary = "This is a test summary.".to_string();
        spec.goals = vec!["Goal 1".to_string(), "Goal 2".to_string()];
        spec.in_scope = vec!["Feature A".to_string()];
        spec.out_of_scope = vec!["Feature B".to_string()];
        spec.users = vec!["Developers".to_string()];
        spec.technical_requirements = vec!["Rust 1.70+".to_string()];
        spec.acceptance_criteria = vec!["Tests pass".to_string()];
        spec.design_notes = "Use builder pattern.".to_string();
        spec.tags = vec!["test".to_string(), "example".to_string()];

        syncer.sync_spec(&spec).unwrap();

        let content = fs::read_to_string(target.join("spec-002.md")).unwrap();
        assert!(content.contains("## Summary"));
        assert!(content.contains("This is a test summary."));
        assert!(content.contains("## Goals"));
        assert!(content.contains("- Goal 1"));
        assert!(content.contains("## Scope"));
        assert!(content.contains("### In Scope"));
        assert!(content.contains("- Feature A"));
        assert!(content.contains("### Out of Scope"));
        assert!(content.contains("- Feature B"));
        assert!(content.contains("## Users"));
        assert!(content.contains("- Developers"));
        assert!(content.contains("## Technical Requirements"));
        assert!(content.contains("- Rust 1.70+"));
        assert!(content.contains("## Acceptance Criteria"));
        assert!(content.contains("- Tests pass"));
        assert!(content.contains("## Design Notes"));
        assert!(content.contains("Use builder pattern."));
        assert!(content.contains("tags:"));
        assert!(content.contains("  - test"));
    }

    #[test]
    fn test_sync_all() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("specs");
        let syncer = SpecSyncer::new(target.clone());

        let spec1 = create_test_spec("spec-001", "Approved Spec", true);
        let spec2 = create_test_spec("spec-002", "Draft Spec", false);

        let specs = vec![spec1, spec2];
        let report = syncer.sync_all(&specs).unwrap();

        assert_eq!(report.synced, 1);
        assert!(target.join("spec-001.md").exists());
        assert!(!target.join("spec-002.md").exists());
    }

    #[test]
    fn test_remove_stale() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("specs");
        let syncer = SpecSyncer::new(target.clone());

        // Create a stale spec file
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("spec-stale.md"), "stale content").unwrap();

        // Sync with no specs
        let report = syncer.sync_all(&[]).unwrap();

        assert_eq!(report.removed, 1);
        assert!(!target.join("spec-stale.md").exists());
    }

    #[test]
    fn test_remove_spec() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("specs");
        let syncer = SpecSyncer::new(target.clone());

        // Create a spec file
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("spec-001.md"), "content").unwrap();

        // Remove it
        syncer.remove_spec("spec-001").unwrap();
        assert!(!target.join("spec-001.md").exists());
    }

    #[test]
    fn test_list_synced() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("specs");
        let syncer = SpecSyncer::new(target.clone());

        // Create some spec files
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("spec-001.md"), "content").unwrap();
        fs::write(target.join("spec-002.md"), "content").unwrap();
        fs::write(target.join("other.md"), "content").unwrap(); // Should be ignored

        let synced = syncer.list_synced().unwrap();
        assert_eq!(synced, vec!["spec-001", "spec-002"]);
    }

    #[test]
    fn test_with_defaults() {
        let temp = TempDir::new().unwrap();
        let syncer = SpecSyncer::with_defaults(temp.path());
        assert_eq!(syncer.target_dir(), temp.path().join("specs"));
    }

    /// cas-d731. This used to assert one hand-rolled spelling per input, which
    /// is how the three escapers drifted without anyone noticing: the
    /// assertions pinned the shape of the escape rather than the property that
    /// matters. Now the contract is stated directly — quoting is whatever the
    /// shared emitter chooses, provided a real parser reads the value back
    /// unchanged on a single line.
    #[test]
    fn test_escape_yaml_round_trips_through_a_real_parser() {
        for value in [
            "simple",
            "with: colon",
            "with # hash",
            "with\nnewline",
            " leading space",
            "with \"quotes\"",
            "with 'apostrophes'",
            "C:\\spec\\path: inspect",
            "true",
        ] {
            let scalar = escape_yaml(value);
            assert!(!scalar.contains('\n'), "{value:?} -> {scalar:?}");

            let document = format!("title: {scalar}\n");
            let parsed: serde_yaml::Value = serde_yaml::from_str(&document)
                .unwrap_or_else(|e| panic!("invalid YAML for {value:?}: {e}\n{document}"));
            assert_eq!(
                parsed.get("title").and_then(|v| v.as_str()),
                Some(value),
                "value changed on the way through YAML: {document}"
            );
        }

        // Ordinary text still reads plainly in the generated file.
        assert_eq!(escape_yaml("simple"), "simple");
    }

    #[test]
    fn test_generate_spec_md_frontmatter() {
        let syncer = SpecSyncer::new(PathBuf::from("/tmp/test"));

        let mut spec = create_test_spec("spec-001", "Test Spec", true);
        spec.task_id = Some("task-123".to_string());
        spec.version = 2;

        let content = syncer.generate_spec_md(&spec);

        assert!(content.starts_with("---\n"));
        assert!(content.contains("id: spec-001\n"));
        assert!(content.contains("title: Test Spec\n"));
        assert!(content.contains("spec_type: epic\n"));
        assert!(content.contains("status: approved\n"));
        assert!(content.contains("version: 2\n"));
        assert!(content.contains("task_id: task-123\n"));
        assert!(content.contains("approved_at:"));
        assert!(content.contains("approved_by: test-user\n"));
    }

    #[test]
    fn test_draft_spec_not_synced() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("specs");
        let syncer = SpecSyncer::new(target.clone());

        let draft = create_test_spec("spec-001", "Draft Spec", false);

        // Should not sync
        assert!(!syncer.sync_spec(&draft).unwrap());
        assert!(!target.join("spec-001.md").exists());
    }

    #[test]
    fn test_unapproved_spec_removed() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("specs");
        let syncer = SpecSyncer::new(target.clone());

        // First sync an approved spec
        let mut spec = create_test_spec("spec-001", "Test Spec", true);
        syncer.sync_spec(&spec).unwrap();
        assert!(target.join("spec-001.md").exists());

        // Now unapprove it and sync again
        spec.status = SpecStatus::Draft;
        syncer.sync_spec(&spec).unwrap();
        assert!(!target.join("spec-001.md").exists());
    }
}
