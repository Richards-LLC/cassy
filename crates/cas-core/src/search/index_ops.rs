use crate::error::CoreError;
use crate::search::{ArtifactDocument, DocType, SearchIndex, artifact_document_id};
use cas_types::{Entry, Rule, Skill, Spec, Task};

impl SearchIndex {
    /// Index a single entry
    pub fn index_entry(&self, entry: &Entry) -> Result<(), CoreError> {
        let mut writer = self.writer()?;

        // Delete existing document with same ID
        let id_term = tantivy::Term::from_field_text(self.id_field, &entry.id);
        writer.delete_term(id_term);

        // Add new document
        let mut doc = tantivy::TantivyDocument::new();
        doc.add_text(self.id_field, &entry.id);
        doc.add_text(self.content_field, &entry.content);
        doc.add_text(self.tags_field, entry.tags.join(" "));
        doc.add_text(self.type_field, entry.entry_type.to_string());
        doc.add_text(self.doc_type_field, DocType::Entry.as_str());
        if let Some(ref title) = entry.title {
            doc.add_text(self.title_field, title);
        }

        writer
            .add_document(doc)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;

        Ok(())
    }

    /// Index a single task
    pub fn index_task(&self, task: &Task) -> Result<(), CoreError> {
        let mut writer = self.writer()?;

        // Delete existing document with same ID
        let id_term = tantivy::Term::from_field_text(self.id_field, &task.id);
        writer.delete_term(id_term);

        // Build content from title + description + acceptance criteria
        let mut content = task.title.clone();
        if !task.description.is_empty() {
            content.push(' ');
            content.push_str(&task.description);
        }
        if !task.acceptance_criteria.is_empty() {
            content.push(' ');
            content.push_str(&task.acceptance_criteria);
        }

        // Add new document
        let mut doc = tantivy::TantivyDocument::new();
        doc.add_text(self.id_field, &task.id);
        doc.add_text(self.content_field, &content);
        doc.add_text(self.tags_field, ""); // Tasks don't have tags
        doc.add_text(self.type_field, task.status.to_string());
        doc.add_text(self.doc_type_field, DocType::Task.as_str());
        doc.add_text(self.title_field, &task.title);

        writer
            .add_document(doc)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;

        Ok(())
    }

