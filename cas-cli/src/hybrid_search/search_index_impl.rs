use std::path::Path;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter};

use cas_code::CodeSymbol;
use cas_core::search::{ArtifactDocument, artifact_document_id};

use crate::error::MemError;
use crate::hybrid_search::frontmatter::{FrontmatterFields, extract_frontmatter_fields};
use crate::hybrid_search::{DEFAULT_WRITER_MEMORY, DocType, EXPECTED_FIELD_COUNT, SearchIndex};
use crate::types::{Entry, Rule, Skill, Spec, Task};

/// Add memory frontmatter fields to a Tantivy document. Safe to call for
/// non-memory docs — it will simply no-op when no frontmatter is present.
fn add_frontmatter_terms(
    doc: &mut tantivy::TantivyDocument,
    fields: &FrontmatterFields,
    module_field: tantivy::schema::Field,
    track_field: tantivy::schema::Field,
    problem_type_field: tantivy::schema::Field,
    severity_field: tantivy::schema::Field,
    root_cause_field: tantivy::schema::Field,
    mem_date_field: tantivy::schema::Field,
) {
    if let Some(ref v) = fields.module {
        doc.add_text(module_field, v);
    }
    if let Some(ref v) = fields.track {
        doc.add_text(track_field, v);
    }
    if let Some(ref v) = fields.problem_type {
        doc.add_text(problem_type_field, v);
    }
    if let Some(ref v) = fields.severity {
        doc.add_text(severity_field, v);
    }
    if let Some(ref v) = fields.root_cause {
        doc.add_text(root_cause_field, v);
    }
    if let Some(ref v) = fields.date {
        doc.add_text(mem_date_field, v);
    }
}
impl SearchIndex {
    /// Build the current schema
    fn build_schema() -> Schema {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("id", STRING | STORED);
        schema_builder.add_text_field("content", TEXT);
        schema_builder.add_text_field("tags", TEXT);
        schema_builder.add_text_field("type", STRING);
        schema_builder.add_text_field("title", TEXT);
        schema_builder.add_text_field("doc_type", STRING | STORED);
        // Code-specific fields (added in schema v2)
        schema_builder.add_text_field("language", STRING);
        schema_builder.add_text_field("kind", STRING);
        schema_builder.add_text_field("file_path", STRING | STORED);
        // Memory frontmatter fields (cas-7b1e). All STRING | STORED so they
        // are term-filterable and retrievable for debugging. Legacy memories
        // without these fields simply index nothing for these columns.
        schema_builder.add_text_field("module", STRING | STORED);
        schema_builder.add_text_field("track", STRING | STORED);
        schema_builder.add_text_field("problem_type", STRING | STORED);
        schema_builder.add_text_field("severity", STRING | STORED);
        schema_builder.add_text_field("root_cause", STRING | STORED);
        schema_builder.add_text_field("mem_date", STRING | STORED);
        schema_builder.build()
    }

    /// Open or create a search index
    pub fn open(index_dir: &Path) -> Result<Self, MemError> {
        prepare_versioned_index_dir(index_dir)?;
        let schema = Self::build_schema();

        let index = if index_dir.exists() && index_dir.join("meta.json").exists() {
            // Open existing index and check for schema mismatch
            let existing_index = Index::open_in_dir(index_dir)?;
            let existing_field_count = existing_index.schema().fields().count();

            if existing_field_count != EXPECTED_FIELD_COUNT {
                // Never remove a directory merely because its schema is from
                // another release. A still-running binary may be using the
                // same path, and a caller can explicitly rebuild the
                // versioned path through `rebuild` when it owns that action.
                drop(existing_index);
                return Err(MemError::Other(format!(
                    "schema mismatch at {}: existing index has {existing_field_count} fields, expected {EXPECTED_FIELD_COUNT}; run the reindex maintenance action (`mcp__cas__system action=reindex bm25=true`) from an agent session",
                    index_dir.display()
                )));
            } else {
                existing_index
            }
        } else {
            std::fs::create_dir_all(index_dir)?;
            Index::create_in_dir(index_dir, schema.clone())?
        };

        // Get field handles from the index's schema (whether existing or new)
        let schema = index.schema();
        let id_field = schema.get_field("id").expect("id field missing");
        let content_field = schema.get_field("content").expect("content field missing");
        let tags_field = schema.get_field("tags").expect("tags field missing");
        let type_field = schema.get_field("type").expect("type field missing");
        let title_field = schema.get_field("title").expect("title field missing");
        let doc_type_field = schema
            .get_field("doc_type")
            .expect("doc_type field missing");
        let language_field = schema
            .get_field("language")
            .expect("language field missing");
        let kind_field = schema.get_field("kind").expect("kind field missing");
        let file_path_field = schema
            .get_field("file_path")
            .expect("file_path field missing");
        let module_field = schema.get_field("module").expect("module field missing");
        let track_field = schema.get_field("track").expect("track field missing");
        let problem_type_field = schema
            .get_field("problem_type")
            .expect("problem_type field missing");
        let severity_field = schema
            .get_field("severity")
            .expect("severity field missing");
        let root_cause_field = schema
            .get_field("root_cause")
            .expect("root_cause field missing");
        let mem_date_field = schema
            .get_field("mem_date")
            .expect("mem_date field missing");

        Ok(Self {
            index,
            schema: schema.clone(),
            id_field,
            content_field,
            tags_field,
            type_field,
            title_field,
            doc_type_field,
            language_field,
            kind_field,
            file_path_field,
            module_field,
            track_field,
            problem_type_field,
            severity_field,
            root_cause_field,
            mem_date_field,
            writer_memory: DEFAULT_WRITER_MEMORY,
            cached_reader: std::sync::Mutex::new(None),
            cached_query_parser: std::sync::Mutex::new(None),
        })
    }

