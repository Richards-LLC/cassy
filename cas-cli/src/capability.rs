//! Runtime capability evidence for factory routes.
//!
//! This module is deliberately called only by explicit doctor/preflight
//! commands. Hook/session-start code must not call these adapters: capability
//! checks may invoke a provider CLI and must never turn session startup into a
//! network probe.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use cas_factory::{CapabilityAvailability, CapabilityEvidence, CapabilitySnapshot, RouteIdentity};
use cas_pty::{Harness, HarnessConformanceReceipt};
use serde::Deserialize;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};

const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const CLAUDE_ENDPOINT: &str = "https://api.anthropic.com";
const CODEX_ENDPOINT: &str = "https://api.openai.com";
const GROK_ENDPOINT: &str = "https://api.x.ai";

/// The binary observation already collected by doctor/preflight. Keeping it
/// separate from auth evidence lets a timeout remain Unknown rather than
/// being collapsed into a misleading missing-account result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BinaryObservation {
    Observed(String),
    Unavailable,
    TimedOut,
}

#[derive(Debug, Deserialize)]
struct ClaudeAuthStatus {
    #[serde(rename = "loggedIn")]
    logged_in: bool,
}

/// Build the route key used by all provider adapters. `account_profile` is a
/// label/path only; credentials and key values never enter the identity.
pub fn route_identity(
    harness: Harness,
    provider: &str,
    endpoint: &str,
    model: &str,
    account_profile: &str,
) -> RouteIdentity {
    RouteIdentity::new(
        harness_name(harness),
        provider,
        endpoint,
        model,
        account_profile,
    )
}

/// Build the canonical identity for one built-in harness route. Callers that
/// display or consume evidence should use this exact key rather than matching
/// on the harness name alone: model, endpoint, and account profile are route
/// selectors too.
pub fn harness_route_identity(
    harness: Harness,
    model: &str,
    account_profile: &str,
) -> RouteIdentity {
    let (provider, endpoint) = match harness {
        Harness::ClaudeCode => ("anthropic", CLAUDE_ENDPOINT),
        Harness::CodexCli => ("openai", CODEX_ENDPOINT),
        Harness::GrokBuild => ("xai", GROK_ENDPOINT),
    };
    route_identity(harness, provider, endpoint, model, account_profile)
}

/// Collect the capability evidence for one supported harness route.
///
/// The caller supplies the version observation and typed conformance receipts
/// so this adapter does not repeat the binary probe. Claude and Codex auth are
/// checked with their own local CLI status commands; Grok's models command is
/// the existing bounded auth/availability surface and is still restricted to
/// doctor/preflight callers.
pub(crate) fn probe_harness(
    harness: Harness,
    model: &str,
    account_dir: Option<&str>,
    binary: &BinaryObservation,
    receipt: Option<&HarnessConformanceReceipt>,
    now_ms: u64,
    deadline: Deadline,
) -> (RouteIdentity, CapabilityEvidence) {
    let profile = account_dir.unwrap_or("default");
    let identity = harness_route_identity(harness, model, profile);

    let evidence = match binary {
        BinaryObservation::Unavailable => unavailable(
            now_ms,
            format!("{} binary is not available on PATH", harness_name(harness)),
            enable_path(harness),
        ),
        BinaryObservation::TimedOut => unknown(
            now_ms,
            format!("{} binary version probe timed out", harness_name(harness)),
            "Retry `cas factory doctor` or `cas factory preflight`.",
        ),
        BinaryObservation::Observed(_) => match harness {
            Harness::ClaudeCode => probe_claude_auth(account_dir, now_ms, deadline),
            Harness::CodexCli => probe_codex_auth(account_dir, now_ms, deadline),
            Harness::GrokBuild => {
                probe_grok_auth_and_pin(account_dir, receipt, binary, now_ms, deadline)
            }
        },
    };
    (identity, evidence)
}

