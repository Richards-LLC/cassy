use crate::support::*;
use cas::mcp::CasCore;
use cas::mcp::tools::*;
use cas::store::{open_entity_store, open_store};
use cas::types::{Entity, EntityType, Scope};
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use std::collections::HashMap;
use tempfile::TempDir;

/// Helper to setup CAS with entity store initialized
fn setup_cas_with_entities(test_env: &mut TestEnvGuard) -> (TempDir, CasCore, CasService) {
    let (temp, core) = setup_cas(test_env);
    let cas_dir = temp.path().join(".cas");

    // Initialize entity store
    let entity_store = open_entity_store(&cas_dir).unwrap();
    entity_store.init().unwrap();

    let service = CasService::new(core.clone(), None);
    (temp, core, service)
}

/// Helper to create a test entity
fn create_test_entity(
    entity_store: &dyn cas_store::EntityStore,
    name: &str,
    entity_type: EntityType,
    description: Option<&str>,
) -> String {
    let id = entity_store.generate_entity_id().unwrap();
    let entity = Entity {
        id: id.clone(),
        name: name.to_string(),
        entity_type,
        aliases: vec![],
        description: description.map(|s| s.to_string()),
        created: Utc::now(),
        updated: Utc::now(),
        mention_count: 0,
        confidence: 0.9,
        archived: false,
        metadata: HashMap::new(),
        summary: None,
        summary_updated: None,
    };
    entity_store.add_entity(&entity).unwrap();
    id
}

/// Helper to generate entry ID
fn gen_entry_id(store: &dyn cas_store::Store) -> String {
    store.generate_id().unwrap()
}

