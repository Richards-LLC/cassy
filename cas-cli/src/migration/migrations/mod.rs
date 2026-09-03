//! Individual migration definitions
//!
//! Each migration is in its own file for easier management and git history.
//! Files are named: m{ID:03}_{name}.rs

use crate::migration::Migration;

// Entries subsystem (1-50)
mod m001_entries_add_session_id;
mod m002_entries_add_source_tool;
mod m003_entries_add_pending_extraction;
mod m004_entries_add_observation_type;
mod m005_entries_add_stability;
mod m006_entries_add_access_count;
mod m007_entries_add_raw_content;
mod m008_entries_add_compressed;
mod m009_entries_add_memory_tier;
mod m010_entries_add_importance;
mod m011_entries_add_valid_from;
mod m012_entries_add_valid_until;
mod m013_entries_add_review_after;
mod m014_entries_add_pending_embedding;
mod m015_entries_add_belief_type;
mod m016_entries_add_confidence;
mod m017_entries_add_domain;
mod m018_entries_idx_session;
mod m019_entries_idx_pending;
mod m020_entries_idx_obs_type;
mod m021_entries_idx_stability;
mod m022_entries_idx_memory_tier;
mod m023_entries_idx_importance;
mod m024_entries_idx_pending_embedding;
mod m025_entries_idx_belief_type;
mod m026_entries_idx_confidence;
mod m027_entries_idx_domain;
mod m028_sessions_add_title;
mod m029_entries_add_branch;
mod m030_entries_idx_branch;
mod m031_sessions_add_branch;
mod m032_sessions_add_worktree_id;
mod m033_entries_add_scope;
mod m034_entries_add_team_id;
mod m038_entries_add_last_reviewed;
mod m039_entries_add_updated_at;
mod m040_entries_add_indexed_at;
mod m041_entries_idx_pending_index;
mod m042_sessions_add_outcome;
mod m043_sessions_add_friction_score;
mod m044_sessions_add_delight_count;

// Rules subsystem (51-70)
mod m051_rules_add_hook_command;
mod m052_rules_add_category;
mod m053_rules_add_priority;
mod m054_rules_add_surface_count;
mod m055_rules_idx_category;
mod m056_rules_idx_priority;
mod m057_rules_add_scope;
mod m058_rules_add_auto_approve_tools;
mod m059_rules_add_auto_approve_paths;
mod m060_rules_add_team_id;

// Skills subsystem (71-90)
mod m071_skills_add_summary;
mod m072_skills_add_preconditions;
mod m073_skills_add_postconditions;
mod m074_skills_add_validation_script;
mod m075_skills_add_invokable;
mod m076_skills_add_argument_hint;
mod m077_skills_add_context_mode;
mod m078_skills_add_agent_type;
mod m079_skills_add_allowed_tools;
mod m080_skills_add_hooks;
mod m081_skills_add_team_id;
mod m085_skills_add_disable_model_invocation;
mod m086_skills_add_disallowed_tools;

// Agents subsystem (91-110)
mod m091_task_leases_add_epoch;
mod m092_agents_add_worktree_id;
mod m093_agents_add_branch;
mod m094_task_leases_add_task_fk;
mod m095_working_epics_create_table;
mod m096_agents_add_ppid;
mod m097_agents_add_cc_session_id;
mod m098_agents_idx_ppid;

// Worktrees subsystem (111-130)
mod m111_worktrees_create_table;
mod m112_worktrees_idx_task;
mod m113_worktrees_idx_branch;
mod m114_worktrees_idx_status;
mod m115_worktrees_idx_path;
mod m116_tasks_add_branch;
mod m117_tasks_add_worktree_id;
mod m118_tasks_idx_branch;
mod m119_tasks_idx_worktree;
mod m120_worktrees_add_epic_id;
mod m121_worktrees_idx_epic;
mod m122_tasks_add_pending_verification;
mod m123_tasks_add_pending_worktree_merge;
mod m124_tasks_add_team_id;
mod m129_worktree_leases_create_table;
mod m130_worktrees_add_jj_fields;

