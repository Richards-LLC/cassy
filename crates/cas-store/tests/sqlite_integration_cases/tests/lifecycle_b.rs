use crate::*;

#[test]
fn test_sqlite_skill_store_full_lifecycle() {
    let temp = setup_temp_dir();
    let store = SqliteSkillStore::open(temp.path()).expect("Failed to create store");
    store.init().expect("Failed to init store");

    // Generate ID
    let id = store.generate_id().expect("Failed to generate ID");
    assert!(id.starts_with("cas-sk"));

    // Create and add skill
    let mut skill = Skill::new(id.clone(), "Git Commit".to_string());
    skill.description = "Create git commits".to_string();
    skill.invocation = "/commit".to_string();
    skill.status = SkillStatus::Enabled;
    skill.tags = vec!["git".to_string()];
    store.add(&skill).expect("Failed to add skill");

    // Get skill
    let retrieved = store.get(&id).expect("Failed to get skill");
    assert_eq!(retrieved.name, "Git Commit");
    assert_eq!(retrieved.status, SkillStatus::Enabled);

    // Update skill
    let mut updated = retrieved.clone();
    updated.status = SkillStatus::Disabled;
    store.update(&updated).expect("Failed to update skill");

    let after_update = store.get(&id).expect("Failed to get after update");
    assert_eq!(after_update.status, SkillStatus::Disabled);

    // List skills
    let all = store.list(None).expect("Failed to list all skills");
    assert_eq!(all.len(), 1);

    // List enabled (should be empty now)
    let enabled = store.list_enabled().expect("Failed to list enabled");
    assert_eq!(enabled.len(), 0);

    // Search
    let search_results = store.search("git").expect("Failed to search");
    assert_eq!(search_results.len(), 1);

    // Delete
    store.delete(&id).expect("Failed to delete skill");
    assert_eq!(store.get(&id).unwrap().status, SkillStatus::Retired);

    store.close().expect("Failed to close store");
}

// =============================================================================
// SqliteEntityStore Integration Tests
// =============================================================================

#[test]
fn test_sqlite_entity_store_full_lifecycle() {
    let temp = setup_temp_dir();
    let store = SqliteEntityStore::open(temp.path()).expect("Failed to create store");
    store.init().expect("Failed to init store");

    // Create and add entity
    let id = store.generate_entity_id().expect("Failed to generate ID");
    let mut entity = Entity::new(id.clone(), "Rust".to_string(), EntityType::Tool);
    entity.description = Some("Systems programming language".to_string());
    entity.aliases = vec!["rust-lang".to_string()];
    store.add_entity(&entity).expect("Failed to add entity");

    // Get entity
    let retrieved = store.get_entity(&id).expect("Failed to get entity");
    assert_eq!(retrieved.name, "Rust");
    assert_eq!(retrieved.entity_type, EntityType::Tool);

    // Get by name
    let by_name = store
        .get_entity_by_name("Rust", None)
        .expect("Failed to get by name");
    assert!(by_name.is_some());
    assert_eq!(by_name.unwrap().id, id);

    // Get by alias
    let by_alias = store
        .get_entity_by_name("rust-lang", None)
        .expect("Failed to get by alias");
    assert!(by_alias.is_some());

    // Update entity
    let mut updated = retrieved.clone();
    updated.description = Some("Updated description".to_string());
    store
        .update_entity(&updated)
        .expect("Failed to update entity");

    // List entities
    let all = store.list_entities(None).expect("Failed to list entities");
    assert_eq!(all.len(), 1);

    let tech_only = store
        .list_entities(Some(EntityType::Tool))
        .expect("Failed to list tech");
    assert_eq!(tech_only.len(), 1);

    // Search entities
    let search = store
        .search_entities("rust", None)
        .expect("Failed to search");
    assert_eq!(search.len(), 1);

    // Delete entity
    store.delete_entity(&id).expect("Failed to delete entity");
    assert!(store.get_entity(&id).is_err());

    store.close().expect("Failed to close store");
}

#[test]
fn test_sqlite_entity_store_relationships() {
    let temp = setup_temp_dir();
    let store = SqliteEntityStore::open(temp.path()).expect("Failed to create store");
    store.init().expect("Failed to init store");

    // Create entities
    let e1 = Entity::new("ent-1".to_string(), "CAS".to_string(), EntityType::Project);
    let e2 = Entity::new("ent-2".to_string(), "Rust".to_string(), EntityType::Tool);

    store.add_entity(&e1).expect("Failed to add e1");
    store.add_entity(&e2).expect("Failed to add e2");

    // Create relationship
    let rel_id = store
        .generate_relationship_id()
        .expect("Failed to generate rel ID");
    let rel = Relationship::new(
        rel_id.clone(),
        "ent-1".to_string(),
        "ent-2".to_string(),
        RelationType::Uses,
    );
    store
        .add_relationship(&rel)
        .expect("Failed to add relationship");

    // Get relationship
    let retrieved = store
        .get_relationship(&rel_id)
        .expect("Failed to get relationship");
    assert_eq!(retrieved.relation_type, RelationType::Uses);

    // Get relationship between
    let between = store
        .get_relationship_between("ent-1", "ent-2", None)
        .expect("Failed to get between");
    assert!(between.is_some());

    // Get entity relationships
    let e1_rels = store
        .get_entity_relationships("ent-1")
        .expect("Failed to get entity rels");
    assert_eq!(e1_rels.len(), 1);

    // Get outgoing/incoming
    let outgoing = store
        .get_outgoing_relationships("ent-1")
        .expect("Failed to get outgoing");
    assert_eq!(outgoing.len(), 1);

    let incoming = store
        .get_incoming_relationships("ent-2")
        .expect("Failed to get incoming");
    assert_eq!(incoming.len(), 1);

    // Get connected entities
    let connected = store
        .get_connected_entities("ent-1")
        .expect("Failed to get connected");
    assert_eq!(connected.len(), 1);
    assert_eq!(connected[0].0.name, "Rust");

    // Delete relationship
    store
        .delete_relationship(&rel_id)
        .expect("Failed to delete relationship");
    assert!(store.get_relationship(&rel_id).is_err());
}

