use std::path::{Path, PathBuf};

use cas_store::{EventStore, SqliteEventStore};
use chrono::Utc;

use super::{Family, REGISTRY, UNCODED, facts, find, tag, codes_in};

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "target") {
                rust_sources(&path, out);
            }
        } else if path.extension().is_some_and(|ext| matches!(ext.to_str(), Some("rs" | "sh" | "py"))) {
            out.push(path);
        }
    }
}

/// Codes a production line emits: `gates::tag("CODE"` calls and `[CODE]` literals.
fn emitted_codes() -> std::collections::BTreeSet<String> {
    let manifest = crate::test_paths::crate_root();
    let manifest = manifest.as_path();
    let mut files = Vec::new();
    rust_sources(&manifest.join("src"), &mut files);
    rust_sources(&manifest.join("../crates"), &mut files);
    rust_sources(&manifest.join("../scripts"), &mut files);
    let mut emitted = std::collections::BTreeSet::new();
    for file in files {
        if file.starts_with(manifest.join("src/gates")) {
            continue;
        }
        let source = std::fs::read_to_string(&file).unwrap();
        let code_text: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//") && !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        for (start, _) in code_text.match_indices("gates::tag(") {
            let rest = code_text[start + 11..].trim_start();
            if let Some(code) = rest.strip_prefix('"').and_then(|rest| rest.split('"').next()) {
                assert!(
                    find(code).is_some(),
                    "{}: gates::tag({code:?}) names no gate[...] declaration",
                    file.display()
                );
                emitted.insert(code.to_string());
            }
        }
        for row in REGISTRY {
            if code_text.contains(&format!("[{}]", row.code)) {
                emitted.insert(row.code.to_string());
            }
        }
    }
    emitted
}

/// Checks that only grant an exemption and never refuse on their own.
const NON_REFUSING: &[&str] = &["G-57"];

/// The registry is measured from code: every row comes from one declaration,
/// and every refusing check (gates and guards declared in Rust) emits its
/// code, so its refusals are recorded under that code and not as uncoded.
#[test]
fn every_registered_refusal_site_emits_its_code() {
    for family in [Family::Gate, Family::Guard, Family::Watchdog, Family::Notice, Family::Ci] {
        assert!(REGISTRY.iter().any(|row| row.family() == family), "no {family:?} row registered");
    }
    if !crate::test_paths::crate_root().join("src/gates").is_dir() {
        eprintln!("SKIP gate emission scan: source checkout unavailable in this test archive");
        return;
    }
    let emitted = emitted_codes();
    let silent: Vec<&str> = REGISTRY
        .iter()
        .filter(|row| (matches!(row.family(), Family::Gate | Family::Guard) && row.source.contains(".rs:"))
            || row.source.contains(".sh:") || row.source.contains(".py:"))
        .filter(|row| !emitted.contains(row.code) && !NON_REFUSING.contains(&row.code))
        .map(|row| row.source)
        .collect();
    assert!(silent.is_empty(), "refusing checks that never emit their code: {silent:#?}");

    // Replay the two uncoded script boundaries from the rejected delivery.
    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path();
    assert!(std::process::Command::new("git").args(["init", "-q"]).arg(repo).status().unwrap().success());
    assert!(std::process::Command::new("git").current_dir(repo)
        .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "--allow-empty", "-qm", "fixture"])
        .status().unwrap().success());
    assert!(std::process::Command::new("git").current_dir(repo).args(["checkout", "--detach", "-q"]).status().unwrap().success());
    let scripts = crate::test_paths::crate_root().join("../scripts");
    let release = std::process::Command::new("bash").arg(scripts.join("release.sh"))
        .current_dir(repo).output().unwrap();
    assert!(!release.status.success());
    assert!(codes_in(&String::from_utf8_lossy(&release.stderr)).contains(&"C-34"));
    let mut hook = std::process::Command::new("bash").arg(scripts.join("fast-rows-pre-push.sh"))
        .current_dir(repo).env_remove("CAS_SKIP_FAST_ROWS")
        .stdin(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    use std::io::Write;
    writeln!(hook.stdin.take().unwrap(), "refs/heads/factory/fixture {} refs/heads/factory/fixture {}", "1".repeat(40), "0".repeat(40)).unwrap();
    let hook = hook.wait_with_output().unwrap();
    assert!(!hook.status.success());
    assert!(codes_in(&String::from_utf8_lossy(&hook.stderr)).contains(&"H-38"));
}

/// A refusal, its override and a declared false positive are counted under
/// the refused code; a refusal with no code is counted as uncoded.
#[test]
fn refusals_overrides_and_false_positives_are_counted_per_code() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(dir.path()).unwrap();
    store.init().unwrap();
    let before = Utc::now() - chrono::Duration::seconds(1);
    let code = REGISTRY.iter().find(|row| row.family() == Family::Gate).unwrap().code;

    facts::record_refusal(&store, Some("cas-1"), &tag(code, "REFUSED")).unwrap();
    facts::record_refusal(&store, Some("cas-1"), "a refusal without a code").unwrap();
    facts::record_refusal(&store, Some("cas-1"), &tag(code, "REFUSED again")).unwrap();
    assert_eq!(facts::record_override(&store, "cas-1", "reviewed-drop: abc").unwrap(), vec![code]);
    facts::record_false_positive(&store, code, Some("cas-1"), "fixture was right").unwrap();
    assert!(facts::record_false_positive(&store, "G-999", None, "x").is_err());

    let counts = facts::summarize(&store, before).unwrap();
    assert_eq!(counts[code], facts::Counts { refused: 2, overridden: 1, false_positive: 1 });
    assert_eq!(counts[UNCODED].refused, 1);
    assert!(facts::summarize(&store, Utc::now() + chrono::Duration::seconds(5)).unwrap().is_empty());
}