// Code subsystem (131-150)
mod m131_code_files_create_table;
mod m132_code_symbols_create_table;
mod m133_code_relationships_create_table;
mod m134_code_memory_links_create_table;
mod m135_code_files_idx_path;
mod m136_code_files_idx_language;
mod m137_code_symbols_idx_name;
mod m138_code_symbols_idx_file;
mod m139_code_symbols_idx_kind;
mod m140_code_relationships_idx_source;
mod m141_prompts_create_table;
mod m142_file_changes_create_table;
mod m143_commit_links_create_table;

// Blame v2 migrations (154-164)
mod m154_prompts_add_messages_json;
mod m155_prompts_add_model;
mod m156_prompts_add_tool_version;
mod m157_file_changes_add_line_attributions;
mod m158_file_changes_add_human_modified_lines;
mod m159_line_attributions_create_table;
mod m160_line_attributions_idx_prompt;
mod m161_line_attributions_idx_file;
mod m162_line_attributions_idx_content_hash;
mod m163_file_snapshots_create_table;
mod m164_file_snapshots_idx_session;

// Events subsystem (151-170)
mod m151_events_create_table;
mod m152_agents_add_role;
mod m153_supervisor_queue_create_table;

// Verification subsystem (165+)
mod m165_verifications_add_verification_type;

// Epic verification owner (166)
mod m166_tasks_add_epic_verification_owner;

// Recording text search (167)
mod m167_recording_text_fts5;

// Recordings subsystem (171-190)
mod m171_recordings_create_table;
mod m172_recording_agents_create_table;
mod m173_recording_events_create_table;
mod m174_recordings_fts_create;
mod m175_recordings_idx_session;
mod m176_recordings_idx_started;
mod m177_recording_agents_idx_name;
mod m178_recording_agents_idx_recording;
mod m179_recording_events_idx_recording;
mod m180_recording_events_idx_timestamp;
mod m181_tasks_add_deliverables;
mod m182_tasks_add_demo_statement;
mod m183_entries_idx_team_id;
mod m184_rules_idx_team_id;
mod m185_skills_idx_team_id;
mod m186_tasks_idx_team_id;
mod m187_tasks_idx_assignee;
mod m188_id_sequences_create_table;
mod m189_events_friction_type_index;
mod m190_tasks_add_execution_note;
mod m191_agents_add_startup_confirmed;
mod m192_perf_indexes;
mod m193_spawn_queue_force_isolate;
mod m194_spawn_queue_repair_broken_schema;
mod m195_entries_add_share;
mod m196_rules_add_share;
mod m197_skills_add_share;
mod m198_tasks_add_share;
pub mod m199_known_repos;
mod m200_agents_add_pid_starttime;
mod m201_spawn_queue_add_worker_spec;
mod m202_tasks_add_depth;
mod m203_spawn_queue_add_factory_session;
mod m204_agents_add_factory_session;
mod m205_agents_factory_session_index;
mod m206_spawn_queue_add_task_id;
mod m207_task_lease_history_add_reason;
mod m208_prompt_queue_recipient_seen_create_table;
mod m209_retrieval_feedback_create_tables;
mod m210_verifier_authority;
mod m211_verification_dispatches;
mod m212_worker_delivery_transactions;
mod m213_verification_proof_boundaries;
pub mod m214_known_repo_bindings;
mod m215_verifier_server_handoffs;
mod m216_spawn_queue_add_requester_config_dir;
mod m217_spawn_queue_add_lifecycle_state;
mod m218_prompt_queue_recipient_transport_create_table;
mod m219_knowledge_store_create_tables;
mod m220_prompt_queue_wake_observability;
mod m221_history_index_create_tables;
mod m222_history_docs_create_table;
mod m223_history_commits_fts;
mod m224_history_commit_symbols;
mod m225_commit_links_link_method;
mod m226_knowledge_pages_add_attribution;
mod m227_knowledge_page_tombstones;
mod m228_history_epochs_create_table;
mod m229_code_vector_state;
mod m230_verification_repository_proof;
mod m231_sync_conflicts_create_table;
mod m232_worker_completion_receipts_add_artifact_path;
mod m233_tasks_add_terminal_outcome;
mod m234_external_task_dependencies;
mod m235_external_task_dependency_suppressions;
mod m236_delegation_receipts_create_table;
mod m237_entries_add_source_ids;
mod m238_skills_add_source_ids;
mod m239_task_execution_states_create_table;
mod m240_rule_skill_versions;
mod m241_tasks_add_origin_project;
mod m242_rule_skill_versions_operations;
mod m243_surfaced_artifacts_create_table;
mod m244_retrieval_outcomes_add_unresolved;
mod m245_retrieval_outcomes_add_attribution;
mod m246_spawn_queue_add_requester_secure_storage_dir;
mod m247_tasks_add_delivery_mode;
mod m248_tasks_retire_pending_supervisor_review;
mod m249_task_dependency_tombstones;
mod m250_quarantined_rows;
mod m251_sync_revisions;
mod m252_sync_conflicts_add_revisions;
mod m253_history_embedding_error;
mod m254_code_index_skipped_files;

