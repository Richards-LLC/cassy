//! Account mutations through the paired-device router, on disposable profiles.
use super::*;
use serde_json::{Value, json};

async fn operation(kind: &str) {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set_current_dir(&home);
    let folder = home.join(".claude-disposable");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"test@example.test"}}"#,
    )
    .unwrap();
    let base = home.join(".cas/claude-base");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("settings.json"), "{}").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(base.join("settings.json"), folder.join("settings.json")).unwrap();
    let mut request = json!({"operation":kind,"account":"claude:disposable","confirmation":"claude:disposable","new_name":"renamed"});
    // The same real operation is refused without the management grant.
    let read = ops_fixture([Scope::MachineRead].into());
    let (status, body) = read
        .call("POST", "/v1/accounts/operations", Some(request.clone()))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(folder.is_dir());
    let fixture = ops_fixture([Scope::MachineRead, Scope::FactoryManage].into());
    if kind == "remove" || kind == "disable" {
        request["confirmation"] = json!("wrong account");
        assert_eq!(
            fixture
                .call("POST", "/v1/accounts/operations", Some(request.clone()))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert!(folder.is_dir());
        request["confirmation"] = json!("claude:disposable");
    }
    if kind == "enable" {
        crate::account_lifecycle::set_disabled(&home, &folder, true).unwrap();
    }
    let (status, body) = fixture
        .call("POST", "/v1/accounts/operations", Some(request))
        .await;
    if cfg!(target_os = "macos") && kind == "rename" {
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "Claude path-keyed login stays protected: {body}"
        );
        return;
    }
    assert_eq!(status, StatusCode::OK, "{body}");
    match kind {
        "remove" => {
            assert!(!folder.exists());
            assert!(base.join("settings.json").is_file());
        }
        "rename" => {
            assert!(!folder.exists());
            assert!(home.join(".claude-renamed").is_dir());
        }
        "disable" => assert!(crate::account_lifecycle::disabled(&home, &folder)),
        "enable" => assert!(!crate::account_lifecycle::disabled(&home, &folder)),
        "set_default" => assert_eq!(
            super::super::launch_env::default_profile_name(cas_mux::SupervisorCli::Claude).unwrap(),
            "disposable"
        ),
        _ => unreachable!(),
    }
    let audit = fixture.audit(&format!("accounts:{kind}"));
    assert!(
        audit.iter().any(|row| row["outcome"] == "requested"),
        "{audit:?}"
    );
    assert_eq!(audit.last().unwrap()["outcome"], "allowed");
    let (status, listed) = read.call("GET", "/v1/accounts", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "read-only pairing can list: {listed}"
    );
    let serialized = listed.to_string();
    assert!(
        !serialized.contains(home.to_str().unwrap()),
        "no filesystem paths cross the wire"
    );
    if kind == "remove" {
        assert!(!serialized.contains("test@example.test"));
        let (status, _) = fixture
            .call(
                "POST",
                "/v1/accounts/operations",
                Some(json!({"operation":"shell","account":"claude:disposable"})),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    if kind == "rename" {
        assert!(serialized.contains("claude:renamed"));
    }
    let _: Value = body;
}

#[tokio::test]
async fn remove_requires_management_and_confirmation_preserves_shared_base() {
    operation("remove").await;
}
#[tokio::test]
async fn disable_is_confirmed_and_audited() {
    operation("disable").await;
}
#[tokio::test]
async fn enable_restores_selection_and_audits() {
    operation("enable").await;
}
#[tokio::test]
async fn rename_moves_only_the_resolved_profile() {
    operation("rename").await;
}
#[tokio::test]
async fn set_default_persists_launch_selection() {
    operation("set_default").await;
}
