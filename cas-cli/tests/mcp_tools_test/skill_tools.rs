use crate::support::*;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;

fn git(root: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(root: &std::path::Path, message: &str) {
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=CAS Test",
            "-c",
            "user.email=cas-test@example.invalid",
            "commit",
            "-m",
            message,
        ],
    );
}

#[tokio::test]
async fn test_skill_create() {
    let (_temp, service) = setup_cas();

    let req = SkillCreateRequest {
        scope: "global".to_string(),
        name: "Test Skill".to_string(),
        description: "A skill for testing".to_string(),
        invocation: "test-skill".to_string(),
        skill_type: "command".to_string(),
        tags: Some("test,skill".to_string()),
        summary: None,
        example: None,
        preconditions: None,
        postconditions: None,
        validation_script: None,
        invokable: false,
        argument_hint: None,
        context_mode: None,
        agent_type: None,
        allowed_tools: None,
        disallowed_tools: None,
        draft: false,
        disable_model_invocation: false,
    };

    let result = service
        .cas_skill_create(Parameters(req))
        .await
        .expect("skill_create should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created skill") || text.contains("Test Skill"));
}

#[tokio::test]
async fn test_skill_show() {
    let (_temp, service) = setup_cas();

    // Create skill
    let req = SkillCreateRequest {
        scope: "global".to_string(),
        name: "Show Skill".to_string(),
        description: "A skill for show test".to_string(),
        invocation: "show-skill".to_string(),
        skill_type: "command".to_string(),
        tags: None,
        summary: None,
        example: None,
        preconditions: None,
        postconditions: None,
        validation_script: None,
        invokable: false,
        argument_hint: None,
        context_mode: None,
        agent_type: None,
        allowed_tools: None,
        disallowed_tools: None,
        draft: false,
        disable_model_invocation: false,
    };

    let result = service
        .cas_skill_create(Parameters(req))
        .await
        .expect("skill_create should succeed");

    let text = extract_text(result);
    let id = extract_skill_id(&text).expect("should have skill ID");

    // Show skill
    let show_req = IdRequest { id: id.clone() };
    let result = service
        .cas_skill_show(Parameters(show_req))
        .await
        .expect("skill_show should succeed");

    let text = extract_text(result);
    assert!(text.contains("Show Skill") || text.contains("show-skill"));
}

#[tokio::test]
async fn project_skill_show_marks_a_checkout_behind_its_sync_ref() {
    let (temp, service) = setup_cas();
    let root = temp.path();
    git(root, &["init", "--initial-branch=main"]);
    std::fs::create_dir_all(root.join(".claude/skills/stale-skill")).unwrap();
    std::fs::write(
        root.join(".claude/skills/stale-skill/SKILL.md"),
        "---\nname: stale-skill\ndescription: stale test\n---\n\nOld guidance\n",
    )
    .unwrap();
    commit(root, "add project skill");

    git(root, &["checkout", "-b", "factory/stale-skill"]);
    git(root, &["checkout", "main"]);
    std::fs::write(root.join("new-guidance.md"), "new guidance\n").unwrap();
    commit(root, "new project guidance");
    git(root, &["checkout", "factory/stale-skill"]);

    let result = service
        .cas_skill_show(Parameters(IdRequest {
            id: "stale-skill".to_string(),
        }))
        .await
        .expect("file-backed skill should be served");
    let text = extract_text(result);

    assert!(text.starts_with("WARNING: STALE PROJECT SKILL"), "{text}");
    assert!(text.contains("factory/stale-skill"), "{text}");
    assert!(text.contains("1 commit(s) behind `main`"), "{text}");
}

#[tokio::test]
async fn project_skill_show_is_quiet_when_checkout_is_current() {
    let (temp, service) = setup_cas();
    let root = temp.path();
    git(root, &["init", "--initial-branch=main"]);
    std::fs::create_dir_all(root.join(".claude/skills/current-skill")).unwrap();
    std::fs::write(
        root.join(".claude/skills/current-skill/SKILL.md"),
        "---\nname: current-skill\ndescription: current test\n---\n\nCurrent guidance\n",
    )
    .unwrap();
    commit(root, "add current project skill");

    let result = service
        .cas_skill_show(Parameters(IdRequest {
            id: "current-skill".to_string(),
        }))
        .await
        .expect("file-backed skill should be served");
    let text = extract_text(result);

    assert!(!text.contains("STALE PROJECT SKILL"), "{text}");
    assert!(text.starts_with("Skill: current-skill"), "{text}");
}

#[tokio::test]
async fn test_skill_list() {
    let (_temp, service) = setup_cas();

    // Create skills
    for i in 0..3 {
        let req = SkillCreateRequest {
            scope: "global".to_string(),
            name: format!("List Skill {i}"),
            description: format!("Skill {i} for list test"),
            invocation: format!("list-skill-{i}"),
            skill_type: "command".to_string(),
            tags: None,
            summary: None,
            example: None,
            preconditions: None,
            postconditions: None,
            validation_script: None,
            invokable: false,
            argument_hint: None,
            context_mode: None,
            agent_type: None,
            allowed_tools: None,
            disallowed_tools: None,
            draft: false,
            disable_model_invocation: false,
        };
        service
            .cas_skill_create(Parameters(req))
            .await
            .expect("skill_create should succeed");
    }

    // List all skills
    let list_req = LimitRequest {
        scope: "all".to_string(),
        limit: Some(10),
        sort: None,
        sort_order: None,
        team_id: None,
    };
    let result = service
        .cas_skill_list_all(Parameters(list_req))
        .await
        .expect("skill_list_all should succeed");

    let text = extract_text(result);
    assert!(text.contains("List Skill") || text.contains("Skills"));
}