/// All migrations in order. IDs must be sequential and never reused.
pub const MIGRATIONS: &[Migration] = &[
    // Entries
    m001_entries_add_session_id::MIGRATION,
    m002_entries_add_source_tool::MIGRATION,
    m003_entries_add_pending_extraction::MIGRATION,
    m004_entries_add_observation_type::MIGRATION,
    m005_entries_add_stability::MIGRATION,
    m006_entries_add_access_count::MIGRATION,
    m007_entries_add_raw_content::MIGRATION,
    m008_entries_add_compressed::MIGRATION,
    m009_entries_add_memory_tier::MIGRATION,
    m010_entries_add_importance::MIGRATION,
    m011_entries_add_valid_from::MIGRATION,
    m012_entries_add_valid_until::MIGRATION,
    m013_entries_add_review_after::MIGRATION,
    m014_entries_add_pending_embedding::MIGRATION,
    m015_entries_add_belief_type::MIGRATION,
    m016_entries_add_confidence::MIGRATION,
    m017_entries_add_domain::MIGRATION,
    m018_entries_idx_session::MIGRATION,
    m019_entries_idx_pending::MIGRATION,
    m020_entries_idx_obs_type::MIGRATION,
    m021_entries_idx_stability::MIGRATION,
    m022_entries_idx_memory_tier::MIGRATION,
    m023_entries_idx_importance::MIGRATION,
    m024_entries_idx_pending_embedding::MIGRATION,
    m025_entries_idx_belief_type::MIGRATION,
    m026_entries_idx_confidence::MIGRATION,
    m027_entries_idx_domain::MIGRATION,
    m028_sessions_add_title::MIGRATION,
    m029_entries_add_branch::MIGRATION,
    m030_entries_idx_branch::MIGRATION,
    m031_sessions_add_branch::MIGRATION,
    m032_sessions_add_worktree_id::MIGRATION,
    m033_entries_add_scope::MIGRATION,
    m034_entries_add_team_id::MIGRATION,
    m038_entries_add_last_reviewed::MIGRATION,
    m039_entries_add_updated_at::MIGRATION,
    m040_entries_add_indexed_at::MIGRATION,
    m041_entries_idx_pending_index::MIGRATION,
    m042_sessions_add_outcome::MIGRATION,
    m043_sessions_add_friction_score::MIGRATION,
    m044_sessions_add_delight_count::MIGRATION,
    // Rules
    // Historical compatibility migration: the rule model no longer exposes
    // or writes this legacy column, but the ID remains in the forward-only
    // ledger for databases that already applied it.
    m051_rules_add_hook_command::MIGRATION,
    m052_rules_add_category::MIGRATION,
    m053_rules_add_priority::MIGRATION,
    m054_rules_add_surface_count::MIGRATION,
    m055_rules_idx_category::MIGRATION,
    m056_rules_idx_priority::MIGRATION,
    m057_rules_add_scope::MIGRATION,
    m058_rules_add_auto_approve_tools::MIGRATION,
    m059_rules_add_auto_approve_paths::MIGRATION,
    m060_rules_add_team_id::MIGRATION,
    // Skills
    m071_skills_add_summary::MIGRATION,
    m072_skills_add_preconditions::MIGRATION,
    m073_skills_add_postconditions::MIGRATION,
    m074_skills_add_validation_script::MIGRATION,
    m075_skills_add_invokable::MIGRATION,
    m076_skills_add_argument_hint::MIGRATION,
    m077_skills_add_context_mode::MIGRATION,
    m078_skills_add_agent_type::MIGRATION,
    m079_skills_add_allowed_tools::MIGRATION,
    m080_skills_add_hooks::MIGRATION,
    m081_skills_add_team_id::MIGRATION,
    m085_skills_add_disable_model_invocation::MIGRATION,
    m086_skills_add_disallowed_tools::MIGRATION,
    // Agents
    m091_task_leases_add_epoch::MIGRATION,
    m092_agents_add_worktree_id::MIGRATION,
    m093_agents_add_branch::MIGRATION,
    m094_task_leases_add_task_fk::MIGRATION,
    m095_working_epics_create_table::MIGRATION,
    m096_agents_add_ppid::MIGRATION,
    m097_agents_add_cc_session_id::MIGRATION,
    m098_agents_idx_ppid::MIGRATION,
    // Worktrees
    m111_worktrees_create_table::MIGRATION,
    m112_worktrees_idx_task::MIGRATION,
    m113_worktrees_idx_branch::MIGRATION,
    m114_worktrees_idx_status::MIGRATION,
    m115_worktrees_idx_path::MIGRATION,
    m116_tasks_add_branch::MIGRATION,
    m117_tasks_add_worktree_id::MIGRATION,
    m118_tasks_idx_branch::MIGRATION,
    m119_tasks_idx_worktree::MIGRATION,
    m120_worktrees_add_epic_id::MIGRATION,
    m121_worktrees_idx_epic::MIGRATION,
    m122_tasks_add_pending_verification::MIGRATION,
    m123_tasks_add_pending_worktree_merge::MIGRATION,
    m124_tasks_add_team_id::MIGRATION,
    m129_worktree_leases_create_table::MIGRATION,
    m130_worktrees_add_jj_fields::MIGRATION,
    // Code
    m131_code_files_create_table::MIGRATION,
    m132_code_symbols_create_table::MIGRATION,
    m133_code_relationships_create_table::MIGRATION,
    m134_code_memory_links_create_table::MIGRATION,
    m135_code_files_idx_path::MIGRATION,
    m136_code_files_idx_language::MIGRATION,
    m137_code_symbols_idx_name::MIGRATION,
    m138_code_symbols_idx_file::MIGRATION,
    m139_code_symbols_idx_kind::MIGRATION,
    m140_code_relationships_idx_source::MIGRATION,
    m141_prompts_create_table::MIGRATION,
    m142_file_changes_create_table::MIGRATION,
    m143_commit_links_create_table::MIGRATION,
    // Events
    m151_events_create_table::MIGRATION,
    // Agent roles (factory sessions)
    m152_agents_add_role::MIGRATION,
    m153_supervisor_queue_create_table::MIGRATION,
    // Blame v2
    m154_prompts_add_messages_json::MIGRATION,
    m155_prompts_add_model::MIGRATION,
    m156_prompts_add_tool_version::MIGRATION,
    m157_file_changes_add_line_attributions::MIGRATION,
    m158_file_changes_add_human_modified_lines::MIGRATION,
    m159_line_attributions_create_table::MIGRATION,
    m160_line_attributions_idx_prompt::MIGRATION,
    m161_line_attributions_idx_file::MIGRATION,
    m162_line_attributions_idx_content_hash::MIGRATION,
    m163_file_snapshots_create_table::MIGRATION,
    m164_file_snapshots_idx_session::MIGRATION,
    // Verification
    m165_verifications_add_verification_type::MIGRATION,
    // Epic verification owner
    m166_tasks_add_epic_verification_owner::MIGRATION,
    // Recording text search
    m167_recording_text_fts5::MIGRATION,
    // Recordings
    m171_recordings_create_table::MIGRATION,
    m172_recording_agents_create_table::MIGRATION,
    m173_recording_events_create_table::MIGRATION,
    m174_recordings_fts_create::MIGRATION,
    m175_recordings_idx_session::MIGRATION,
    m176_recordings_idx_started::MIGRATION,
    m177_recording_agents_idx_name::MIGRATION,
    m178_recording_agents_idx_recording::MIGRATION,
    m179_recording_events_idx_recording::MIGRATION,
    m180_recording_events_idx_timestamp::MIGRATION,
    m181_tasks_add_deliverables::MIGRATION,
    m182_tasks_add_demo_statement::MIGRATION,
    // Missing team_id and assignee indexes
    m183_entries_idx_team_id::MIGRATION,
    m184_rules_idx_team_id::MIGRATION,
    m185_skills_idx_team_id::MIGRATION,
    m186_tasks_idx_team_id::MIGRATION,
    m187_tasks_idx_assignee::MIGRATION,
    // ID sequences
    m188_id_sequences_create_table::MIGRATION,
    // Events friction_type index
    m189_events_friction_type_index::MIGRATION,
    // Execution note for task methodology tracking (cas-7fc1)
    m190_tasks_add_execution_note::MIGRATION,
    // Agent startup confirmation for crash-on-startup detection (cas-714d)
    m191_agents_add_startup_confirmed::MIGRATION,
    // Performance indexes for hot polling paths (cas-aee3)
    m192_perf_indexes::MIGRATION,
    // Spawn queue force/isolate columns (moved from hand-rolled init, cas-3c74)
    m193_spawn_queue_force_isolate::MIGRATION,
    // Repair spawn_queue DBs corrupted by earlier m193 revision (cas-a68b)
    m194_spawn_queue_repair_broken_schema::MIGRATION,
    // Add share column to entries/rules/skills/tasks for pre-column DBs
    m195_entries_add_share::MIGRATION,
    m196_rules_add_share::MIGRATION,
    m197_skills_add_share::MIGRATION,
    m198_tasks_add_share::MIGRATION,
    // Host-scoped known_repos registry for cross-repo sweep (EPIC cas-7c88, Unit 4)
    m199_known_repos::MIGRATION,
    // Typed PID-reuse fingerprint on agents (EPIC cas-9508 / cas-b157)
    m200_agents_add_pid_starttime::MIGRATION,
    // Add worker_spec column to spawn_queue for per-worker CLI/model/effort overrides (cas-2992)
    m201_spawn_queue_add_worker_spec::MIGRATION,
    // Add depth column to tasks for per-task speed mode (EPIC cas-1255 / cas-0344)
    m202_tasks_add_depth::MIGRATION,
    // Scope spawn/shutdown requests to the owning factory session (cas-0f7f)
    m203_spawn_queue_add_factory_session::MIGRATION,
    // Scope factory agent visibility to the owning factory session (cas-7baa)
    m204_agents_add_factory_session::MIGRATION,
    // idx_agents_factory_session moved out of baseline AGENT_SCHEMA (2026-07-06
    // serve-brick hotfix): baselines run at store open BEFORE migrations, so a
    // baseline index on the m204-added column aborted init on pre-m204 DBs.
    m205_agents_factory_session_index::MIGRATION,
    // Add task_id column to spawn_queue for spawn-time task pre-assignment (cas-6913)
    m206_spawn_queue_add_task_id::MIGRATION,
    // Separate human-readable lease event reasons from transfer agent IDs (cas-7aef)
    m207_task_lease_history_add_reason::MIGRATION,
    // Register worker inbox per-recipient seen state (cas-337e)
    m208_prompt_queue_recipient_seen_create_table::MIGRATION,
    // Versioned retrieval identity and explicit outcome feedback (cas-aeac)
    m209_retrieval_feedback_create_tables::MIGRATION,
    // Typed verifier provenance and one-time child capabilities (cas-941b)
    m210_verifier_authority::MIGRATION,
    // Explicit task-scoped verification dispatch ownership and recovery (cas-08ca)
    m211_verification_dispatches::MIGRATION,
    m212_worker_delivery_transactions::MIGRATION,
    m213_verification_proof_boundaries::MIGRATION,
    m214_known_repo_bindings::MIGRATION,
    m215_verifier_server_handoffs::MIGRATION,
    // Preserve requesting supervisor CLAUDE_CONFIG_DIR across daemon queue consumption.
    m216_spawn_queue_add_requester_config_dir::MIGRATION,
    // GH #60: spawn lifecycle state per request (queued → provisioning →
    // launched → registered/FAILED) so a spawn that never registers is
    // queryable as FAILED rather than silence.
    m217_spawn_queue_add_lifecycle_state::MIGRATION,
    m218_prompt_queue_recipient_transport_create_table::MIGRATION,
    // Project knowledge distillation store: page index, content-hash source
    // ledger and contentless FTS5 body index (EPIC cas-7d31 / cas-cbf1).
    m219_knowledge_store_create_tables::MIGRATION,
    m220_prompt_queue_wake_observability::MIGRATION,
    m221_history_index_create_tables::MIGRATION,
    m222_history_docs_create_table::MIGRATION,
    // Lexical (FTS5) half of the history query surface (EPIC cas-6212 /
    // cas-7f40), separate from m221 so installs that already applied it are
    // reached. Renumbered 222 → 223 when the M6 lane claimed 222 first; IDs
    // are never reused, so the later lane moves.
    m223_history_commits_fts::MIGRATION,
    // Commit ↔ symbol mapping plus the history_commits.symbol_mapping verdict
    // column (EPIC cas-6212 / cas-0562), separate from m221 for the same reason
    // m223 is. Renumbered 222 → 224 after the M6 and M4 lanes claimed 222/223.
    m224_history_commit_symbols::MIGRATION,
    // Provenance link_method (EPIC cas-6212 / cas-519f, spec §5.3): the column
    // that keeps a reconstructed commit link distinguishable from an observed
    // one, now that the history indexer is a second writer of commit_links.
    // Renumbered 224 → 225 after the M3 lane claimed 224.
    m225_commit_links_link_method::MIGRATION,
    m226_knowledge_pages_add_attribution::MIGRATION,
    m227_knowledge_page_tombstones::MIGRATION,
    // Running-binary timeline behind the is-it-fixed verdict (EPIC cas-6212 /
    // cas-8d2a, spec §9). Renumbered 224 → 226 → 228 as the parallel M3/M5
    // and knowledge-store lanes claimed the earlier IDs; migration IDs are
    // never reused.
    m228_history_epochs_create_table::MIGRATION,
    m229_code_vector_state::MIGRATION,
    m230_verification_repository_proof::MIGRATION,
    m231_sync_conflicts_create_table::MIGRATION,
    m232_worker_completion_receipts_add_artifact_path::MIGRATION,
    m233_tasks_add_terminal_outcome::MIGRATION,
    m234_external_task_dependencies::MIGRATION,
    m235_external_task_dependency_suppressions::MIGRATION,
    m236_delegation_receipts_create_table::MIGRATION,
    m237_entries_add_source_ids::MIGRATION,
    m238_skills_add_source_ids::MIGRATION,
    m239_task_execution_states_create_table::MIGRATION,
    m240_rule_skill_versions::MIGRATION,
    m241_tasks_add_origin_project::MIGRATION,
    m242_rule_skill_versions_operations::MIGRATION,
    m243_surfaced_artifacts_create_table::MIGRATION,
    m244_retrieval_outcomes_add_unresolved::MIGRATION,
    m245_retrieval_outcomes_add_attribution::MIGRATION,
    m246_spawn_queue_add_requester_secure_storage_dir::MIGRATION,
    m247_tasks_add_delivery_mode::MIGRATION,
    m248_tasks_retire_pending_supervisor_review::MIGRATION,
    m249_task_dependency_tombstones::MIGRATION,
    m250_quarantined_rows::MIGRATION,
    m251_sync_revisions::MIGRATION,
    m252_sync_conflicts_add_revisions::MIGRATION,
    m253_history_embedding_error::MIGRATION,
    m254_code_index_skipped_files::MIGRATION,
];