// =============================================================================
// Incremental Indexing Integration Tests
// =============================================================================

#[test]
fn test_sqlite_store_pending_index_tracking() {
    let temp = setup_temp_dir();
    let store = SqliteStore::open(temp.path()).expect("Failed to create store");
    store.init().expect("Failed to init store");

    // Initially no pending entries
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list pending");
    assert_eq!(pending.len(), 0, "Should start with no pending entries");

    // Add entries - they should be pending immediately
    let entry1 = Entry::new(
        "idx-001".to_string(),
        "First entry for indexing".to_string(),
    );
    let entry2 = Entry::new(
        "idx-002".to_string(),
        "Second entry for indexing".to_string(),
    );
    store.add(&entry1).expect("Failed to add entry1");
    store.add(&entry2).expect("Failed to add entry2");

    // Verify entries are pending (updated_at set, indexed_at NULL)
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list pending");
    assert_eq!(pending.len(), 2, "Both entries should be pending");

    // Mark first entry as indexed
    store
        .mark_indexed("idx-001")
        .expect("Failed to mark indexed");

    // Now only second entry should be pending
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list pending");
    assert_eq!(pending.len(), 1, "Only one entry should be pending");
    assert_eq!(pending[0].id, "idx-002");

    // Mark second entry as indexed
    store
        .mark_indexed("idx-002")
        .expect("Failed to mark indexed");

    // No entries pending
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list pending");
    assert_eq!(pending.len(), 0, "No entries should be pending");

    store.close().expect("Failed to close store");
}

#[test]
fn test_sqlite_store_update_triggers_reindex() {
    let temp = setup_temp_dir();
    let store = SqliteStore::open(temp.path()).expect("Failed to create store");
    store.init().expect("Failed to init store");

    // Add entry
    let entry = Entry::new("upd-001".to_string(), "Entry content".to_string());
    store.add(&entry).expect("Failed to add entry");

    // Mark as indexed
    store
        .mark_indexed("upd-001")
        .expect("Failed to mark indexed");

    // Verify not pending
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list pending");
    assert_eq!(
        pending.len(),
        0,
        "Entry should not be pending after indexing"
    );

    // Update the entry - should trigger reindex
    let mut updated = store.get("upd-001").expect("Failed to get entry");
    updated.content = "Updated content that needs reindexing".to_string();
    store.update(&updated).expect("Failed to update entry");

    // Entry should be pending again (updated_at > indexed_at)
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list pending");
    assert_eq!(
        pending.len(),
        1,
        "Updated entry should be pending for reindex"
    );
    assert_eq!(pending[0].id, "upd-001");

    store.close().expect("Failed to close store");
}

#[test]
fn test_sqlite_store_batch_mark_indexed() {
    let temp = setup_temp_dir();
    let store = SqliteStore::open(temp.path()).expect("Failed to create store");
    store.init().expect("Failed to init store");

    // Add multiple entries
    for i in 0..10 {
        let entry = Entry::new(format!("batch-{i:03}"), format!("Batch entry {i}"));
        store.add(&entry).expect("Failed to add entry");
    }

    // All should be pending
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list pending");
    assert_eq!(pending.len(), 10, "All 10 entries should be pending");

    // Batch mark first 5 as indexed
    let id_strings: Vec<String> = (0..5).map(|i| format!("batch-{i:03}")).collect();
    let id_refs: Vec<&str> = id_strings.iter().map(|s| s.as_str()).collect();
    store
        .mark_indexed_batch(&id_refs)
        .expect("Failed to batch mark indexed");

    // Only last 5 should be pending
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list pending");
    assert_eq!(pending.len(), 5, "Only 5 entries should be pending");
    for p in &pending {
        assert!(
            p.id.starts_with("batch-00") && p.id.as_str() >= "batch-005",
            "Pending entry {} should be >= batch-005",
            p.id
        );
    }

    store
        .mark_index_pending_batch(&["batch-001", "batch-003"])
        .expect("Failed to batch re-queue indexed entries");
    let pending = store
        .list_pending_index(100)
        .expect("Failed to list re-queued entries");
    assert_eq!(
        pending.len(),
        7,
        "Two indexed entries should be pending again"
    );
    assert!(pending.iter().any(|entry| entry.id == "batch-001"));
    assert!(pending.iter().any(|entry| entry.id == "batch-003"));

    store.close().expect("Failed to close store");
}