/// Key-presence adapter for Qwen/OpenCode routes.
///
/// It intentionally reads only whether a configured environment variable or
/// credential file exists and is non-empty. It never returns, logs, or stores
/// the key value and never contacts the endpoint. `lane_pairing` is an
/// optional human-readable route pairing check supplied by the caller.
pub fn probe_key_presence(
    identity: RouteIdentity,
    key_env: &str,
    credentials_path: Option<&Path>,
    lane_pairing: Option<Result<(), String>>,
    now_ms: u64,
) -> (RouteIdentity, CapabilityEvidence) {
    if let Some(Err(reason)) = lane_pairing {
        return (
            identity,
            unavailable(
                now_ms,
                format!("route pairing is invalid: {reason}"),
                format!("Use a {key_env} value matching the configured route."),
            ),
        );
    }

    let env_present = std::env::var(key_env)
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    let file_present = credentials_path.is_some_and(|path| {
        path.is_file()
            && std::fs::metadata(path)
                .ok()
                .is_some_and(|metadata| metadata.len() > 0)
    });
    if env_present || file_present {
        (
            identity,
            available(
                now_ms,
                format!("{key_env} or the configured credentials file is present"),
            ),
        )
    } else {
        (
            identity,
            unavailable(
                now_ms,
                format!("{key_env} and the configured credentials file are absent"),
                format!("Set {key_env} or generate a Token Plan key, then rerun doctor."),
            ),
        )
    }
}

fn probe_claude_auth(
    account_dir: Option<&str>,
    now_ms: u64,
    deadline: Deadline,
) -> CapabilityEvidence {
    let mut command = Command::new("claude");
    if let Some(account_dir) = account_dir.filter(|dir| !dir.trim().is_empty()) {
        command
            .env("CLAUDE_CONFIG_DIR", account_dir)
            .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", account_dir);
    } else {
        command
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR");
    }
    command
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .args(["auth", "status", "--json"]);
    match run_command(&mut command, deadline, AUTH_PROBE_TIMEOUT) {
        Ok(output) if output.status.success() => {
            match serde_json::from_slice::<ClaudeAuthStatus>(&output.stdout) {
                Ok(status) if status.logged_in => {
                    available(now_ms, "Claude auth status is logged in")
                }
                Ok(_) => unavailable(
                    now_ms,
                    "Claude auth status is logged out",
                    "Run `claude login`, then rerun doctor.",
                ),
                Err(_) => unknown(
                    now_ms,
                    "Claude auth status returned an unrecognized response",
                    "Retry doctor after confirming the Claude CLI is healthy.",
                ),
            }
        }
        Ok(_) | Err(BoundedCommandError::Io) => unknown(
            now_ms,
            "Claude auth status could not be read",
            "Retry doctor after confirming the Claude CLI is healthy.",
        ),
        Err(BoundedCommandError::TimedOut) => unknown(
            now_ms,
            "Claude auth status probe timed out",
            "Retry doctor; a transient auth probe failure is not an unavailable account.",
        ),
    }
}

fn probe_codex_auth(
    account_dir: Option<&str>,
    now_ms: u64,
    deadline: Deadline,
) -> CapabilityEvidence {
    let mut command = Command::new("codex");
    if let Some(account_dir) = account_dir.filter(|dir| !dir.trim().is_empty()) {
        command.env("CODEX_HOME", account_dir);
    } else {
        command.env_remove("CODEX_HOME");
    }
    command.args(["login", "status"]);
    match run_command(&mut command, deadline, AUTH_PROBE_TIMEOUT) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            classify_codex_auth(output.status.success(), &stdout, &stderr, now_ms)
        }
        Err(BoundedCommandError::Io) => unknown(
            now_ms,
            "Codex login status could not be read",
            "Retry doctor after confirming the Codex CLI is healthy.",
        ),
        Err(BoundedCommandError::TimedOut) => unknown(
            now_ms,
            "Codex login status probe timed out",
            "Retry doctor; a transient auth probe failure is not an unavailable account.",
        ),
    }
}

