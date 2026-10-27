//! Runtime capability evidence for factory routes.
//!
//! This module is deliberately called only by explicit doctor/preflight
//! commands. Hook/session-start code must not call these adapters: capability
//! checks may invoke a provider CLI and must never turn session startup into a
//! network probe.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[cfg(test)]
use cas_factory::CapabilitySnapshot;
use cas_factory::{CapabilityAvailability, CapabilityEvidence, RouteIdentity};
use cas_pty::{Harness, HarnessConformanceReceipt};
use fs2::FileExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};

const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
// A transient failure has a shorter evidence TTL, but must not cause another
// short-lived Claude process during the same token refresh (GH #1294).
const CLAUDE_AUTH_PROBE_INTERVAL_MS: u64 = 60_000;
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
        Harness::OpenCode => (
            crate::opencode_preflight::QWENCLOUD_TOKEN_PLAN_PROVIDER,
            crate::opencode_preflight::QWENCLOUD_TOKEN_PLAN_ENDPOINT,
        ),
    };
    let model = if harness == Harness::OpenCode {
        model
            .strip_prefix("qwencloud/")
            .or_else(|| model.strip_prefix("hosted-token-plan/"))
            .unwrap_or(model)
    } else {
        model
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
            Harness::OpenCode => {
                let (_, evidence) = probe_key_presence(
                    identity.clone(),
                    crate::opencode_preflight::QWENCLOUD_TOKEN_PLAN_API_KEY_ENV,
                    None,
                    None,
                    now_ms,
                );
                evidence
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

/// Account-only probe for one harness, with no binary-version or model
/// context (cas-8a55).
///
/// `probe_harness` answers "can this route run", which needs a version probe
/// and a lane receipt. Spawn asks a narrower question — is the account this
/// worker would use logged in — and asking it before cutting a worktree is
/// what turns a half-hour of silent, assigned-but-dead workers into a refusal
/// the operator can act on. Harnesses without account plumbing return
/// `Unknown` rather than a verdict, because absence of a probe is not evidence
/// of a bad account.
pub(crate) fn probe_account_auth(
    cli: cas_mux::SupervisorCli,
    account_dir: Option<&str>,
    deadline: Deadline,
) -> CapabilityEvidence {
    let now_ms = cas_factory::CapabilitySnapshot::now_ms();
    match cli {
        cas_mux::SupervisorCli::Claude => probe_claude_auth(account_dir, now_ms, deadline),
        cas_mux::SupervisorCli::Codex => probe_codex_auth(account_dir, now_ms, deadline),
        _ => unknown(
            now_ms,
            "harness has no account probe",
            "No account preflight is defined for this harness.",
        ),
    }
}

fn probe_claude_auth(
    account_dir: Option<&str>,
    now_ms: u64,
    deadline: Deadline,
) -> CapabilityEvidence {
    let macos_ssh = cfg!(target_os = "macos")
        && ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    probe_claude_auth_with(account_dir, now_ms, deadline, macos_ssh, &mut run_command)
}

// The process/session boundary is explicit so fixtures never mutate global PATH
// or depend on the test host's platform or login keychain.
fn probe_claude_auth_with(
    account_dir: Option<&str>,
    now_ms: u64,
    deadline: Deadline,
    macos_ssh: bool,
    run: &mut impl FnMut(
        &mut Command,
        Deadline,
        Duration,
    ) -> Result<std::process::Output, BoundedCommandError>,
) -> CapabilityEvidence {
    // Account auth is independent of model. Persist only typed, credential-free
    // evidence so separate doctor/preflight processes share the same limit.
    let cached = (|| -> std::io::Result<CapabilityEvidence> {
        let home = dirs::home_dir().ok_or_else(|| std::io::Error::other("no home directory"))?;
        let profile = account_dir
            .filter(|dir| !dir.trim().is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        let profile = if profile.is_absolute() {
            profile
        } else {
            std::env::current_dir()?.join(profile)
        };
        let profile = profile.canonicalize().unwrap_or(profile);
        let mut key = Sha256::new();
        key.update(profile.as_os_str().as_encoded_bytes());
        key.update([u8::from(macos_ssh)]);
        let cache_dir = home.join(".cas/cache/claude-auth");
        std::fs::create_dir_all(&cache_dir)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(cache_dir.join(format!("{:x}.json", key.finalize())))?;
        // Concurrent per-model checks share the completed observation rather
        // than each launching Claude. Waiting consumes only the caller's budget.
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if deadline.remaining().is_zero() {
                        return Ok(unknown(
                            now_ms,
                            "Claude auth status probe is already running",
                            "Retry doctor after the current probe completes.",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(5).min(deadline.remaining()));
                }
                Err(error) => return Err(error),
            }
        }
        if let Ok(mut evidence) =
            serde_json::from_reader::<_, CapabilityEvidence>((&mut file).take(8192))
        {
            // The cache is user-writable; it cannot extend a state's freshness.
            evidence.ttl_ms = evidence.ttl_ms.min(evidence.availability.default_ttl_ms());
            let interval = evidence.ttl_ms.max(CLAUDE_AUTH_PROBE_INTERVAL_MS);
            if now_ms >= evidence.observed_at_ms
                && now_ms < evidence.observed_at_ms.saturating_add(interval)
            {
                // Do not renew the timestamp or freshness of cached evidence.
                return Ok(evidence);
            }
        }
        // Reserve the attempt before spawning so a crashed caller cannot cause
        // immediate retries by the next CLI process. No credentials are stored.
        write_claude_auth_cache(
            &mut file,
            &unknown(
                now_ms,
                "Claude auth status probe did not complete",
                "Retry doctor after one minute; a transient probe failure is not an unavailable account.",
            ),
        )?;
        let evidence = probe_claude_auth_uncached(account_dir, now_ms, deadline, macos_ssh, run);
        // A completed probe already supplies real evidence. Persistence failure
        // must neither hide that result nor launch the auth command a second time.
        let _ = write_claude_auth_cache(&mut file, &evidence);
        Ok(evidence)
    })();
    cached.unwrap_or_else(|_| {
        probe_claude_auth_uncached(account_dir, now_ms, deadline, macos_ssh, run)
    })
}

fn write_claude_auth_cache(file: &mut File, evidence: &CapabilityEvidence) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(evidence)?;
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(&bytes)
}

fn ssh_login_keychain_unavailable(
    account_dir: Option<&str>,
    deadline: Deadline,
    run: &mut impl FnMut(
        &mut Command,
        Deadline,
        Duration,
    ) -> Result<std::process::Output, BoundedCommandError>,
) -> bool {
    // Read only keychain metadata, never a password (-w/-g), and never unlock
    // or change the search list. In the reported SSH incident only System was
    // searchable; a locked keychain can also report interaction-not-allowed (36).
    let mut search_list = Command::new("security");
    search_list.args(["list-keychains", "-d", "user"]);
    if let Ok(output) = run(&mut search_list, deadline, AUTH_PROBE_TIMEOUT) {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let paths = stdout
                .lines()
                .filter_map(|line| line.trim().strip_prefix('"')?.strip_suffix('"'))
                .collect::<Vec<_>>();
            if !paths.is_empty()
                && !paths.iter().any(|path| {
                    Path::new(path)
                        .file_name()
                        .is_some_and(|name| name == "login.keychain" || name == "login.keychain-db")
                })
            {
                return true;
            }
        }
    }
    let service = crate::claude_credentials::keychain_service(account_dir);
    let mut metadata = Command::new("security");
    metadata.args(["find-generic-password", "-s", &service]);
    matches!(run(&mut metadata, deadline, AUTH_PROBE_TIMEOUT), Ok(output) if output.status.code() == Some(36))
}