    /// Explicitly rebuild a versioned index directory.
    ///
    /// Automatic opens are read/create-only with respect to an existing
    /// index. Rebuilding is reserved for an explicit reindex request, and is
    /// confined to the caller's already-resolved schema-versioned path.
    pub fn rebuild(index_dir: &Path) -> Result<Self, MemError> {
        if index_dir.exists() {
            std::fs::remove_dir_all(index_dir)?;
        }
        let index = Self::open(index_dir)?;
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
        let mut writer = index.writer()?;
        writer.delete_all_documents()?;
        writer.commit()?;
        Ok(index)
    }

    /// Open or create a search index with custom writer memory budget
    pub fn open_with_memory(index_dir: &Path, writer_memory: usize) -> Result<Self, MemError> {
        let mut index = Self::open(index_dir)?;
        index.writer_memory = writer_memory;
        Ok(index)
    }

    /// Create an in-memory search index (for testing)
    pub fn in_memory() -> Result<Self, MemError> {
        let schema = Self::build_schema();
        let index = Index::create_in_ram(schema.clone());

        // Get field handles from schema
        let id_field = schema.get_field("id").expect("id field missing");
        let content_field = schema.get_field("content").expect("content field missing");
        let tags_field = schema.get_field("tags").expect("tags field missing");
        let type_field = schema.get_field("type").expect("type field missing");
        let title_field = schema.get_field("title").expect("title field missing");
        let doc_type_field = schema
            .get_field("doc_type")
            .expect("doc_type field missing");
        let language_field = schema
            .get_field("language")
            .expect("language field missing");
        let kind_field = schema.get_field("kind").expect("kind field missing");
        let file_path_field = schema
            .get_field("file_path")
            .expect("file_path field missing");
        let module_field = schema.get_field("module").expect("module field missing");
        let track_field = schema.get_field("track").expect("track field missing");
        let problem_type_field = schema
            .get_field("problem_type")
            .expect("problem_type field missing");
        let severity_field = schema
            .get_field("severity")
            .expect("severity field missing");
        let root_cause_field = schema
            .get_field("root_cause")
            .expect("root_cause field missing");
        let mem_date_field = schema
            .get_field("mem_date")
            .expect("mem_date field missing");

        Ok(Self {
            index,
            schema,
            id_field,
            content_field,
            tags_field,
            type_field,
            title_field,
            doc_type_field,
            language_field,
            kind_field,
            file_path_field,
            module_field,
            track_field,
            problem_type_field,
            severity_field,
            root_cause_field,
            mem_date_field,
            writer_memory: DEFAULT_WRITER_MEMORY,
            cached_reader: std::sync::Mutex::new(None),
            cached_query_parser: std::sync::Mutex::new(None),
        })
    }

    /// Get an index writer with configurable memory budget
    fn writer(&self) -> Result<IndexWriter, MemError> {
        Ok(self.index.writer(self.writer_memory)?)
    }

    /// Get the names of all indexed fields from the schema
    pub fn field_names(&self) -> Vec<&str> {
        self.schema
            .fields()
            .map(|(_, entry)| entry.name())
            .collect()
    }

    /// Get the number of fields in the schema
    pub fn field_count(&self) -> usize {
        self.schema.fields().count()
    }

    /// Index a single entry
    pub fn index_entry(&self, entry: &Entry) -> Result<(), MemError> {
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
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
        let fm = extract_frontmatter_fields(&entry.content);
        add_frontmatter_terms(
            &mut doc,
            &fm,
            self.module_field,
            self.track_field,
            self.problem_type_field,
            self.severity_field,
            self.root_cause_field,
            self.mem_date_field,
        );

        writer.add_document(doc)?;
        writer.commit()?;

        Ok(())
    }

