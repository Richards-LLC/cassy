use super::*;
use crate::coordination::binding_intake::ChannelDisposition;
use std::time::Duration;

const PROJECT: &str = "qa.invalid/v1-receipts";
const MANAGED: &str = "C0QARECEIPT";
const FOREIGN: &str = "C0FOREIGN";
const UNMAPPED: &str = "C0UNMAPPED";

/// Cloud's approved r1 example, retargeted to this fixture's team.
fn receipt_case(index: usize, team: &str) -> String {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/binding_reconciliation/v1/cloud-receipt-identity-r1.json"
    ))
    .unwrap();
    fixture["cases"][index]["response"]
        .to_string()
        .replace("qa-v1-receipt-team", team)
}

/// A definite refusal: Cloud found the channel mapped to another project.
fn foreign_refusal() -> String {
    serde_json::json!({
        "schema": "cassy.binding-reconciliation/v1",
        "code": "mapping_conflict", "reason": "mapping_project_mismatch",
        "guidance": "This channel is mapped to another project.",
        "scope": null, "binding": null, "strict_activation": null, "activation_id": null,
        "fence_receipt": null, "latest_attempt": null, "settled": null, "blockers": null,
        "retry_after_secs": null, "next_action": "explicit_relink"
    })
    .to_string()
}

fn unmapped_refusal() -> String {
    let mut value: serde_json::Value = serde_json::from_str(&foreign_refusal()).unwrap();
    value["code"] = "mapping_not_found".into();
    value["reason"] = "channel_unmapped".into();
    value["next_action"] = "operator_repair".into();
    value.to_string()
}

/// Violet lists two mappings for this project (and one for another project);
/// Cloud refuses the foreign one and manages the other. A second update is
/// idempotent: the managed channel keeps its original fence evidence.
#[test]
fn cas_9021_update_reconciles_violet_mappings_and_names_blockers() {
    let env = crate::test_support::TestEnvGuard::temp_home();
    let root = env.home().join("project/.cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), format!("[project]\ncanonical_id = {PROJECT:?}\n[slack]\nchannels = [{UNMAPPED:?}]\n")).unwrap();
    let team = uuid::Uuid::new_v4().to_string();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", server.server_addr());
    let config = CloudConfig {
        endpoint: endpoint.clone(),
        token: Some("ordinary-fixture-user".into()),
        team_id: Some(team.clone()),
        ..Default::default()
    };
    config.save_to_cas_dir(&root).unwrap();

    let serving = team.clone();
    let remote = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for managed_case in [0, 1] {
            let mut answers = vec![
                serde_json::json!({"channels": [
                    {"channel_id": MANAGED, "channel_name": "receipts", "project_id": PROJECT},
                    {"channel_id": FOREIGN, "channel_name": "foreign", "project_id": PROJECT},
                    {"channel_id": "C0ELSEWHERE", "channel_name": "other", "project_id": "other/project"},
                ]})
                .to_string(),
            ];
            for body in [foreign_refusal(), receipt_case(managed_case, &serving), unmapped_refusal()] {
                answers.push(serde_json::json!({"projects": [{"id": "project-id", "canonical_id": PROJECT, "name": "fixture", "contributor_count": 0, "memory_count": 0}]}).to_string());
                answers.push(body);
            }
            for answer in answers {
                let request = server.recv_timeout(Duration::from_secs(10)).unwrap().expect("request");
                seen.push(format!("{} {}", request.method(), request.url()));
                let status = if answer.contains("mapping_project_mismatch") || answer.contains("channel_unmapped") { 409 } else { 200 };
                request
                    .respond(
                        tiny_http::Response::from_string(answer)
                            .with_status_code(status)
                            .with_header(tiny_http::Header::from_bytes("Cache-Control", "no-store").unwrap()),
                    )
                    .unwrap();
            }
        }
        assert!(server.try_recv().unwrap().is_none(), "no other channel is touched");
        seen
    });

    let run = || reconcile_with(&root, &config, &format!("{endpoint}/mcp/slack"), false, true, &RuntimeSnapshot::capture(false)).unwrap();
    for _ in 0..2 {
        let ProjectPhase::Warning(detail) = run() else { panic!("a refused channel is a warning") };
        assert!(detail.starts_with(&format!("Slack {FOREIGN} blocked: ")), "{detail}");
        assert!(
            detail.ends_with(&format!("; resolve the mapping with `cas integrate violet --channel {FOREIGN}`, then rerun `cas update`")),
            "{detail}"
        );
    }
    let seen = remote.join().unwrap();
    assert_eq!(seen[0], format!("GET /api/channels?project_id={}", PROJECT.replace('/', "%2F")));
    assert_eq!(seen.iter().filter(|line| line.starts_with("POST ")).count(), 6);

    let state = IntakeJournal::new(&root).read().unwrap();
    let disposition = |channel: &str| {
        state.policies.iter().find(|p| p.scope.channel_id == channel).unwrap().disposition.clone()
    };
    assert_eq!(disposition(FOREIGN), ChannelDisposition::LegacyAllowed, "a definite refusal leaves legacy intake as it was");
    assert_eq!(disposition(UNMAPPED), ChannelDisposition::LegacyAllowed, "a configured channel missing from Violet receives an authoritative refusal");
    let ChannelDisposition::Managed { completion, .. } = disposition(MANAGED) else { panic!("managed") };
    assert_eq!(completion.activation_id.as_deref(), Some("22222222-2222-4222-8222-222222222222"));
    assert_eq!(
        completion.fence_receipt.as_ref().map(|fence| fence.id.as_str()),
        Some("11111111-1111-4111-8111-111111111111"),
        "the repeat keeps the original fence"
    );
    assert_eq!(state.pauses.len(), 3, "a rerun reuses each channel's pause");
}