#[tokio::test]
async fn test_skill_update() {
    let (_temp, service) = setup_cas();

    // Create skill
    let req = SkillCreateRequest {
        scope: "global".to_string(),
        name: "Update Skill".to_string(),
        description: "Original description".to_string(),
        invocation: "update-skill".to_string(),
        skill_type: "command".to_string(),
        tags: None,
        summary: None,
        example: None,
        preconditions: None,
        postconditions: None,
        validation_script: None,
        invokable: false,
        argument_hint: None,
        context_mode: None,
        agent_type: None,
        allowed_tools: None,
        disallowed_tools: None,
        draft: false,
        disable_model_invocation: false,
    };

    let result = service
        .cas_skill_create(Parameters(req))
        .await
        .expect("skill_create should succeed");

    let text = extract_text(result);
    let id = extract_skill_id(&text).expect("should have skill ID");

    // Update skill
    let update_req = SkillUpdateRequest {
        id: id.to_string(),
        name: Some("Updated Skill Name".to_string()),
        description: Some("Updated description".to_string()),
        invocation: None,
        tags: None,
        summary: None,
        disable_model_invocation: None,
    };

    let result = service
        .cas_skill_update(Parameters(update_req))
        .await
        .expect("skill_update should succeed");

    let text = extract_text(result);
    assert!(text.contains("Updated") || text.contains("updated"));
}

#[tokio::test]
async fn test_skill_enable_disable() {
    let (_temp, service) = setup_cas();

    // Create skill
    let req = SkillCreateRequest {
        scope: "global".to_string(),
        name: "Enable Skill".to_string(),
        description: "Skill for enable/disable test".to_string(),
        invocation: "enable-skill".to_string(),
        skill_type: "command".to_string(),
        tags: None,
        summary: None,
        example: None,
        preconditions: None,
        postconditions: None,
        validation_script: None,
        invokable: false,
        argument_hint: None,
        context_mode: None,
        agent_type: None,
        allowed_tools: None,
        disallowed_tools: None,
        draft: false,
        disable_model_invocation: false,
    };

    let result = service
        .cas_skill_create(Parameters(req))
        .await
        .expect("skill_create should succeed");

    let text = extract_text(result);
    let id = extract_skill_id(&text).expect("should have skill ID");

    // Disable skill
    let disable_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_skill_disable(Parameters(disable_req))
        .await
        .expect("skill_disable should succeed");

    let text = extract_text(result);
    assert!(text.contains("Disabled") || text.contains("disabled"));

    // Enable skill
    let enable_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_skill_enable(Parameters(enable_req))
        .await
        .expect("skill_enable should succeed");

    let text = extract_text(result);
    assert!(text.contains("Enabled") || text.contains("enabled"));
}

#[tokio::test]
async fn test_skill_delete() {
    let (_temp, service) = setup_cas();

    // Create skill
    let req = SkillCreateRequest {
        scope: "global".to_string(),
        name: "Delete Skill".to_string(),
        description: "Skill for delete test".to_string(),
        invocation: "delete-skill".to_string(),
        skill_type: "command".to_string(),
        tags: None,
        summary: None,
        example: None,
        preconditions: None,
        postconditions: None,
        validation_script: None,
        invokable: false,
        argument_hint: None,
        context_mode: None,
        agent_type: None,
        allowed_tools: None,
        disallowed_tools: None,
        draft: false,
        disable_model_invocation: false,
    };

    let result = service
        .cas_skill_create(Parameters(req))
        .await
        .expect("skill_create should succeed");

    let text = extract_text(result);
    let id = extract_skill_id(&text).expect("should have skill ID");

    // Delete skill
    let delete_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_skill_delete(Parameters(delete_req))
        .await
        .expect("skill_delete should succeed");

    let text = extract_text(result);
    assert!(text.contains("Deleted"));
}

#[tokio::test]
async fn test_skill_use() {
    let (_temp, service) = setup_cas();

    // Create skill
    let req = SkillCreateRequest {
        scope: "global".to_string(),
        name: "Use Skill".to_string(),
        description: "Skill for use tracking".to_string(),
        invocation: "use-skill".to_string(),
        skill_type: "command".to_string(),
        tags: None,
        summary: None,
        example: None,
        preconditions: None,
        postconditions: None,
        validation_script: None,
        invokable: false,
        argument_hint: None,
        context_mode: None,
        agent_type: None,
        allowed_tools: None,
        disallowed_tools: None,
        draft: false,
        disable_model_invocation: false,
    };

    let result = service
        .cas_skill_create(Parameters(req))
        .await
        .expect("skill_create should succeed");

    let text = extract_text(result);
    let id = extract_skill_id(&text).expect("should have skill ID");

    // Record skill use
    let use_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_skill_use(Parameters(use_req))
        .await
        .expect("skill_use should succeed");

    let text = extract_text(result);
    assert!(text.contains("usage") || text.contains("Used") || text.contains("1"));
}
