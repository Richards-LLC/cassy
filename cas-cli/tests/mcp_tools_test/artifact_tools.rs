//! `mcp__cas__artifact` handler tests (cassy#910, task cas-b72a).
//!
//! The MCP surface is the one every harness actually uses, so these assert the
//! contract a Claude/Codex/Grok worker depends on: one call with a local path,
//! a citable `artifact_id` back, and a refusal that says what to fix.

use crate::support::*;
use cas::mcp::tools::service::ArtifactRequest;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;
use tempfile::TempDir;

fn req(action: &str) -> ArtifactRequest {
    ArtifactRequest {
        action: action.to_string(),
        task_id: None,
        path: None,
        id: None,
    }
}

fn setup_cas_service() -> (TempDir, CasService) {
    let (temp, core) = setup_cas();
    (temp, CasService::new(core, None))
}

/// Publishable file inside the project checkout, which is one of the two
/// permitted roots and needs no factory configuration.
fn write_in_checkout(temp: &TempDir, name: &str, bytes: &[u8]) -> String {
    let dir = temp.path().join("docs");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.display().to_string()
}

#[tokio::test]
async fn publish_returns_a_citable_id_digest_and_size() {
    let (temp, service) = setup_cas_service();
    let path = write_in_checkout(&temp, "brief.pdf", b"hello world");

    let mut request = req("publish");
    request.task_id = Some("cas-b72a".to_string());
    request.path = Some(path);

    let text = extract_text(
        service
            .artifact(Parameters(request))
            .await
            .expect("publish should succeed"),
    );

    assert!(text.contains("artifact_id: art-"), "{text}");
    assert!(
        text.contains("sha256: b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"),
        "the digest of the real bytes must be reported: {text}"
    );
    assert!(text.contains("size_bytes: 11"), "{text}");
    assert!(text.contains("mime: application/pdf"), "{text}");
    assert!(
        text.contains("status: local"),
        "with no cloud credentials the record stays local: {text}"
    );
}

#[tokio::test]
async fn a_published_artifact_is_readable_through_show_and_list() {
    let (temp, service) = setup_cas_service();
    let path = write_in_checkout(&temp, "report.html", b"<html>report</html>");

    let mut publish = req("publish");
    publish.task_id = Some("cas-b72a".to_string());
    publish.path = Some(path);
    let published = extract_text(service.artifact(Parameters(publish)).await.unwrap());
    let id = published
        .lines()
        .find_map(|line| line.strip_prefix("artifact_id: "))
        .expect("publish must state the id")
        .to_string();

    let mut show = req("show");
    show.id = Some(id.clone());
    let shown = extract_text(service.artifact(Parameters(show)).await.unwrap());
    assert!(shown.contains("report.html"), "{shown}");
    assert!(shown.contains("mime: text/html"), "{shown}");
    assert!(shown.contains("task_id: cas-b72a"), "{shown}");

    let mut list = req("list");
    list.task_id = Some("cas-b72a".to_string());
    let listed = extract_text(service.artifact(Parameters(list)).await.unwrap());
    assert!(listed.contains(&id), "{listed}");
    assert!(listed.contains("report.html"), "{listed}");
}

#[tokio::test]
async fn listing_a_task_with_no_artifacts_is_a_plain_answer_not_an_error() {
    let (_temp, service) = setup_cas_service();
    let mut list = req("list");
    list.task_id = Some("cas-nothing".to_string());
    let text = extract_text(service.artifact(Parameters(list)).await.unwrap());
    assert!(
        text.contains("No artifacts published for cas-nothing"),
        "{text}"
    );
}

#[tokio::test]
async fn a_path_outside_the_publishable_roots_is_refused_with_a_usable_message() {
    let (_temp, service) = setup_cas_service();
    let outside = TempDir::new().unwrap();
    let stray = outside.path().join("stray.pdf");
    std::fs::write(&stray, b"not yours").unwrap();

    let mut request = req("publish");
    request.task_id = Some("cas-b72a".to_string());
    request.path = Some(stray.display().to_string());

    let error = service
        .artifact(Parameters(request))
        .await
        .expect_err("a path outside both roots must be refused");
    assert!(
        error
            .message
            .contains("outside this task's publishable roots"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("Publish from"),
        "the refusal must name where publishing IS allowed: {}",
        error.message
    );
}

#[tokio::test]
async fn a_missing_file_is_refused_rather_than_recorded() {
    let (temp, service) = setup_cas_service();
    let mut request = req("publish");
    request.task_id = Some("cas-b72a".to_string());
    request.path = Some(temp.path().join("absent.pdf").display().to_string());

    let error = service.artifact(Parameters(request)).await.unwrap_err();
    assert!(error.message.contains("no file at"), "{}", error.message);

    let mut list = req("list");
    list.task_id = Some("cas-b72a".to_string());
    let listed = extract_text(service.artifact(Parameters(list)).await.unwrap());
    assert!(
        listed.contains("No artifacts"),
        "no row may be left: {listed}"
    );
}

#[tokio::test]
async fn required_fields_are_named_when_missing() {
    let (_temp, service) = setup_cas_service();

    let error = service
        .artifact(Parameters(req("publish")))
        .await
        .unwrap_err();
    assert!(error.message.contains("task_id"), "{}", error.message);

    let mut without_path = req("publish");
    without_path.task_id = Some("cas-b72a".to_string());
    let error = service
        .artifact(Parameters(without_path))
        .await
        .unwrap_err();
    assert!(error.message.contains("path"), "{}", error.message);

    let error = service.artifact(Parameters(req("show"))).await.unwrap_err();
    assert!(error.message.contains("id"), "{}", error.message);

    let error = service.artifact(Parameters(req("list"))).await.unwrap_err();
    assert!(error.message.contains("task_id"), "{}", error.message);
}

#[tokio::test]
async fn an_unknown_action_lists_the_valid_ones() {
    let (_temp, service) = setup_cas_service();
    let error = service
        .artifact(Parameters(req("upload")))
        .await
        .unwrap_err();
    assert!(
        error.message.contains("publish")
            && error.message.contains("show")
            && error.message.contains("list"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn showing_an_unknown_artifact_names_the_id() {
    let (_temp, service) = setup_cas_service();
    let mut show = req("show");
    show.id = Some("art-nope".to_string());
    let error = service.artifact(Parameters(show)).await.unwrap_err();
    assert!(error.message.contains("art-nope"), "{}", error.message);
}

#[tokio::test]
async fn the_credential_cache_is_not_publishable_through_the_tool_either() {
    let (temp, service) = setup_cas_service();
    let cloud_json = temp.path().join(".cas").join("cloud.json");
    std::fs::create_dir_all(cloud_json.parent().unwrap()).unwrap();
    std::fs::write(&cloud_json, "{\"token\":\"real-secret\"}").unwrap();

    let mut request = req("publish");
    request.task_id = Some("cas-b72a".to_string());
    request.path = Some(cloud_json.display().to_string());

    let error = service.artifact(Parameters(request)).await.unwrap_err();
    assert!(
        error.message.contains("never publishable"),
        "{}",
        error.message
    );
    assert!(
        !error.message.contains("real-secret"),
        "a refusal must not echo the file's contents: {}",
        error.message
    );
}