#[test]
fn update_inventory_probes_each_socket_once_cas_9299() {
    use crate::ui::factory::{SessionManager, gui_socket_path};
    let env = crate::test_support::TestEnvGuard::temp_home();
    let manager = SessionManager::new();
    let runtime_root = env.home().join("runtime/.cas");
    std::fs::create_dir_all(&runtime_root).unwrap();
    std::fs::write(runtime_root.join("config.toml"), "[project]\n").unwrap();
    let mut metadata = crate::ui::factory::create_metadata(
        "active9299",
        std::process::id(),
        "supervisor",
        &[],
        None,
        runtime_root.parent().unwrap().to_str(),
        None,
    );
    manager.save_metadata(&metadata).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(gui_socket_path("probe9299")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", server.server_addr());
    let config = CloudConfig {
        endpoint: endpoint.clone(),
        token: Some("ordinary-fixture-user".into()),
        team_id: Some(uuid::Uuid::new_v4().to_string()),
        ..Default::default()
    };
    let remote = std::thread::spawn(move || {
        for _ in 0..2 {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            assert!(request.url().starts_with("/api/channels?"));
            request
                .respond(tiny_http::Response::from_string(
                    serde_json::json!({
                        "channels": ([MANAGED, FOREIGN].map(|id| serde_json::json!({
                            "channel_id": id, "project_id": PROJECT
                        })))
                    })
                    .to_string(),
                ))
                .unwrap();
        }
        assert!(
            server.try_recv().unwrap().is_none(),
            "held runtime prevents Cloud submission"
        );
    });
    let inventory = RuntimeSnapshot::capture(false);
    for index in 0..4 {
        metadata.epic_id = Some(format!("fixture-epic-{index}"));
        metadata.last_supervisor_mcp_call_at = Some(chrono::Utc::now());
        metadata.held_workers = vec![format!("worker-{index}")];
        manager.save_metadata(&metadata).unwrap();
    }
    let after_metadata = inventory.validate();
    for _ in 0..4 {
        manager.list_sessions().unwrap();
    }
    let after_cleanup = inventory.validate();
    assert!(
        after_metadata.is_ok() && after_cleanup.is_ok(),
        "routine writes and no-op cleanup must preserve inventory: metadata={after_metadata:?}, cleanup={after_cleanup:?}"
    );
    for name in ["first", "second"] {
        let root = env.home().join(name).join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            format!("[project]\ncanonical_id = {PROJECT:?}\n"),
        )
        .unwrap();
        config.save_to_cas_dir(&root).unwrap();
        let result = reconcile_with(
            &root,
            &config,
            &format!("{endpoint}/mcp/slack"),
            false,
            true,
            &inventory,
        )
        .unwrap();
        assert!(
            matches!(result, ProjectPhase::Warning(_)),
            "held endpoint must refuse"
        );
    }
    remote.join().unwrap();
    let mut probes = 0;
    while let Ok((stream, _)) = listener.accept() {
        probes += 1;
        drop(stream);
    }
    assert_eq!(
        probes, 1,
        "one shared update inventory across projects and channels"
    );
    SessionManager::new()
        .save_metadata(&crate::ui::factory::create_metadata(
            "new9299",
            std::process::id(),
            "supervisor",
            &[],
            None,
            None,
            None,
        ))
        .unwrap();
    assert!(
        inventory.validate().is_err(),
        "a new runtime invalidates cached inventory"
    );
    manager.remove_metadata("new9299").unwrap();
    inventory
        .validate()
        .expect("restored session identities remain valid");
    let before_removal = RuntimeSnapshot::capture(false);
    manager.remove_metadata(&metadata.name).unwrap();
    assert!(
        before_removal.validate().is_err(),
        "removing a session must invalidate inventory"
    );
    manager.save_metadata(&metadata).unwrap();
    // Each identity component must fence replacement, independently of mutable
    // progress fields and of whether the process is currently alive.
    type Replacement = (&'static str, fn(&mut crate::ui::factory::SessionMetadata));
    let replacements: [Replacement; 7] = [
        ("name", |m| m.name.push_str("-replacement")),
        ("created_at", |m| m.created_at.push_str("-replacement")),
        ("pid", |m| m.daemon_pid += 1),
        ("process start", |m| m.daemon_pid_starttime = Some(u64::MAX)),
        ("build", |m| {
            m.daemon_build_hash = Some("replacement".into())
        }),
        ("project", |m| m.project_dir = Some("/replacement".into())),
        ("socket", |m| m.socket_path.push_str("-replacement")),
    ];
    for (label, replace) in replacements {
        let inventory = RuntimeSnapshot::capture(false);
        let mut replacement = metadata.clone();
        replace(&mut replacement);
        manager.save_metadata(&replacement).unwrap();
        assert!(
            inventory.validate().is_err(),
            "{label} replacement must invalidate inventory"
        );
        if replacement.name != metadata.name {
            manager.remove_metadata(&replacement.name).unwrap();
        }
        manager.save_metadata(&metadata).unwrap();
    }
    let before_unlink = RuntimeSnapshot::capture(false);
    drop(listener);
    manager.list_sessions().unwrap();
    assert!(
        !gui_socket_path("probe9299").exists(),
        "unheld orphan is reclaimed"
    );
    assert!(
        before_unlink.validate().is_err(),
        "actual socket unlink invalidates inventory"
    );
}
