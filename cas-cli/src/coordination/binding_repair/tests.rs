use super::*;
use crate::coordination::binding_intake::ChannelDisposition;
use std::io::{Read, Write};
const PROJECT: &str = "github.com/example/project";

fn project(root: &Path, team: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("config.toml"),
        format!("[project]\ncanonical_id = {PROJECT:?}\n"),
    )
    .unwrap();
    crate::cloud::CloudConfig {
        team_id: Some(team.into()),
        ..Default::default()
    }
    .save_to_cas_dir(root)
    .unwrap();
}
fn scope() -> IntakeScope {
    IntakeScope {
        team_id: Uuid::new_v4().to_string(),
        canonical_project_id: PROJECT.into(),
        channel_id: "C0123456789".into(),
    }
}
fn runtime(root: &Path, capability: bool) -> std::thread::JoinHandle<()> {
    use crate::ui::factory::daemon::runtime::violet_intake::*;
    use crate::ui::factory::{protocol::*, session::*};
    let name = "binding-fixture";
    let mut metadata = create_metadata(
        name,
        std::process::id(),
        "supervisor",
        &[],
        None,
        None,
        None,
    );
    metadata.project_dir = Some(root.parent().unwrap().to_str().unwrap().into());
    SessionManager::new().save_metadata(&metadata).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(gui_socket_path(name)).unwrap();
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let welcome = serde_json::json!({ "Welcome": {"session_name":name,"state":{"focused_pane":null,"panes":[],"epic_id":null,"epic_title":null,"cols":80,"rows":24},"scrollback":null,"protocol_version":3,"capabilities": if capability { vec!["binding_intake_control_v1"] } else { vec![] } }});
        stream
            .write_all(&encode_message(&welcome).unwrap())
            .unwrap();
        if !capability {
            return;
        }
        let mut header = [0; 4];
        stream.read_exact(&mut header).unwrap();
        let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
        stream.read_exact(&mut bytes).unwrap();
        let ClientMessage::BindingIntakePause { request } = serde_json::from_slice(&bytes).unwrap()
        else {
            panic!("local pause required")
        };
        let state = IntakeJournal::new(&root).read().unwrap();
        let pause = state
            .pauses
            .iter()
            .find(|pause| pause.operation_id == request.operation_id)
            .unwrap()
            .clone();
        let reply = IntakeReply {
            nonce: request.nonce,
            state: Some(state),
            acknowledgement: Some(DrainAcknowledgement {
                nonce: request.nonce,
                pause,
                runtime: RuntimeIdentity::current(&root, name).unwrap(),
            }),
            error: None,
        };
        stream
            .write_all(&encode_message(&DaemonMessage::BindingIntake { reply }).unwrap())
            .unwrap();
    })
}

#[test]
fn cas_9021_ordinary_http_invocation_is_bound_to_local_drain_and_server_receipt() {
    let env = crate::test_support::TestEnvGuard::temp_home();
    let scope = scope();
    let root = env.home().join("project/.cas");
    project(&root, &scope.team_id);
    let gui = runtime(&root, true);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", server.server_addr());
    let target = scope.clone();
    let remote = std::thread::spawn(move || {
        let get = server
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("ordinary project verification");
        assert_eq!(get.method().as_str(), "GET");
        assert_eq!(get.url(), format!("/api/teams/{}/projects", target.team_id));
        get.respond(tiny_http::Response::from_string(
            serde_json::json!({"projects":[{"id":"project-id","canonical_id":PROJECT}]})
                .to_string(),
        ))
        .unwrap();
        let Some(mut post) = server.recv_timeout(Duration::from_secs(5)).unwrap() else {
            return false;
        };
        assert_eq!(post.method().as_str(), "POST");
        assert_eq!(post.url(), RECONCILE_PATH);
        assert_eq!(
            post.headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .unwrap()
                .value
                .as_str(),
            "Bearer ordinary-fixture-user"
        );
        let mut bytes = Vec::new();
        post.as_reader().read_to_end(&mut bytes).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"schema":super::super::binding_reconciliation::SCHEMA,"team_id":target.team_id,"project_id":PROJECT,"channel_id":target.channel_id})
        );
        let mut body: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/binding_reconciliation/v1/completed.json"
        ))
        .unwrap();
        body["scope"]["team_id"] = target.team_id.clone().into();
        body["binding"]["team_id"] = target.team_id.into();
        post.respond(
            tiny_http::Response::from_string(body.to_string())
                .with_header(tiny_http::Header::from_bytes("Cache-Control", "no-store").unwrap()),
        )
        .unwrap();
        true
    });
    let report = OrdinaryBindingClient::new(&endpoint, "ordinary-fixture-user")
        .unwrap()
        .repair(&root, &scope)
        .unwrap();
    gui.join().unwrap();
    let posted = remote.join().unwrap();
    assert!(
        posted,
        "reconciliation must reach the ordinary HTTP path after actual local drain"
    );
    assert!(
        matches!(report.outcome, RepairOutcome::Managed),
        "{report:?}"
    );
    let state = IntakeJournal::new(&root).read().unwrap();
    assert_eq!(state.pauses[0].operation_id, report.operation_id);
    assert!(
        matches!(&state.policies[0].disposition,ChannelDisposition::Managed { completion } if completion.latest_attempt.as_ref().unwrap().id == report.activation_id.as_ref().unwrap().as_str())
    );
}

#[test]
fn cas_9021_old_or_unknown_actual_runtime_preflight_sends_zero_post() {
    let env = crate::test_support::TestEnvGuard::temp_home();
    let scope = scope();
    let root = env.home().join("project/.cas");
    project(&root, &scope.team_id);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client = OrdinaryBindingClient::new(
        &format!("http://{}", server.server_addr()),
        "ordinary-fixture-user",
    )
    .unwrap();
    let gui = runtime(&root, false);
    let old = client.repair(&root, &scope).unwrap();
    gui.join().unwrap();
    assert!(matches!(old.outcome, RepairOutcome::Blocked));
    assert!(old.error.unwrap().contains("lacks"));
    let metadata_path = crate::ui::factory::session::metadata_path("binding-fixture");
    let mut metadata: crate::ui::factory::protocol::SessionMetadata =
        serde_json::from_slice(&std::fs::read(&metadata_path).unwrap()).unwrap();
    metadata.daemon_build_hash = None;
    crate::ui::factory::session::SessionManager::new()
        .save_metadata(&metadata)
        .unwrap();
    let unknown = client.repair(&root, &scope).unwrap();
    assert!(matches!(unknown.outcome, RepairOutcome::Blocked));
    assert!(unknown.error.unwrap().contains("build"));
    assert!(
        server.try_recv().unwrap().is_none(),
        "no GET/POST may escape unsafe actual-runtime preflight"
    );
    assert!(!IntakeJournal::new(&root).legacy_allowed().unwrap());
}