    /// Index durable task artifacts with a single commit for a close-time scan
    /// or a full backfill.
    pub fn index_artifacts(&self, artifacts: &[ArtifactDocument]) -> Result<usize, CoreError> {
        if artifacts.is_empty() {
            return Ok(0);
        }

        let mut writer = self.writer()?;
        for artifact in artifacts {
            let id = artifact_document_id(&artifact.task_id, &artifact.path);
            writer.delete_term(tantivy::Term::from_field_text(self.id_field, &id));

            let mut doc = tantivy::TantivyDocument::new();
            doc.add_text(self.id_field, &id);
            doc.add_text(
                self.content_field,
                format!(
                    "{} {} {} {}",
                    artifact.task_id,
                    artifact.path,
                    std::path::Path::new(&artifact.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                    artifact.content
                ),
            );
            doc.add_text(self.tags_field, "");
            doc.add_text(self.type_field, &artifact.task_id);
            doc.add_text(self.doc_type_field, DocType::Artifact.as_str());
            doc.add_text(self.title_field, &artifact.path);
            writer
                .add_document(doc)
                .map_err(|e| CoreError::Other(e.to_string()))?;
        }
        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(artifacts.len())
    }

    /// Index a single rule
    pub fn index_rule(&self, rule: &Rule) -> Result<(), CoreError> {
        let mut writer = self.writer()?;

        // Delete existing document with same ID
        let id_term = tantivy::Term::from_field_text(self.id_field, &rule.id);
        writer.delete_term(id_term);

        // Add new document
        let mut doc = tantivy::TantivyDocument::new();
        doc.add_text(self.id_field, &rule.id);
        doc.add_text(self.content_field, &rule.content);
        doc.add_text(self.tags_field, ""); // Rules don't have tags
        doc.add_text(self.type_field, rule.status.to_string());
        doc.add_text(self.doc_type_field, DocType::Rule.as_str());
        doc.add_text(self.title_field, ""); // Rules don't have separate titles

        writer
            .add_document(doc)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;

        Ok(())
    }

    /// Index a single skill
    pub fn index_skill(&self, skill: &Skill) -> Result<(), CoreError> {
        let mut writer = self.writer()?;

        // Delete existing document with same ID
        let id_term = tantivy::Term::from_field_text(self.id_field, &skill.id);
        writer.delete_term(id_term);

        // Build content from name + description + invocation + example
        let mut content = skill.name.clone();
        if !skill.description.is_empty() {
            content.push(' ');
            content.push_str(&skill.description);
        }
        if !skill.invocation.is_empty() {
            content.push(' ');
            content.push_str(&skill.invocation);
        }
        if !skill.example.is_empty() {
            content.push(' ');
            content.push_str(&skill.example);
        }

        // Add new document
        let mut doc = tantivy::TantivyDocument::new();
        doc.add_text(self.id_field, &skill.id);
        doc.add_text(self.content_field, &content);
        doc.add_text(self.tags_field, skill.tags.join(" "));
        doc.add_text(self.type_field, skill.status.to_string());
        doc.add_text(self.doc_type_field, DocType::Skill.as_str());
        doc.add_text(self.title_field, &skill.name);

        writer
            .add_document(doc)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;

        Ok(())
    }

    /// Index a single spec
    pub fn index_spec(&self, spec: &Spec) -> Result<(), CoreError> {
        let mut writer = self.writer()?;

        // Delete existing document with same ID
        let id_term = tantivy::Term::from_field_text(self.id_field, &spec.id);
        writer.delete_term(id_term);

        // Build content with weighted fields:
        // - title (high weight): repeated for emphasis
        // - summary (high weight): repeated for emphasis
        // - goals (medium weight): joined
        // - acceptance_criteria (medium weight): joined
        // - design_notes (medium weight): included once
        // - tags (low weight): joined
        let mut content = String::new();

        // High weight: title (repeat for emphasis)
        content.push_str(&spec.title);
        content.push(' ');
        content.push_str(&spec.title);
        content.push(' ');

        // High weight: summary (repeat for emphasis)
        if !spec.summary.is_empty() {
            content.push_str(&spec.summary);
            content.push(' ');
            content.push_str(&spec.summary);
            content.push(' ');
        }

        // Medium weight: goals
        if !spec.goals.is_empty() {
            content.push_str(&spec.goals.join(" "));
            content.push(' ');
        }

        // Medium weight: acceptance_criteria
        if !spec.acceptance_criteria.is_empty() {
            content.push_str(&spec.acceptance_criteria.join(" "));
            content.push(' ');
        }

        // Medium weight: design_notes
        if !spec.design_notes.is_empty() {
            content.push_str(&spec.design_notes);
            content.push(' ');
        }

        // Add new document
        let mut doc = tantivy::TantivyDocument::new();
        doc.add_text(self.id_field, &spec.id);
        doc.add_text(self.content_field, &content);
        doc.add_text(self.tags_field, spec.tags.join(" ")); // Low weight: tags
        doc.add_text(self.type_field, spec.status.to_string());
        doc.add_text(self.doc_type_field, DocType::Spec.as_str());
        doc.add_text(self.title_field, &spec.title);

        writer
            .add_document(doc)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;

        Ok(())
    }

    /// Delete an entry from the index
    pub fn delete(&self, id: &str) -> Result<(), CoreError> {
        let mut writer = self.writer()?;
        let id_term = tantivy::Term::from_field_text(self.id_field, id);
        writer.delete_term(id_term);
        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    /// Reindex all entries (legacy method - use reindex_all for unified search)
    pub fn reindex(&self, entries: &[Entry]) -> Result<(), CoreError> {
        let mut writer = self.writer()?;

        // Clear the index
        writer
            .delete_all_documents()
            .map_err(|e| CoreError::Other(e.to_string()))?;

        // Add all entries
        for entry in entries {
            if entry.archived {
                continue;
            }

            let mut doc = tantivy::TantivyDocument::new();
            doc.add_text(self.id_field, &entry.id);
            doc.add_text(self.content_field, &entry.content);
            doc.add_text(self.tags_field, entry.tags.join(" "));
            doc.add_text(self.type_field, entry.entry_type.to_string());
            doc.add_text(self.doc_type_field, DocType::Entry.as_str());
            if let Some(ref title) = entry.title {
                doc.add_text(self.title_field, title);
            }

            writer
                .add_document(doc)
                .map_err(|e| CoreError::Other(e.to_string()))?;
        }

        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    /// Reindex all entity types (entries, tasks, rules, skills, specs)
    pub fn reindex_all(
        &self,
        entries: &[Entry],
        tasks: &[Task],
        rules: &[Rule],
        skills: &[Skill],
        specs: &[Spec],
    ) -> Result<usize, CoreError> {
        let mut writer = self.writer()?;
        let mut count = 0;

        // Clear the index
        writer
            .delete_all_documents()
            .map_err(|e| CoreError::Other(e.to_string()))?;

        // Add entries
        for entry in entries {
            if entry.archived {
                continue;
            }

            let mut doc = tantivy::TantivyDocument::new();
            doc.add_text(self.id_field, &entry.id);
            doc.add_text(self.content_field, &entry.content);
            doc.add_text(self.tags_field, entry.tags.join(" "));
            doc.add_text(self.type_field, entry.entry_type.to_string());
            doc.add_text(self.doc_type_field, DocType::Entry.as_str());
            if let Some(ref title) = entry.title {
                doc.add_text(self.title_field, title);
            }

            writer
                .add_document(doc)
                .map_err(|e| CoreError::Other(e.to_string()))?;
            count += 1;
        }

        // Add tasks
        for task in tasks {
            let mut content = task.title.clone();
            if !task.description.is_empty() {
                content.push(' ');
                content.push_str(&task.description);
            }
            if !task.acceptance_criteria.is_empty() {
                content.push(' ');
                content.push_str(&task.acceptance_criteria);
            }

            let mut doc = tantivy::TantivyDocument::new();
            doc.add_text(self.id_field, &task.id);
            doc.add_text(self.content_field, &content);
            doc.add_text(self.tags_field, ""); // Tasks don't have tags
            doc.add_text(self.type_field, task.status.to_string());
            doc.add_text(self.doc_type_field, DocType::Task.as_str());
            doc.add_text(self.title_field, &task.title);

            writer
                .add_document(doc)
                .map_err(|e| CoreError::Other(e.to_string()))?;
            count += 1;
        }

        // Add rules
        for rule in rules {
            let mut doc = tantivy::TantivyDocument::new();
            doc.add_text(self.id_field, &rule.id);
            doc.add_text(self.content_field, &rule.content);
            doc.add_text(self.tags_field, "");
            doc.add_text(self.type_field, rule.status.to_string());
            doc.add_text(self.doc_type_field, DocType::Rule.as_str());
            doc.add_text(self.title_field, "");

            writer
                .add_document(doc)
                .map_err(|e| CoreError::Other(e.to_string()))?;
            count += 1;
        }

        // Add skills
        for skill in skills {
            let mut content = skill.name.clone();
            if !skill.description.is_empty() {
                content.push(' ');
                content.push_str(&skill.description);
            }
            if !skill.invocation.is_empty() {
                content.push(' ');
                content.push_str(&skill.invocation);
            }

            let mut doc = tantivy::TantivyDocument::new();
            doc.add_text(self.id_field, &skill.id);
            doc.add_text(self.content_field, &content);
            doc.add_text(self.tags_field, skill.tags.join(" "));
            doc.add_text(self.type_field, skill.status.to_string());
            doc.add_text(self.doc_type_field, DocType::Skill.as_str());
            doc.add_text(self.title_field, &skill.name);

            writer
                .add_document(doc)
                .map_err(|e| CoreError::Other(e.to_string()))?;
            count += 1;
        }

        // Add specs
        for spec in specs {
            // Build content with weighted fields
            let mut content = String::new();

            // High weight: title (repeat for emphasis)
            content.push_str(&spec.title);
            content.push(' ');
            content.push_str(&spec.title);
            content.push(' ');

            // High weight: summary (repeat for emphasis)
            if !spec.summary.is_empty() {
                content.push_str(&spec.summary);
                content.push(' ');
                content.push_str(&spec.summary);
                content.push(' ');
            }

            // Medium weight: goals
            if !spec.goals.is_empty() {
                content.push_str(&spec.goals.join(" "));
                content.push(' ');
            }

            // Medium weight: acceptance_criteria
            if !spec.acceptance_criteria.is_empty() {
                content.push_str(&spec.acceptance_criteria.join(" "));
                content.push(' ');
            }

            // Medium weight: design_notes
            if !spec.design_notes.is_empty() {
                content.push_str(&spec.design_notes);
                content.push(' ');
            }

            let mut doc = tantivy::TantivyDocument::new();
            doc.add_text(self.id_field, &spec.id);
            doc.add_text(self.content_field, &content);
            doc.add_text(self.tags_field, spec.tags.join(" ")); // Low weight: tags
            doc.add_text(self.type_field, spec.status.to_string());
            doc.add_text(self.doc_type_field, DocType::Spec.as_str());
            doc.add_text(self.title_field, &spec.title);

            writer
                .add_document(doc)
                .map_err(|e| CoreError::Other(e.to_string()))?;
            count += 1;
        }

        writer
            .commit()
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(count)
    }
}