    /// Index multiple entries efficiently with a single commit
    ///
    /// This is 10-100x faster than calling index_entry() for each entry
    /// because it uses a single commit for all documents.
    pub fn index_entries_batch(&self, entries: &[Entry]) -> Result<usize, MemError> {
        self.index_entries_batch_inner(entries, false)
    }

    /// Index a background batch and wait for Tantivy's merge policy to finish.
    ///
    /// A short-lived daemon writer otherwise drops immediately after commit,
    /// before its asynchronous merge can collapse the accumulated segments.
    pub fn index_entries_batch_and_merge(&self, entries: &[Entry]) -> Result<usize, MemError> {
        self.index_entries_batch_inner(entries, true)
    }

    fn index_entries_batch_inner(
        &self,
        entries: &[Entry],
        wait_for_merge: bool,
    ) -> Result<usize, MemError> {
        if entries.is_empty() {
            return Ok(0);
        }

        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
        let mut writer = self.writer()?;
        let mut count = 0;

        for entry in entries {
            if entry.archived {
                continue;
            }

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
            let fm = extract_frontmatter_fields(&entry.content);
            add_frontmatter_terms(
                &mut doc,
                &fm,
                self.module_field,
                self.track_field,
                self.problem_type_field,
                self.severity_field,
                self.root_cause_field,
                self.mem_date_field,
            );

            writer.add_document(doc)?;
            count += 1;
        }

        writer.commit()?;
        if wait_for_merge {
            writer.wait_merging_threads()?;
        }
        Ok(count)
    }

    /// Number of currently searchable Tantivy segments.
    pub fn segment_count(&self) -> Result<usize, MemError> {
        Ok(self.index.searchable_segment_ids()?.len())
    }

    /// Index a single task
    pub fn index_task(&self, task: &Task) -> Result<(), MemError> {
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
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

        writer.add_document(doc)?;
        writer.commit()?;

        Ok(())
    }

    /// Index durable task artifacts with one commit for a close-time scan or
    /// a full backfill. Artifact content is already bounded and text-validated
    /// by the discovery layer.
    pub fn index_artifacts(&self, artifacts: &[ArtifactDocument]) -> Result<usize, MemError> {
        if artifacts.is_empty() {
            return Ok(0);
        }

        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
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
            writer.add_document(doc)?;
        }
        writer.commit()?;
        Ok(artifacts.len())
    }

    /// Index a single rule
    pub fn index_rule(&self, rule: &Rule) -> Result<(), MemError> {
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
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

        writer.add_document(doc)?;
        writer.commit()?;

        Ok(())
    }

    /// Index a single skill
    pub fn index_skill(&self, skill: &Skill) -> Result<(), MemError> {
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
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

        writer.add_document(doc)?;
        writer.commit()?;

        Ok(())
    }

    /// Delete an entry from the index
    pub fn delete(&self, id: &str) -> Result<(), MemError> {
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
        let mut writer = self.writer()?;
        let id_term = tantivy::Term::from_field_text(self.id_field, id);
        writer.delete_term(id_term);
        writer.commit()?;
        Ok(())
    }

    /// Reindex all entries (legacy method - use reindex_all for unified search)
    pub fn reindex(&self, entries: &[Entry]) -> Result<(), MemError> {
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
        let mut writer = self.writer()?;

        // Clear the index
        writer.delete_all_documents()?;

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
            let fm = extract_frontmatter_fields(&entry.content);
            add_frontmatter_terms(
                &mut doc,
                &fm,
                self.module_field,
                self.track_field,
                self.problem_type_field,
                self.severity_field,
                self.root_cause_field,
                self.mem_date_field,
            );

            writer.add_document(doc)?;
        }

        writer.commit()?;
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
    ) -> Result<usize, MemError> {
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
        let mut writer = self.writer()?;
        let mut count = 0;

        // Clear the index
        writer.delete_all_documents()?;

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
            let fm = extract_frontmatter_fields(&entry.content);
            add_frontmatter_terms(
                &mut doc,
                &fm,
                self.module_field,
                self.track_field,
                self.problem_type_field,
                self.severity_field,
                self.root_cause_field,
                self.mem_date_field,
            );

            writer.add_document(doc)?;
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

            writer.add_document(doc)?;
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

            writer.add_document(doc)?;
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

            writer.add_document(doc)?;
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

            writer.add_document(doc)?;
            count += 1;
        }

        writer.commit()?;
        Ok(count)
    }

    /// Reindex all entity types including code symbols
    pub fn reindex_all_with_code(
        &self,
        entries: &[Entry],
        tasks: &[Task],
        rules: &[Rule],
        skills: &[Skill],
        symbols: &[CodeSymbol],
    ) -> Result<usize, MemError> {
        #[cfg(test)]
        let _disk_index_test_lock = crate::test_support::disk_index_test_lock();
        let mut writer = self.writer()?;
        let mut count = 0;

        // Clear the index
        writer.delete_all_documents()?;

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
            let fm = extract_frontmatter_fields(&entry.content);
            add_frontmatter_terms(
                &mut doc,
                &fm,
                self.module_field,
                self.track_field,
                self.problem_type_field,
                self.severity_field,
                self.root_cause_field,
                self.mem_date_field,
            );

            writer.add_document(doc)?;
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
            doc.add_text(self.tags_field, "");
            doc.add_text(self.type_field, task.status.to_string());
            doc.add_text(self.doc_type_field, DocType::Task.as_str());
            doc.add_text(self.title_field, &task.title);

            writer.add_document(doc)?;
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

            writer.add_document(doc)?;
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

            writer.add_document(doc)?;
            count += 1;
        }

        // Add code symbols
        for symbol in symbols {
            let mut content = symbol.qualified_name.clone();
            content.push(' ');
            content.push_str(&symbol.source);
            if let Some(ref documentation) = symbol.documentation {
                content.push(' ');
                content.push_str(documentation);
            }

            let mut doc = tantivy::TantivyDocument::new();
            doc.add_text(self.id_field, &symbol.id);
            doc.add_text(self.content_field, &content);
            doc.add_text(self.tags_field, "");
            doc.add_text(self.type_field, symbol.kind.to_string());
            doc.add_text(self.doc_type_field, DocType::CodeSymbol.as_str());
            doc.add_text(self.title_field, &symbol.qualified_name);
            doc.add_text(self.language_field, symbol.language.to_string());
            doc.add_text(self.kind_field, symbol.kind.to_string());
            doc.add_text(self.file_path_field, &symbol.file_path);

            writer.add_document(doc)?;
            count += 1;
        }

        writer.commit()?;
        Ok(count)
    }
}

