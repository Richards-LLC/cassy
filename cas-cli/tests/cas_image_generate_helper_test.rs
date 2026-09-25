//! Offline behavior contracts for the cas-image-generate shell helper.

use assert_cmd::Command;
use base64::Engine;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[cfg(unix)]

fn repo_root() -> PathBuf {
    cas::test_paths::workspace_root()
}

#[cfg(unix)]
#[test]
fn helper_streams_large_reference_and_honors_returned_mime() {
    let project = TempDir::new().expect("temporary image-generation project");
    let reference = project.path().join("reference.png");
    let reference_bytes: Vec<u8> = (0..256 * 1024).map(|index| (index % 251) as u8).collect();
    fs::write(&reference, &reference_bytes).expect("write large reference fixture");
    assert!(
        fs::metadata(&reference)
            .expect("stat reference fixture")
            .len()
            > 200_000,
        "reference fixture must exercise the large-file path"
    );

    let bin = project.path().join("bin");
    fs::create_dir(&bin).expect("create fake provider bin");
    let payload_capture = project.path().join("payload.json");
    let fake_curl = bin.join("curl");
    cas::test_paths::warm_stub(
        &fake_curl,
        r##"#!/usr/bin/env bash
set -euo pipefail
payload=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --data-binary)
            payload="${2#@}"
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done
[[ -n "$payload" ]] || { echo "fake curl did not receive --data-binary" >&2; exit 1; }
cp "$payload" "$CURL_PAYLOAD_CAPTURE"
printf '%s' '{"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/jpeg","data":"anBlZy1maXh0dXJl"}}]}}]}'
"##,
    );

    let script = repo_root()
        .join("cas-cli/src/builtins/skills/cas-image-generate/scripts/generate-image.sh");
    let requested_output = project.path().join("generated.png");
    let actual_output = project.path().join("generated.jpg");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    Command::new("bash")
        .arg(&script)
        .args([
            "--prompt",
            "preserve the supplied reference",
            "--output",
            requested_output.to_str().expect("requested output path"),
            "--reference",
            reference.to_str().expect("reference path"),
        ])
        .env("GEMINI_API_KEY", "offline-test-key")
        .env("CURL_PAYLOAD_CAPTURE", &payload_capture)
        .env("PATH", path)
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "wrote={}",
            actual_output.display()
        )))
        .stderr(predicates::str::contains("MIME mismatch"));

    assert!(
        !requested_output.exists(),
        "mismatched requested extension must not lie"
    );
    assert_eq!(
        fs::read(&actual_output).expect("read generated JPEG fixture"),
        b"jpeg-fixture"
    );

    let payload: Value =
        serde_json::from_slice(&fs::read(&payload_capture).expect("read captured request payload"))
            .expect("captured request is JSON");
    let parts = payload["contents"][0]["parts"]
        .as_array()
        .expect("request has a parts array");
    assert_eq!(parts.len(), 2, "prompt and one reference must be sent");
    assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
    assert_eq!(
        parts[1]["inlineData"]["data"],
        base64::engine::general_purpose::STANDARD.encode(reference_bytes)
    );
}

/// Audit L4 F8: the asset playbook asks for 16:9, 2K, A4 and OG sizes, so the
/// helper must be able to request them instead of hoping the prompt is obeyed.
#[cfg(unix)]
#[test]
fn helper_sends_aspect_and_size_as_image_config_only_when_asked() {
    let project = TempDir::new().expect("temporary image-generation project");
    let bin = project.path().join("bin");
    fs::create_dir(&bin).expect("create fake provider bin");
    cas::test_paths::warm_stub(
        &bin.join("curl"),
        r##"#!/usr/bin/env bash
set -euo pipefail
payload=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --data-binary) payload="${2#@}"; shift 2 ;;
        *) shift ;;
    esac
done
cp "$payload" "$CURL_PAYLOAD_CAPTURE"
printf '%s' '{"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":"cG5n"}}]}}]}'
"##,
    );
    let script = repo_root()
        .join("cas-cli/src/builtins/skills/cas-image-generate/scripts/generate-image.sh");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let run = |extra: &[&str], capture: &std::path::Path| {
        let output = project.path().join("hero.png");
        Command::new("bash")
            .arg(&script)
            .args(["--prompt", "hero", "--output", output.to_str().unwrap()])
            .args(extra)
            .env("GEMINI_API_KEY", "offline-test-key")
            .env("CURL_PAYLOAD_CAPTURE", capture)
            .env("PATH", &path)
            .assert()
            .success();
        serde_json::from_slice::<Value>(&fs::read(capture).expect("captured payload"))
            .expect("payload is JSON")
    };

    let sized = run(
        &["--aspect", "16:9", "--size", "2K"],
        &project.path().join("sized.json"),
    );
    assert_eq!(sized["generationConfig"]["responseModalities"][0], "IMAGE");
    assert_eq!(sized["generationConfig"]["imageConfig"]["aspectRatio"], "16:9");
    assert_eq!(sized["generationConfig"]["imageConfig"]["imageSize"], "2K");

    let plain = run(&[], &project.path().join("plain.json"));
    assert!(
        plain.get("generationConfig").is_none(),
        "without --aspect/--size the request keeps the model defaults: {plain}"
    );

    Command::new("bash")
        .arg(&script)
        .args(["--prompt", "hero", "--output", "x.png", "--size", "8K", "--dry-run"])
        .env("GEMINI_API_KEY", "offline-test-key")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--size must be 1K, 2K or 4K"));
}