#[tokio::test]
async fn test_entity_list_query_filter() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, _core, service) = setup_cas_with_entities(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let entity_store = open_entity_store(&cas_dir).unwrap();

    // Create test entities (Tool is the technology type)
    create_test_entity(
        &*entity_store,
        "Python",
        EntityType::Tool,
        Some("A programming language"),
    );
    create_test_entity(
        &*entity_store,
        "JavaScript",
        EntityType::Tool,
        Some("Web scripting language"),
    );
    create_test_entity(&*entity_store, "John Smith", EntityType::Person, None);

    // Test query filter by name
    let req = EntityListRequest {
        entity_type: None,
        query: Some("Python".to_string()),
        tags: None,
        scope: None,
        sort: None,
        sort_order: None,
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_list(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);
    assert!(text.contains("Python"), "Should find Python");
    assert!(!text.contains("JavaScript"), "Should NOT find JavaScript");

    // Test query filter by description
    let req = EntityListRequest {
        entity_type: None,
        query: Some("scripting".to_string()),
        tags: None,
        scope: None,
        sort: None,
        sort_order: None,
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_list(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);
    assert!(
        text.contains("JavaScript"),
        "Should find JavaScript via description"
    );
    assert!(!text.contains("Python"), "Should NOT find Python");
}

#[tokio::test]
async fn test_entity_list_sort_by_name() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, _core, service) = setup_cas_with_entities(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let entity_store = open_entity_store(&cas_dir).unwrap();

    // Create entities with different names
    create_test_entity(&*entity_store, "Zebra", EntityType::Concept, None);
    create_test_entity(&*entity_store, "Alpha", EntityType::Concept, None);
    create_test_entity(&*entity_store, "Middle", EntityType::Concept, None);

    // Test sort by name ascending
    let req = EntityListRequest {
        entity_type: None,
        query: None,
        tags: None,
        scope: None,
        sort: Some("name".to_string()),
        sort_order: Some("asc".to_string()),
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_list(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);

    // Verify order: Alpha should appear before Middle, Middle before Zebra
    let alpha_pos = text.find("Alpha").expect("Alpha should exist");
    let middle_pos = text.find("Middle").expect("Middle should exist");
    let zebra_pos = text.find("Zebra").expect("Zebra should exist");
    assert!(
        alpha_pos < middle_pos,
        "Alpha should come before Middle (asc)"
    );
    assert!(
        middle_pos < zebra_pos,
        "Middle should come before Zebra (asc)"
    );

    // Test sort by name descending
    let req = EntityListRequest {
        entity_type: None,
        query: None,
        tags: None,
        scope: None,
        sort: Some("name".to_string()),
        sort_order: Some("desc".to_string()),
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_list(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);

    let alpha_pos = text.find("Alpha").expect("Alpha should exist");
    let zebra_pos = text.find("Zebra").expect("Zebra should exist");
    assert!(
        zebra_pos < alpha_pos,
        "Zebra should come before Alpha (desc)"
    );
}

#[tokio::test]
async fn test_entity_list_sort_by_mentions() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, _core, service) = setup_cas_with_entities(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let entity_store = open_entity_store(&cas_dir).unwrap();

    // Create entities with different mention counts
    let id1 = entity_store.generate_entity_id().unwrap();
    let entity1 = Entity {
        id: id1.clone(),
        name: "LowMentions".to_string(),
        entity_type: EntityType::Tool,
        aliases: vec![],
        description: None,
        created: Utc::now(),
        updated: Utc::now(),
        mention_count: 1,
        confidence: 0.9,
        archived: false,
        metadata: HashMap::new(),
        summary: None,
        summary_updated: None,
    };
    entity_store.add_entity(&entity1).unwrap();

    let id2 = entity_store.generate_entity_id().unwrap();
    let entity2 = Entity {
        id: id2.clone(),
        name: "HighMentions".to_string(),
        entity_type: EntityType::Tool,
        aliases: vec![],
        description: None,
        created: Utc::now(),
        updated: Utc::now(),
        mention_count: 100,
        confidence: 0.9,
        archived: false,
        metadata: HashMap::new(),
        summary: None,
        summary_updated: None,
    };
    entity_store.add_entity(&entity2).unwrap();

    // Test sort by mentions descending
    let req = EntityListRequest {
        entity_type: None,
        query: None,
        tags: None,
        scope: None,
        sort: Some("mentions".to_string()),
        sort_order: Some("desc".to_string()),
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_list(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);

    let high_pos = text
        .find("HighMentions")
        .expect("HighMentions should exist");
    let low_pos = text.find("LowMentions").expect("LowMentions should exist");
    assert!(
        high_pos < low_pos,
        "HighMentions should come before LowMentions (desc)"
    );
}

#[tokio::test]
async fn test_entity_extract_query_filter() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, _core, service) = setup_cas_with_entities(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let store = open_store(&cas_dir).unwrap();

    // Create entries with different content
    use cas::types::Entry;
    let id1 = gen_entry_id(&*store);
    let entry1 = Entry::new(id1, "TypeScript is great for web development".to_string());
    store.add(&entry1).unwrap();

    let id2 = gen_entry_id(&*store);
    let entry2 = Entry::new(id2, "Python is excellent for data science".to_string());
    store.add(&entry2).unwrap();

    // Extract entities from entries containing "TypeScript"
    let req = EntityExtractRequest {
        query: Some("TypeScript".to_string()),
        scope: None,
        tags: None,
        entity_type: None,
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_extract(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);
    assert!(
        text.contains("Entity extraction complete"),
        "Should complete extraction"
    );
}

#[tokio::test]
async fn test_entity_extract_scope_filter() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, _core, service) = setup_cas_with_entities(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let store = open_store(&cas_dir).unwrap();

    // Create a global entry
    use cas::types::Entry;
    let id1 = gen_entry_id(&*store);
    let mut entry1 = Entry::new(id1, "Global entry mentioning Rust".to_string());
    entry1.scope = Scope::Global;
    store.add(&entry1).unwrap();

    // Create a project entry
    let id2 = gen_entry_id(&*store);
    let mut entry2 = Entry::new(id2, "Project entry mentioning Go".to_string());
    entry2.scope = Scope::Project;
    store.add(&entry2).unwrap();

    // Extract from global scope only
    let req = EntityExtractRequest {
        query: None,
        scope: Some("global".to_string()),
        tags: None,
        entity_type: None,
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_extract(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);
    assert!(
        text.contains("Entity extraction complete"),
        "Should complete extraction"
    );
}

#[tokio::test]
async fn test_entity_extract_tags_filter() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, _core, service) = setup_cas_with_entities(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let store = open_store(&cas_dir).unwrap();

    // Create entry with tags
    use cas::types::Entry;
    let id1 = gen_entry_id(&*store);
    let mut entry1 = Entry::new(id1, "Entry about Java programming".to_string());
    entry1.tags = vec!["programming".to_string(), "backend".to_string()];
    store.add(&entry1).unwrap();

    // Create entry without tags
    let id2 = gen_entry_id(&*store);
    let entry2 = Entry::new(id2, "Entry about C++ programming".to_string());
    store.add(&entry2).unwrap();

    // Extract from entries with "programming" tag
    let req = EntityExtractRequest {
        query: None,
        scope: None,
        tags: Some("programming".to_string()),
        entity_type: None,
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_extract(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);
    assert!(
        text.contains("Entity extraction complete"),
        "Should complete extraction"
    );
}

#[tokio::test]
async fn test_entity_extract_entity_type_filter() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, _core, service) = setup_cas_with_entities(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let store = open_store(&cas_dir).unwrap();

    // Create entry mentioning both a person and technology
    use cas::types::Entry;
    let id = gen_entry_id(&*store);
    let entry = Entry::new(id, "Alice uses Kubernetes for deployment".to_string());
    store.add(&entry).unwrap();

    // Extract only Tool (technology) entities
    let req = EntityExtractRequest {
        query: None,
        scope: None,
        tags: None,
        entity_type: Some("tool".to_string()),
        limit: None,
    };
    let result = service
        .inner
        .cas_entity_extract(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);
    assert!(
        text.contains("Entity extraction complete"),
        "Should complete extraction"
    );
}

#[tokio::test]
async fn test_entity_list_combined_filters() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, _core, service) = setup_cas_with_entities(&mut test_env);
    let cas_dir = temp.path().join(".cas");
    let entity_store = open_entity_store(&cas_dir).unwrap();

    // Create multiple entities (Tool is the technology type)
    create_test_entity(
        &*entity_store,
        "React Framework",
        EntityType::Tool,
        Some("Frontend library"),
    );
    create_test_entity(
        &*entity_store,
        "Vue Framework",
        EntityType::Tool,
        Some("Frontend framework"),
    );
    create_test_entity(
        &*entity_store,
        "Django Framework",
        EntityType::Tool,
        Some("Backend framework"),
    );

    // Filter by type AND query
    let req = EntityListRequest {
        entity_type: Some("tool".to_string()),
        query: Some("Frontend".to_string()),
        tags: None,
        scope: None,
        sort: Some("name".to_string()),
        sort_order: Some("asc".to_string()),
        limit: Some(10),
    };
    let result = service
        .inner
        .cas_entity_list(Parameters(req))
        .await
        .unwrap();
    let text = extract_text(result);

    assert!(text.contains("React"), "Should find React (frontend)");
    assert!(text.contains("Vue"), "Should find Vue (frontend)");
    assert!(!text.contains("Django"), "Should NOT find Django (backend)");

    // Verify sort order
    let react_pos = text.find("React").expect("React should exist");
    let vue_pos = text.find("Vue").expect("Vue should exist");
    assert!(
        react_pos < vue_pos,
        "React should come before Vue (alphabetically)"
    );
}