#[cfg(test)]
mod tests {
    use crate::migration::migrations::*;
    use std::collections::{BTreeMap, HashSet};

    fn assert_migration_registry_is_valid(migrations: &[Migration]) {
        let mut ids = BTreeMap::new();
        let mut previous: Option<&Migration> = None;

        for migration in migrations {
            if let Some(first) = ids.insert(migration.id, migration) {
                panic!(
                    "Duplicate migration ID {} registered by {} and {}",
                    migration.id, first.name, migration.name
                );
            }
            if let Some(previous) = previous {
                assert!(
                    migration.id > previous.id,
                    "Migration ID ordering violation: {} ({}) follows {} ({})",
                    migration.id,
                    migration.name,
                    previous.id,
                    previous.name
                );
            }
            previous = Some(migration);
        }
    }

    #[test]
    fn migration_registry_ids_are_unique_and_ordered() {
        assert_migration_registry_is_valid(MIGRATIONS);
    }

    #[test]
    #[should_panic(
        expected = "Duplicate migration ID 1 registered by entries_add_session_id and synthetic_duplicate"
    )]
    fn migration_registry_duplicate_id_names_both_migrations() {
        let first = MIGRATIONS[0].clone();
        let duplicate = Migration {
            name: "synthetic_duplicate",
            ..first.clone()
        };

        assert_migration_registry_is_valid(&[first, duplicate]);
    }

    #[test]
    fn test_migration_names_unique() {
        let mut seen = HashSet::new();
        for m in MIGRATIONS {
            assert!(seen.insert(m.name), "Duplicate migration name: {}", m.name);
        }
    }

    #[test]
    fn test_all_migrations_have_detection() {
        for m in MIGRATIONS {
            assert!(
                m.detect.is_some(),
                "Migration {} ({}) missing detection query",
                m.id,
                m.name
            );
        }
    }

    /// cas-0955: guard against orphan migration files.
    ///
    /// Thirteen files (m035/m036/m037, m061-m063, m082-m084, m125-m128) sat in
    /// this directory for months with no `mod` declaration and no `MIGRATIONS`
    /// entry, so they never compiled and never ran — the schema changes they
    /// described (`visibility`, `owner_id`, `collaborators`) simply did not
    /// exist in any live database. A migration that is not declared *and*
    /// registered is dead code that reads like shipped schema, so fail loudly
    /// the moment one appears.
    #[test]
    fn test_every_migration_file_is_declared_and_registered() {
        let dir = crate::test_paths::crate_root().join("src/migration/migrations");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // Source tree unavailable (packaged/vendored build) — nothing to check.
            return;
        };
        let source = std::fs::read_to_string(dir.join("mod.rs"))
            .expect("migrations/mod.rs must be readable when the source tree is present");

        let declared: HashSet<&str> = source
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let rest = line
                    .strip_prefix("pub mod ")
                    .or_else(|| line.strip_prefix("mod "))?;
                rest.strip_suffix(';')
            })
            .collect();
        let registered: HashSet<&str> = source
            .lines()
            .filter_map(|line| line.trim().strip_suffix("::MIGRATION,"))
            .collect();

        let mut modules: Vec<String> = entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension()? != "rs" {
                    return None;
                }
                let stem = path.file_stem()?.to_str()?.to_string();
                (stem != "mod").then_some(stem)
            })
            .collect();
        modules.sort();
        assert!(
            !modules.is_empty(),
            "no migration files found in {} — the guard would pass vacuously",
            dir.display()
        );

        let orphans: Vec<&String> = modules
            .iter()
            .filter(|m| !declared.contains(m.as_str()) || !registered.contains(m.as_str()))
            .collect();
        assert!(
            orphans.is_empty(),
            "migration files exist but are never compiled or run \
             (missing a `mod` declaration and/or a MIGRATIONS entry in mod.rs): {orphans:?}"
        );
    }
}
