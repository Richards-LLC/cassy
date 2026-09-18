//! No struct that holds a credential may derive `Debug` (task cas-c75d).
//!
//! The per-type tests beside each impl prove the *current* types do not leak.
//! This one proves the *next* type will not either: it re-runs, as a test, the
//! sweep that found the original thirteen leaks, so a newly added
//! `#[derive(Debug)]` over a `token` field fails here instead of printing a
//! bearer into a log six months from now.
//!
//! A derived `Debug` prints every field verbatim. Any `{:?}`, `tracing` field,
//! `unwrap()` panic message, or `anyhow` error chain that formats such a struct
//! therefore writes the live credential wherever that output goes.

use std::path::{Path, PathBuf};

/// Field names that hold a credential rather than a reference to one.
///
/// Deliberately exact: `token_hash` and `credential_id` are handles, safe to
/// print, and must not trip the guard — matching requires the field name to be
/// followed directly by `:`.
const CREDENTIAL_FIELDS: &[&str] = &[
    "token",
    "api_key",
    "apikey",
    "secret",
    "password",
    "bearer",
    "credential",
    "upload_url",
    "access_token",
    "refresh_token",
    "private_key",
    "passphrase",
];

/// Structs allowed to derive `Debug` despite a matching field name, each with
/// the reason it is not a credential. Empty today; kept so a future exception
/// is recorded here rather than by loosening the rule.
const ALLOWLIST: &[(&str, &str)] = &[];

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/cas-cli`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cas-cli must have a parent directory")
        .to_path_buf()
}

fn rust_sources(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_credential_field(line: &str) -> bool {
    let trimmed = line.trim().trim_start_matches("pub ");
    let trimmed = if let Some(rest) = trimmed.strip_prefix("pub(") {
        rest.split_once(')').map(|(_, r)| r.trim()).unwrap_or(rest)
    } else {
        trimmed
    };
    CREDENTIAL_FIELDS.iter().any(|field| {
        trimmed
            .strip_prefix(field)
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    })
}

struct Violation {
    location: String,
    name: String,
    field: String,
}

fn scan(path: &Path, repo: &Path) -> Vec<Violation> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut violations = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        let Some(name) = struct_name(line) else {
            continue;
        };

        // Attributes and doc comments immediately above the struct.
        let mut derives_debug = false;
        let mut cursor = index;
        while cursor > 0 {
            let above = lines[cursor - 1].trim();
            if above.starts_with("#[") || above.starts_with("//") || above.is_empty() {
                if above.starts_with("#[derive") && above.contains("Debug") {
                    derives_debug = true;
                }
                cursor -= 1;
            } else {
                break;
            }
        }
        if !derives_debug {
            continue;
        }

        // The struct body, to its matching close brace.
        let mut depth = 0i32;
        let mut opened = false;
        for body_line in lines.iter().skip(index) {
            depth += body_line.matches('{').count() as i32;
            if body_line.contains('{') {
                opened = true;
            }
            depth -= body_line.matches('}').count() as i32;
            if opened && !std::ptr::eq(*body_line, lines[index]) && is_credential_field(body_line) {
                violations.push(Violation {
                    location: format!(
                        "{}:{}",
                        path.strip_prefix(repo).unwrap_or(path).display(),
                        index + 1
                    ),
                    name: name.to_string(),
                    field: body_line.trim().trim_end_matches(',').to_string(),
                });
                break;
            }
            if opened && depth <= 0 {
                break;
            }
        }
    }
    violations
}

fn struct_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let rest = trimmed
        .strip_prefix("pub struct ")
        .or_else(|| trimmed.strip_prefix("struct "))
        .or_else(|| {
            trimmed
                .strip_prefix("pub(")
                .and_then(|r| r.split_once(')'))
                .and_then(|(_, r)| r.trim_start().strip_prefix("struct "))
        })?;
    let name = rest
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()?;
    (!name.is_empty()).then_some(name)
}

#[test]
fn no_struct_holding_a_credential_derives_debug() {
    let repo = repo_root();
    let mut sources = Vec::new();
    rust_sources(&repo.join("cas-cli").join("src"), &mut sources);
    for crate_dir in std::fs::read_dir(repo.join("crates"))
        .expect("crates/ must exist")
        .flatten()
    {
        rust_sources(&crate_dir.path().join("src"), &mut sources);
    }
    assert!(
        sources.len() > 100,
        "the scan found only {} files; it is not reaching the source tree",
        sources.len()
    );

    let violations: Vec<Violation> = sources
        .iter()
        .flat_map(|path| scan(path, &repo))
        .filter(|violation| {
            !ALLOWLIST
                .iter()
                .any(|(allowed, _)| *allowed == violation.name)
        })
        .collect();

    assert!(
        violations.is_empty(),
        "a derived Debug prints the credential verbatim. Replace it with a redacting \
         `impl fmt::Debug` (see `ArtifactUploadClient` in cas-cli/src/artifacts/cloud.rs) \
         and add a test asserting the value never appears in `{{:?}}`:\n{}",
        violations
            .iter()
            .map(|v| format!("  {} — struct {} holds `{}`", v.location, v.name, v.field))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_guard_detects_a_planted_leak() {
    // Without this, a scanner that silently matches nothing would "pass"
    // forever. The fixture is the exact shape the guard exists to catch.
    let dir = tempfile::TempDir::new().unwrap();
    let planted = dir.path().join("planted.rs");
    std::fs::write(
        &planted,
        "#[derive(Debug, Clone)]\npub struct LeakyClient {\n    endpoint: String,\n    token: String,\n}\n",
    )
    .unwrap();

    let found = scan(&planted, dir.path());
    assert_eq!(
        found.len(),
        1,
        "the guard must catch a derived Debug over a token"
    );
    assert_eq!(found[0].name, "LeakyClient");
    assert!(found[0].field.contains("token"), "{}", found[0].field);
}

#[test]
fn the_guard_does_not_trip_on_handles_or_on_a_manual_impl() {
    let dir = tempfile::TempDir::new().unwrap();

    // `token_hash` and `credential_id` are safe to print.
    let handles = dir.path().join("handles.rs");
    std::fs::write(
        &handles,
        "#[derive(Debug)]\npub struct Capability {\n    pub token_hash: String,\n    pub credential_id: String,\n}\n",
    )
    .unwrap();
    assert!(
        scan(&handles, dir.path()).is_empty(),
        "a hash or an id is a handle, not a credential"
    );

    // A manual impl is the sanctioned fix and must not be flagged.
    let manual = dir.path().join("manual.rs");
    std::fs::write(
        &manual,
        "#[derive(Clone)]\npub struct Client {\n    token: String,\n}\n\nimpl fmt::Debug for Client {\n    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {\n        f.debug_struct(\"Client\").field(\"token\", &\"[redacted]\").finish()\n    }\n}\n",
    )
    .unwrap();
    assert!(
        scan(&manual, dir.path()).is_empty(),
        "a redacting manual impl is the fix, not a violation"
    );
}

#[test]
fn a_credential_field_is_recognised_however_it_is_declared() {
    for declaration in [
        "    token: String,",
        "    pub token: String,",
        "    pub(crate) token: Option<String>,",
        "    pub api_key: String,",
        "    upload_url: String,",
        "    pub access_token: String,",
    ] {
        assert!(is_credential_field(declaration), "missed: {declaration}");
    }
    for safe in [
        "    token_hash: String,",
        "    pub credential_id: String,",
        "    pub tokens_used: u64,",
        "    pub secrets_dir: PathBuf,",
        "    endpoint: String,",
    ] {
        assert!(!is_credential_field(safe), "false positive: {safe}");
    }
}