fn classify_codex_auth(
    success: bool,
    stdout: &str,
    stderr: &str,
    now_ms: u64,
) -> CapabilityEvidence {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if combined.contains("not logged in") || combined.contains("logged out") {
        unavailable(
            now_ms,
            "Codex login status reports no authenticated account",
            "Run `codex login`, then rerun doctor.",
        )
    } else if combined.contains("logged in") || success {
        available(now_ms, "Codex login status is authenticated")
    } else {
        unknown(
            now_ms,
            "Codex login status returned an unrecognized response",
            "Retry doctor after confirming the Codex CLI is healthy.",
        )
    }
}

fn probe_grok_auth_and_pin(
    account_dir: Option<&str>,
    receipt: Option<&HarnessConformanceReceipt>,
    binary: &BinaryObservation,
    now_ms: u64,
    deadline: Deadline,
) -> CapabilityEvidence {
    let Some(receipt) = receipt else {
        return unavailable(
            now_ms,
            "Grok has no validated conformance receipt for this route",
            "Run and persist the typed Grok factory conformance matrix.",
        );
    };
    if !receipt.validates_pin() {
        return unavailable(
            now_ms,
            "Grok conformance evidence failed its required checks",
            "Repair failed checks and rerun the typed Grok factory conformance matrix.",
        );
    }
    if let BinaryObservation::Observed(version) = binary {
        if !observed_pin_matches(version, &receipt.harness_version) {
            return unavailable(
                now_ms,
                format!(
                    "Grok binary {version} differs from validated pin {}",
                    receipt.harness_version
                ),
                format!(
                    "Use validated Grok {} or rerun the conformance matrix.",
                    receipt.harness_version
                ),
            );
        }
    }

    let mut command = Command::new("grok");
    if let Some(account_dir) = account_dir.filter(|dir| !dir.trim().is_empty()) {
        command.env("GROK_HOME", account_dir);
    }
    command.arg("models");
    match run_command(&mut command, deadline, AUTH_PROBE_TIMEOUT) {
        Ok(output) if output.status.success() => available(
            now_ms,
            "Grok auth/model availability is observable at the validated pin",
        ),
        Ok(output) => {
            let output = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .to_ascii_lowercase();
            if output.contains("auth")
                || output.contains("login")
                || output.contains("unauthorized")
                || output.contains("credential")
            {
                unavailable(
                    now_ms,
                    "Grok model availability reports an authentication failure",
                    "Sign in to Grok Build, then rerun doctor.",
                )
            } else {
                unknown(
                    now_ms,
                    "Grok model availability returned an unrecognized failure",
                    "Retry doctor; a transient provider failure is not an unavailable account.",
                )
            }
        }
        Err(BoundedCommandError::Io) => unknown(
            now_ms,
            "Grok model availability could not be read",
            "Retry doctor after confirming the Grok CLI is healthy.",
        ),
        Err(BoundedCommandError::TimedOut) => unknown(
            now_ms,
            "Grok model availability probe timed out",
            "Retry doctor; a transient provider failure is not an unavailable account.",
        ),
    }
}

fn observed_pin_matches(observed: &str, pin: &str) -> bool {
    observed == pin
        || observed.split_whitespace().any(|token| {
            token.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && character != '.' && character != '-'
            }) == pin
        })
}

fn available(now_ms: u64, reason: impl Into<String>) -> CapabilityEvidence {
    CapabilityEvidence::new(CapabilityAvailability::Available, now_ms).with_reason(reason)
}

fn unavailable(
    now_ms: u64,
    reason: impl Into<String>,
    remediation: impl Into<String>,
) -> CapabilityEvidence {
    CapabilityEvidence::new(CapabilityAvailability::Unavailable, now_ms)
        .with_reason(reason)
        .with_remediation(remediation)
}

fn unknown(
    now_ms: u64,
    reason: impl Into<String>,
    remediation: impl Into<String>,
) -> CapabilityEvidence {
    CapabilityEvidence::new(CapabilityAvailability::Unknown, now_ms)
        .with_reason(reason)
        .with_remediation(remediation)
}