fn probe_claude_auth_uncached(
    account_dir: Option<&str>,
    now_ms: u64,
    deadline: Deadline,
    macos_ssh: bool,
    run: &mut impl FnMut(
        &mut Command,
        Deadline,
        Duration,
    ) -> Result<std::process::Output, BoundedCommandError>,
) -> CapabilityEvidence {
    if macos_ssh && ssh_login_keychain_unavailable(account_dir, deadline, run) {
        return unknown(
            now_ms,
            "macOS login keychain locked in ssh session",
            "Unlock the login keychain from a local macOS session, then retry doctor after one minute.",
        );
    }
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
    match run(&mut command, deadline, AUTH_PROBE_TIMEOUT) {
        Ok(output) if output.status.success() => {
            match serde_json::from_slice::<ClaudeAuthStatus>(&output.stdout) {
                Ok(status) if status.logged_in => {
                    available(now_ms, "Claude auth status is logged in")
                }
                Ok(_) => unavailable(
                    now_ms,
                    "Claude auth status is logged out",
                    "Run `claude login`, then retry doctor after one minute.",
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
    let selected = account_dir
        .filter(|dir| !dir.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("CODEX_HOME")
                .filter(|dir| !dir.is_empty())
                .map(Into::into)
        });
    let homes = dirs::home_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is unavailable"))
        .and_then(|home| crate::accounts::codex_homes(&home, selected.as_deref()));
    let Ok(homes) = homes else {
        return unknown(
            now_ms,
            "Codex account registry could not be read",
            "Retry doctor after confirming HOME and the Codex account directories are readable.",
        );
    };
    let mut uncertain = None;
    for home in homes {
        let evidence = probe_codex_home_auth(&home, now_ms, deadline);
        match evidence.availability {
            CapabilityAvailability::Available => return evidence,
            CapabilityAvailability::Unknown => uncertain = Some(evidence),
            CapabilityAvailability::Unavailable => {}
        }
    }
    // One uncertain account prevents a false logout verdict; every probe
    // shares the caller's deadline, including when the registry has many homes.
    uncertain.unwrap_or_else(|| classify_codex_auth(false, "Not logged in", "", now_ms))
}

fn probe_codex_home_auth(home: &Path, now_ms: u64, deadline: Deadline) -> CapabilityEvidence {
    let mut command = Command::new("codex");
    command.env("CODEX_HOME", home);
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
        Harness::OpenCode => format!(
            "Set {}, then rerun doctor.",
            crate::opencode_preflight::QWENCLOUD_TOKEN_PLAN_API_KEY_ENV
        ),
    }
}

fn harness_name(harness: Harness) -> &'static str {
    match harness {
        Harness::ClaudeCode => "claude",
        Harness::CodexCli => "codex",
        Harness::GrokBuild => "grok",
        Harness::OpenCode => "opencode",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_factory::CAPABILITY_UNKNOWN_TTL_MS;

    #[cfg(unix)]
    fn claude_probe_fixture() -> crate::test_support::TestEnvGuard {
        use std::os::unix::fs::PermissionsExt;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let bin = env.home().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for (name, body) in [
            (
                "claude",
                "printf 'probe\\n' >> \"$HOME/claude-calls\"; printf '%s' \"$CLAUDE_TEST_STATUS\"; exit \"$CLAUDE_TEST_EXIT\"",
            ),
            (
                "security",
                "if [ \"$1\" = list-keychains ]; then printf '%s\\n' \"$CLAUDE_TEST_KEYCHAINS\"; else printf '%s' \"$3\" > \"$HOME/keychain-service\"; exit \"$CLAUDE_TEST_KEYCHAIN_EXIT\"; fi",
            ),
        ] {
            let path = bin.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        env.set("CLAUDE_TEST_STATUS", r#"{"loggedIn":true}"#);
        env.set("CLAUDE_TEST_EXIT", "0");
        env.set(
            "CLAUDE_TEST_KEYCHAINS",
            "    \"/Library/Keychains/System.keychain\"",
        );
        env.set("CLAUDE_TEST_KEYCHAIN_EXIT", "36");
        for name in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
            env.remove(name);
        }
        env
    }

    #[cfg(unix)]
    fn fixture_claude_probe(
        env: &crate::test_support::TestEnvGuard,
        account: Option<&str>,
        now_ms: u64,
        macos_ssh: bool,
    ) -> CapabilityEvidence {
        probe_claude_auth_with(
            account,
            now_ms,
            Deadline::after(Duration::from_secs(30)),
            macos_ssh,
            &mut |command, deadline, cap| {
                command.env("PATH", env.home().join("bin"));
                run_command(command, deadline, cap)
            },
        )
    }

    #[cfg(unix)]
    #[test]
    fn claude_cache_failure_keeps_real_auth_evidence_cas_eb0f() {
        let mut env = claude_probe_fixture();
        let cache = env.home().join(".cas/cache");
        std::fs::create_dir_all(&cache).unwrap();
        // A file in place of the cache directory fails even under privileged
        // test runners; unlike chmod, it deterministically prevents caching.
        std::fs::write(cache.join("claude-auth"), "not a directory").unwrap();
        let calls = env.home().join("claude-calls");
        for (index, (response, expected)) in [
            (r#"{"loggedIn":false}"#, CapabilityAvailability::Unavailable),
            (r#"{"loggedIn":true}"#, CapabilityAvailability::Available),
        ]
        .into_iter()
        .enumerate()
        {
            env.set("CLAUDE_TEST_STATUS", response);
            let evidence = fixture_claude_probe(&env, None, 10, false);
            assert_eq!(
                evidence.availability, expected,
                "a broken optional cache must not hide the account's real auth status"
            );
            assert_eq!(
                std::fs::read_to_string(&calls).unwrap().lines().count(),
                index + 1,
                "each failed-cache request must execute exactly one real auth probe"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn claude_ssh_locked_keychain_is_not_a_logout_cas_43b3() {
        let mut env = claude_probe_fixture();
        env.set("SSH_CONNECTION", "fixture");
        env.set("CLAUDE_TEST_STATUS", r#"{"loggedIn":false}"#);
        env.set("CLAUDE_TEST_EXIT", "1");
        for (profile, search_list, expected_service) in [
            (None, "    \"/Library/Keychains/System.keychain\"", None),
            (
                Some("listed-but-locked"),
                "    \"/Users/fixture/Library/Keychains/login.keychain-db\"",
                None,
            ),
            (
                Some("profile-e\u{301}"),
                "    \"/Users/fixture/Library/Keychains/login.keychain-db\"",
                Some("Claude Code-credentials-789b3bab"),
            ),
        ] {
            env.set("CLAUDE_TEST_KEYCHAINS", search_list);
            let evidence = fixture_claude_probe(&env, profile, 10, true);
            if let Some(expected_service) = expected_service {
                assert_eq!(
                    std::fs::read_to_string(env.home().join("keychain-service")).unwrap(),
                    expected_service,
                    "metadata lookup must use the selected profile's NFC path hash"
                );
            }
            assert_eq!(evidence.availability, CapabilityAvailability::Unknown);
            // pin: incident #1294 requires this diagnostic instead of login advice.
            assert_eq!(
                evidence.reason.as_deref(),
                Some("macOS login keychain locked in ssh session")
            );
            assert!(
                !evidence
                    .remediation
                    .unwrap_or_default()
                    .contains("claude login")
            );
            assert!(
                !env.home().join("claude-calls").exists(),
                "inaccessible keychain must not trigger a Claude auth probe"
            );
        }
        // An SSH observation must not leak into a local session's cache.
        env.set("CLAUDE_TEST_EXIT", "0");
        let local = fixture_claude_probe(&env, None, 10, false);
        assert_eq!(local.availability, CapabilityAvailability::Unavailable);
        assert!(local.remediation.unwrap().contains("claude login"));
    }

    #[cfg(unix)]
    #[test]
    fn claude_auth_probes_are_bounded_per_profile_cas_43b3() {
        let mut env = claude_probe_fixture();
        let calls = env.home().join("claude-calls");
        let first = fixture_claude_probe(&env, None, 10, false);
        assert_eq!(first.availability, CapabilityAvailability::Available);
        env.set("CLAUDE_TEST_STATUS", r#"{"loggedIn":false}"#);
        let cached = fixture_claude_probe(&env, None, 1_010, false);
        assert_eq!(
            cached, first,
            "a repeated check must reuse the observation, including its timestamp"
        );
        let default_profile = env.home().join(".claude");
        let alias = fixture_claude_probe(&env, default_profile.to_str(), 1_010, false);
        assert_eq!(
            alias, first,
            "the explicit default directory must share the default cache entry"
        );
        assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 1);
        let other = fixture_claude_probe(&env, Some("other-profile"), 1_010, false);
        assert_eq!(other.availability, CapabilityAvailability::Unavailable);
        let refreshed = fixture_claude_probe(&env, None, 300_010, false);
        assert_eq!(refreshed.availability, CapabilityAvailability::Unavailable);
        assert_eq!(refreshed.observed_at_ms, 300_010);
        // An external `claude login` cannot notify this process's cache. Its
        // logged-out advice must explain the bounded delay before a new probe.
        env.set("CLAUDE_TEST_STATUS", r#"{"loggedIn":true}"#);
        let after_login = fixture_claude_probe(&env, None, 301_010, false);
        let login_advice_matches = after_login == refreshed
            && after_login
                .remediation
                .as_deref()
                .is_some_and(|advice| advice.contains("one minute"));
        let logged_in = fixture_claude_probe(&env, None, 360_010, false);
        assert_eq!(logged_in.availability, CapabilityAvailability::Available);
        env.set("CLAUDE_TEST_STATUS", "unrecognized failure");
        env.set("CLAUDE_TEST_EXIT", "1");
        let failed = fixture_claude_probe(&env, None, 660_010, false);
        assert_eq!(failed.availability, CapabilityAvailability::Unknown);
        let repeated = fixture_claude_probe(&env, None, 670_010, false);
        assert_eq!(
            repeated, failed,
            "transient failures must also throttle probes without refreshing evidence"
        );
        // The credential-free cache is user-writable: a forged TTL must not
        // extend either the returned freshness or the normal probe interval.
        let cache = std::fs::read_dir(env.home().join(".cas/cache/claude-auth"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                serde_json::from_slice::<CapabilityEvidence>(&std::fs::read(path).unwrap())
                    .unwrap()
                    .observed_at_ms
                    == failed.observed_at_ms
            })
            .unwrap();
        let mut forged = failed.clone();
        forged.ttl_ms = u64::MAX;
        std::fs::write(&cache, serde_json::to_vec(&forged).unwrap()).unwrap();
        let clamped = fixture_claude_probe(&env, None, 680_010, false);
        let expired = fixture_claude_probe(&env, None, 720_010, false);
        assert!(
            login_advice_matches && clamped == failed && expired.observed_at_ms == 720_010,
            "login delay advice={login_advice_matches}; tampered TTL returned {clamped:?}; expiry returned {expired:?}"
        );
        assert_eq!(std::fs::read_to_string(calls).unwrap().lines().count(), 6);
    }

    #[test]
    fn route_identity_keeps_endpoint_and_account_separate() {
        let first = route_identity(
            Harness::ClaudeCode,
            "anthropic",
            CLAUDE_ENDPOINT,
            "claude-opus-5",
            "main",
        );
        let second = route_identity(
            Harness::ClaudeCode,
            "anthropic",
            "https://proxy.example",
            "claude-opus-5",
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

    #[cfg(unix)]
    #[test]
    fn codex_registered_accounts_enable_standard_lane_cas_cba9() {
        use std::os::unix::fs::PermissionsExt;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let bin = env.home().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let codex = bin.join("codex");
        std::fs::write(
            &codex,
            r#"#!/bin/sh
    [ "$1 $2" = 'login status' ] || exit 2
    printf '%s\n' "$CODEX_HOME" >> "$HOME/probes"
    if [ -f "$CODEX_HOME/auth.json" ]; then
        printf 'Logged in using ChatGPT\n'
    else
        printf 'Not logged in\n'
        exit 1
    fi
    "#,
        )
        .unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        if std::env::var_os("CODEX_REGISTRY_FIXTURE_CHILD").is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "capability::tests::codex_registered_accounts_enable_standard_lane_cas_cba9",
                    "--nocapture",
                ])
                .env("PATH", &bin)
                .env("CODEX_REGISTRY_FIXTURE_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let logged_out = env.home().join(".codex-a@example.com");
        let logged_in = env.home().join(".codex-b@example.com");
        for dir in [&logged_out, &logged_in] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(logged_in.join("auth.json"), "{}").unwrap();
        let registry = cas_factory::embedded_registry().unwrap();
        for (explicit, authenticated) in [(false, true), (true, false), (false, false)] {
            if explicit {
                env.set("CODEX_HOME", &logged_out);
            } else {
                env.remove("CODEX_HOME");
            }
            let now = CapabilitySnapshot::now_ms();
            let mut snapshot = CapabilitySnapshot::default();
            for name in ["codex_sol_6_1", "codex_sol_6"] {
                let (identity, evidence) = probe_harness(
                    Harness::CodexCli,
                    &registry.recipes[name].model,
                    None,
                    &BinaryObservation::Observed("fixture".into()),
                    None,
                    now,
                    Deadline::after(Duration::from_secs(5)),
                );
                assert_eq!(
                    evidence.availability,
                    if authenticated {
                        CapabilityAvailability::Available
                    } else {
                        CapabilityAvailability::Unavailable
                    }
                );
                if !authenticated {
                    assert!(
                        evidence
                            .remediation
                            .as_deref()
                            .unwrap()
                            .contains("codex login")
                    );
                }
                snapshot.record(identity, evidence);
            }
            assert_eq!(
                cas_factory::resolve_lane("standard", &snapshot).is_ok(),
                authenticated
            );
            if explicit {
                std::fs::remove_file(logged_in.join("auth.json")).unwrap();
            }
        }
        let probes = std::fs::read_to_string(env.home().join("probes")).unwrap();
        assert!(probes.lines().all(|line| line == logged_out.to_str().unwrap()
                || line == logged_in.to_str().unwrap()), "unexpected home probed: {probes}");
        assert!(!env.home().join(".codex").exists());
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