/// Move the pre-versioned canonical index out of the way once, before opening
/// the current schema path. Same-schema indexes are retained by an atomic
/// rename. Mismatched indexes are quarantined instead of deleted and leave a
/// marker for the background indexer to requeue entry rows for a rebuild.
fn prepare_versioned_index_dir(index_dir: &Path) -> Result<(), MemError> {
    let Some(index_name) = index_dir.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let expected_name = format!("tantivy-v{EXPECTED_FIELD_COUNT}");
    if index_name != expected_name || index_dir.exists() {
        return Ok(());
    }

    let Some(index_parent) = index_dir.parent() else {
        return Ok(());
    };
    let legacy_dir = crate::hybrid_search::legacy_tantivy_index_dir(
        index_parent.parent().unwrap_or(index_parent),
    );
    if !legacy_dir.join("meta.json").is_file() {
        return Ok(());
    }

    let existing_index = Index::open_in_dir(&legacy_dir)?;
    let existing_field_count = existing_index.schema().fields().count();
    drop(existing_index);

    std::fs::create_dir_all(index_parent)?;
    if existing_field_count == EXPECTED_FIELD_COUNT {
        match std::fs::rename(&legacy_dir, index_dir) {
            Ok(()) => {
                tracing::info!(
                    from = %legacy_dir.display(),
                    to = %index_dir.display(),
                    "migrated Tantivy index to its schema-versioned path"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    }

    let marker = crate::hybrid_search::tantivy_rebuild_marker_for_index(index_dir);
    std::fs::write(
        &marker,
        format!(
            "legacy Tantivy index has {existing_field_count} fields; rebuild required for {EXPECTED_FIELD_COUNT}\n"
        ),
    )?;

    // The suffix is deterministic for the normal one-time migration, while
    // the loop preserves an old process that recreated `index/tantivy` after
    // an earlier migration.
    let mut quarantine = index_parent.join(format!(
        "tantivy-legacy-v{existing_field_count}"
    ));
    let mut ordinal = 2;
    while quarantine.exists() {
        quarantine = index_parent.join(format!(
            "tantivy-legacy-v{existing_field_count}-{ordinal}"
        ));
        ordinal += 1;
    }
    match std::fs::rename(&legacy_dir, &quarantine) {
        Ok(()) => {
            tracing::warn!(
                from = %legacy_dir.display(),
                to = %quarantine.display(),
                existing_field_count,
                expected_field_count = EXPECTED_FIELD_COUNT,
                "quarantined a mismatched Tantivy index for one-time rebuild"
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