fn enable_path(harness: Harness) -> String {
    match harness {
        Harness::ClaudeCode => "Run `claude login`, then rerun doctor.".to_string(),
        Harness::CodexCli => "Run `codex login`, then rerun doctor.".to_string(),
        Harness::GrokBuild => "Sign in to Grok Build, then rerun doctor.".to_string(),
    }
}

fn harness_name(harness: Harness) -> &'static str {
    match harness {
        Harness::ClaudeCode => "claude",
        Harness::CodexCli => "codex",
        Harness::GrokBuild => "grok",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_factory::CAPABILITY_UNKNOWN_TTL_MS;

    #[test]
    fn route_identity_keeps_endpoint_and_account_separate() {
        let first = route_identity(
            Harness::ClaudeCode,
            "anthropic",
            CLAUDE_ENDPOINT,
            "opus-5",
            "main",
        );
        let second = route_identity(
            Harness::ClaudeCode,
            "anthropic",
            "https://proxy.example",
            "opus-5",
            "main",
        );
        assert_ne!(first, second);
        assert!(first.key().contains("anthropic"));
        assert!(first.key().contains("main"));
    }

    #[test]
    fn human_readable_binary_version_can_match_a_conformance_pin() {
        assert!(observed_pin_matches("grok 0.2.117 (build)", "0.2.117"));
        assert!(!observed_pin_matches("grok 0.2.118", "0.2.117"));
    }

    #[test]
    fn codex_auth_parser_distinguishes_logged_out_and_unknown() {
        let logged_out = classify_codex_auth(false, "Not logged in", "", 10);
        assert_eq!(logged_out.availability, CapabilityAvailability::Unavailable);
        let unknown = classify_codex_auth(false, "unexpected", "", 10);
        assert_eq!(unknown.availability, CapabilityAvailability::Unknown);
    }

    #[test]
    fn unknown_evidence_expires_with_unknown_ttl() {
        let evidence = unknown(10, "timeout", "retry");
        assert_eq!(evidence.ttl_ms, CAPABILITY_UNKNOWN_TTL_MS);
        assert_eq!(
            evidence.availability_at(10 + CAPABILITY_UNKNOWN_TTL_MS),
            CapabilityAvailability::Unknown
        );
    }

    #[test]
    fn custom_ttl_is_preserved_in_route_status() {
        let identity = RouteIdentity::new("codex", "openai", CODEX_ENDPOINT, "luna", "default");
        let evidence =
            CapabilityEvidence::new(CapabilityAvailability::Available, 10).with_ttl_ms(2);
        let mut snapshot = CapabilitySnapshot::default();
        snapshot.record(identity.clone(), evidence);
        let status = snapshot.status_at(&identity, 12).unwrap();
        assert!(status.stale);
        assert_eq!(status.availability, CapabilityAvailability::Unknown);
        assert_eq!(status.expires_at_ms, 12);
    }

    #[test]
    fn timed_out_binary_probe_is_unknown_without_running_auth() {
        let (_, evidence) = probe_harness(
            Harness::CodexCli,
            "gpt-5.6-luna",
            None,
            &BinaryObservation::TimedOut,
            None,
            10,
            Deadline::after(Duration::from_secs(1)),
        );
        assert_eq!(evidence.availability, CapabilityAvailability::Unknown);
        assert!(
            evidence
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("timed out"))
        );
    }

    #[test]
    fn key_presence_does_not_store_secret_value() {
        let root = tempfile::tempdir().unwrap();
        let credentials = root.path().join("credentials");
        std::fs::write(&credentials, "secret-value").unwrap();
        let identity = RouteIdentity::new(
            "opencode",
            "qwen",
            "http://127.0.0.1:8000",
            "qwen3",
            "local",
        );
        let (identity, evidence) = probe_key_presence(
            identity,
            "CAS_TEST_MISSING_KEY",
            Some(&credentials),
            None,
            1,
        );
        assert_eq!(evidence.availability, CapabilityAvailability::Available);
        assert!(!format!("{identity:?}{evidence:?}").contains("secret-value"));
    }
}
