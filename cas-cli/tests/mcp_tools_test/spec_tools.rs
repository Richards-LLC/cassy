use crate::support::*;
use cas::mcp::tools::service::SpecRequest;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;
use tempfile::TempDir;

/// Helper to setup CAS with CasService for consolidated tools
fn setup_cas_service(test_env: &mut TestEnvGuard) -> (TempDir, CasService) {
    let (temp, core) = setup_cas(test_env);
    (temp, CasService::new(core, None))
}

/// Create a SpecRequest with only the action set
fn spec_req(action: &str) -> SpecRequest {
    SpecRequest {
        action: action.to_string(),
        id: None,
        title: None,
        summary: None,
        goals: None,
        in_scope: None,
        out_of_scope: None,
        users: None,
        technical_requirements: None,
        acceptance_criteria: None,
        design_notes: None,
        additional_notes: None,
        spec_type: None,
        status: None,
        task_id: None,
        source_ids: None,
        supersedes_id: None,
        new_version: None,
        tags: None,
        scope: None,
        limit: None,
    }
}

/// Extract spec ID from output text
fn extract_spec_id(text: &str) -> Option<String> {
    // Try format with "spec-" prefix
    text.split("spec-")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_alphanumeric()).next())
        .map(|s| format!("spec-{s}"))
}

#[tokio::test]
async fn test_spec_create_basic() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas_service(&mut test_env);

    let mut req = spec_req("create");
    req.title = Some("Test Spec".to_string());
    req.summary = Some("A test specification".to_string());
    req.spec_type = Some("feature".to_string());

    let result = service
        .spec(Parameters(req))
        .await
        .expect("spec create should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("spec-") || text.to_lowercase().contains("created"),
        "Should create spec: {text}"
    );
}

#[tokio::test]
async fn test_spec_create_with_fields() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas_service(&mut test_env);

    let mut req = spec_req("create");
    req.title = Some("Full Spec".to_string());
    req.summary = Some("Comprehensive test spec".to_string());
    req.goals = Some("Goal 1,Goal 2".to_string());
    req.acceptance_criteria = Some("Tests pass".to_string());
    req.spec_type = Some("epic".to_string());
    req.tags = Some("important".to_string());

    let result = service
        .spec(Parameters(req))
        .await
        .expect("spec create should succeed");

    let text = extract_text(result);
    assert!(
        extract_spec_id(&text).is_some() || text.to_lowercase().contains("created"),
        "Should create spec: {text}"
    );
}

#[tokio::test]
async fn test_spec_show() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas_service(&mut test_env);

    // Create a spec
    let mut create_req = spec_req("create");
    create_req.title = Some("Show Test Spec".to_string());
    create_req.summary = Some("Spec for show test".to_string());
    create_req.spec_type = Some("feature".to_string());

    let result = service
        .spec(Parameters(create_req))
        .await
        .expect("spec create should succeed");

    let text = extract_text(result);
    let spec_id = extract_spec_id(&text).expect("should have spec ID");

    // Show the spec
    let mut show_req = spec_req("show");
    show_req.id = Some(spec_id.clone());

    let result = service
        .spec(Parameters(show_req))
        .await
        .expect("spec show should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("Show Test Spec") || text.contains(&spec_id),
        "Should show spec: {text}"
    );
}

#[tokio::test]
async fn test_spec_list() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas_service(&mut test_env);

    // Create specs
    for i in 1..=2 {
        let mut req = spec_req("create");
        req.title = Some(format!("List Spec {i}"));
        req.summary = Some(format!("Spec {i} for list"));
        req.spec_type = Some("feature".to_string());

        service
            .spec(Parameters(req))
            .await
            .expect("create should succeed");
    }

    // List all
    let list_req = spec_req("list");

    let result = service
        .spec(Parameters(list_req))
        .await
        .expect("spec list should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("spec-") || text.contains("List Spec"),
        "Should list specs: {text}"
    );
}

#[tokio::test]
async fn test_spec_update() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas_service(&mut test_env);

    // Create
    let mut create_req = spec_req("create");
    create_req.title = Some("Original Title".to_string());
    create_req.summary = Some("Original summary".to_string());
    create_req.spec_type = Some("feature".to_string());

    let result = service.spec(Parameters(create_req)).await.expect("create");
    let spec_id = extract_spec_id(&extract_text(result)).expect("spec ID");

    // Update
    let mut update_req = spec_req("update");
    update_req.id = Some(spec_id.clone());
    update_req.title = Some("Updated Title".to_string());

    let result = service.spec(Parameters(update_req)).await.expect("update");
    let text = extract_text(result);
    assert!(
        text.to_lowercase().contains("updat") || text.contains(&spec_id),
        "Should update: {text}"
    );
}

#[tokio::test]
async fn test_spec_approve() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas_service(&mut test_env);

    // Create
    let mut create_req = spec_req("create");
    create_req.title = Some("Approve Test".to_string());
    create_req.spec_type = Some("feature".to_string());

    let result = service.spec(Parameters(create_req)).await.expect("create");
    let spec_id = extract_spec_id(&extract_text(result)).expect("spec ID");

    // Approve
    let mut approve_req = spec_req("approve");
    approve_req.id = Some(spec_id.clone());

    let result = service
        .spec(Parameters(approve_req))
        .await
        .expect("approve");
    let text = extract_text(result);
    assert!(
        text.to_lowercase().contains("approv") || text.contains(&spec_id),
        "Should approve: {text}"
    );
}

#[tokio::test]
async fn test_spec_delete() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas_service(&mut test_env);

    // Create
    let mut create_req = spec_req("create");
    create_req.title = Some("Delete Test".to_string());
    create_req.spec_type = Some("feature".to_string());

    let result = service.spec(Parameters(create_req)).await.expect("create");
    let spec_id = extract_spec_id(&extract_text(result)).expect("spec ID");

    // Delete
    let mut delete_req = spec_req("delete");
    delete_req.id = Some(spec_id.clone());

    let result = service.spec(Parameters(delete_req)).await.expect("delete");
    let text = extract_text(result);
    assert!(
        text.to_lowercase().contains("delet") || text.contains(&spec_id),
        "Should delete: {text}"
    );
}

#[tokio::test]
async fn test_spec_types() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas_service(&mut test_env);

    for spec_type in ["epic", "feature", "api", "component", "migration"] {
        let mut req = spec_req("create");
        req.title = Some(format!("{spec_type} Spec"));
        req.spec_type = Some(spec_type.to_string());

        let result = service.spec(Parameters(req)).await;
        assert!(result.is_ok(), "{spec_type} spec should create");
    }
}
