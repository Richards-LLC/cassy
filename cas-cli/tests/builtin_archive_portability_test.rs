//! Archive portability guard for the builtin inspection tests.
//!
//! The tests in this list run from nextest archives, where the checkout is not
//! present. Their source inputs must therefore be embedded in the test binary
//! or supplied by Cassy's embedded builtin catalogs.

macro_rules! source {
    ($name:literal, $path:literal) => {
        ($name, include_str!($path))
    };
}

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const BUILTIN_INSPECTION_SOURCES: &[(&str, &str)] = &[
    source!(
        "cas-cli/tests/agent_definition_contract_test.rs",
        "agent_definition_contract_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_doc_hygiene_test.rs",
        "builtin_doc_hygiene_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_flavor_drift_test.rs",
        "builtin_flavor_drift_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_skill_description_test.rs",
        "builtin_skill_description_test.rs"
    ),
    source!(
        "cas-cli/tests/cas_image_generate_skill_test.rs",
        "cas_image_generate_skill_test.rs"
    ),
    source!(
        "cas-cli/tests/factory_codex_skill_guardrails.rs",
        "factory_codex_skill_guardrails.rs"
    ),
    source!(
        "cas-cli/tests/mcp_action_surface_test.rs",
        "mcp_action_surface_test.rs"
    ),
    source!(
        "cas-cli/tests/skill_hygiene_test.rs",
        "skill_hygiene_test.rs"
    ),
    source!(
        "cas-cli/tests/verify_before_claim_skill_test.rs",
        "verify_before_claim_skill_test.rs"
    ),
];

// Intentional compile-time checkout reads are prohibited today. If an
// archive-safe exception is ever required, add its source path and a reason
// here so the exception remains visible in review.
const COMPILE_TIME_CHECKOUT_READ_ALLOWLIST: &[(&str, &str)] = &[];

// These sources contain test-only fixture setup that predates the generated
// checkout-read manifest. Keep this small, purpose-specific set for the
// separate fixture-parent check; the compile-time checkout scan below covers
// every generated test-bearing source.
const FIXTURE_PARENT_SOURCE_PATHS: &[&str] = &[
    "cas-cli/src/mcp/tools/core/task/repo_context.rs",
    "cas-cli/src/store/known_repos.rs",
    "cas-cli/src/worktree/discovery.rs",
    "cas-cli/src/ui/factory/app/render_and_ops/epic_workers.rs",
];

// Archive-mode has no producer checkout to enumerate at runtime. The build
// script embeds every Rust file under cas-cli/tests/**/*.rs and every
// test-bearing Rust source under cas-cli/src/** and crates/*/src/**. When a
// checkout is available, the guard also enumerates those roots at runtime so
// a newly-added file is covered before the next rebuild.
include!(concat!(env!("OUT_DIR"), "/archive_fixture_sources.rs"));

#[test]
fn builtin_inspection_tests_do_not_depend_on_the_checkout_at_runtime() {
    let fixture_sources = fixture_sources();
    assert_allowlist_is_live(&fixture_sources);

    let forbidden = [
        "env!(\"CARGO_MANIFEST_DIR\")",
        "cas::test_paths::workspace_root()",
    ];
    let mut violations = Vec::new();
    for (name, source) in BUILTIN_INSPECTION_SOURCES {
        for (line_number, line) in source.lines().enumerate() {
            for needle in forbidden {
                let guarded_checkout_probe =
                    needle == "cas::test_paths::workspace_root()" && source.contains("SKIP ");
                if line.contains(needle) && !guarded_checkout_probe {
                    violations.push(format!("{name}:{}: {needle}", line_number + 1));
                }
            }
        }
    }
    violations.extend(scan_compile_time_checkout_reads(
        fixture_sources
            .iter()
            .filter(|(name, _)| name.starts_with("cas-cli/tests/")),
    ));

    let fixture_constructors = [concat!("tempdir", "_in("), concat!("TempDir", "::new_in(")];
    let forbidden_fixture_parents = [
        concat!("env!(\"", "CARGO_MANIFEST_DIR"),
        concat!("\"/", "tmp"),
        concat!("\"/var/", "tmp"),
    ];
    let fixture_source_is_forbidden = |source: &str| {
        fixture_constructors
            .iter()
            .any(|constructor| source.contains(constructor))
            && forbidden_fixture_parents
                .iter()
                .any(|parent| source.contains(parent))
    };
    for unsafe_fixture in [
        concat!("tempdir", "_in(Path::new(\"/", "tmp\"))"),
        concat!("TempDir", "::new_in(\"/var/", "tmp\")"),
        concat!("tempdir", "_in(\n env!(\"", "CARGO_MANIFEST_DIR", "\"))"),
    ] {
        assert!(
            fixture_source_is_forbidden(unsafe_fixture),
            "fixture source scan does not cover {unsafe_fixture}"
        );
    }
    for allowed_text in [
        "documentation says /tmp is disposable",
        "assert!(path.starts_with(\"/var/tmp\"))",
        "cas::test_paths::runtime_fixture_parent()",
    ] {
        assert!(
            !fixture_source_is_forbidden(allowed_text),
            "fixture source scan false-positive: {allowed_text}"
        );
    }
    for (name, source) in &fixture_sources {
        if !name.starts_with("cas-cli/tests/")
            && !FIXTURE_PARENT_SOURCE_PATHS.contains(&name.as_str())
        {
            continue;
        }
        let lines: Vec<_> = source.lines().collect();
        for (line_number, line) in lines.iter().enumerate() {
            if fixture_constructors
                .iter()
                .any(|constructor| line.contains(constructor))
            {
                let snippet = lines[line_number..lines.len().min(line_number + 4)].join("\n");
                if fixture_source_is_forbidden(&snippet) {
                    violations.push(format!(
                        "{name}:{}: forbidden fixture parent",
                        line_number + 1
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "real-project fixtures must use cas::test_paths::runtime_fixture_parent(); \
         archive tests must not resolve source files from the producer checkout:\n  {}",
        violations.join("\n  ")
    );
}

fn fixture_sources() -> Vec<(String, String)> {
    let mut sources = BTreeMap::new();
    for (name, source) in GENERATED_FIXTURE_SOURCES {
        sources.insert((*name).to_string(), (*source).to_string());
    }
    if let Some(runtime_sources) = runtime_workspace_sources() {
        for (name, source) in runtime_sources {
            sources.insert(name, source);
        }
    }
    sources.into_iter().collect()
}

fn runtime_workspace_sources() -> Option<Vec<(String, String)>> {
    runtime_workspace_root().map(|root| discover_workspace_sources(&root))
}

fn runtime_workspace_root() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for key in ["CAS_TEST_WORKSPACE_ROOT", "NEXTEST_WORKSPACE_ROOT"] {
        if let Some(path) = std::env::var_os(key).map(PathBuf::from) {
            candidates.push(path);
        }
    }
    if let Ok(path) = std::env::current_dir() {
        candidates.push(path);
    }
    if let Ok(path) = std::env::current_exe() {
        candidates.extend(path.ancestors().map(Path::to_path_buf));
    }
    candidates.into_iter().find_map(|base| {
        base.ancestors()
            .find(|candidate| {
                candidate.join("cas-cli/tests").is_dir() || candidate.join("cas-cli/src").is_dir()
            })
            .map(Path::to_path_buf)
    })
}

fn discover_test_sources(workspace_root: &Path) -> Vec<(String, String)> {
    let tests_dir = workspace_root.join("cas-cli/tests");
    let mut paths = Vec::new();
    collect_test_sources(&tests_dir, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&tests_dir)
                .expect("test source is beneath tests directory");
            let name = format!(
                "cas-cli/tests/{}",
                relative.to_string_lossy().replace('\\', "/")
            );
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read test source {}: {error}", path.display()));
            (name, source)
        })
        .collect()
}

fn discover_workspace_sources(workspace_root: &Path) -> Vec<(String, String)> {
    let tests_dir = workspace_root.join("cas-cli/tests");
    let mut sources = if tests_dir.is_dir() {
        discover_test_sources(workspace_root)
    } else {
        Vec::new()
    };
    discover_source_tree(
        &workspace_root.join("cas-cli/src"),
        "cas-cli/src",
        &mut sources,
    );

    let crates_dir = workspace_root.join("crates");
    if let Ok(entries) = fs::read_dir(&crates_dir) {
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!(
                    "read crate source root entry in {}: {error}",
                    crates_dir.display()
                )
            });
            let crate_root = entry.path();
            let Some(crate_name) = crate_root.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if crate_root.is_dir() {
                discover_source_tree(
                    &crate_root.join("src"),
                    &format!("crates/{crate_name}/src"),
                    &mut sources,
                );
            }
        }
    }
    sources
}

fn discover_source_tree(source_dir: &Path, archive_root: &str, output: &mut Vec<(String, String)>) {
    let mut paths = Vec::new();
    collect_test_bearing_sources(source_dir, &mut paths);
    paths.sort();
    for path in paths {
        let relative = path
            .strip_prefix(source_dir)
            .expect("source is beneath source directory");
        let name = format!(
            "{archive_root}/{}",
            relative.to_string_lossy().replace('\\', "/")
        );
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read source file {}: {error}", path.display()));
        output.push((name, source));
    }
}

fn collect_test_sources(directory: &Path, output: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        panic!(
            "read test source directory {}: {error}",
            directory.display()
        )
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!("read test source entry in {}: {error}", directory.display())
        });
        let path = entry.path();
        if path.is_dir() {
            collect_test_sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

fn collect_test_bearing_sources(directory: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!("read source entry in {}: {error}", directory.display())
        });
        let path = entry.path();
        if path.is_dir() {
            collect_test_bearing_sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read source file {}: {error}", path.display()));
            if contains_test_block(&source) {
                output.push(path);
            }
        }
    }
}

fn contains_test_block(source: &str) -> bool {
    source.contains("#[cfg(test") || source.contains("#[test")
}

fn scan_compile_time_checkout_reads<'a>(
    sources: impl IntoIterator<Item = &'a (String, String)>,
) -> Vec<String> {
    let mut violations: Vec<_> = sources
        .into_iter()
        .flat_map(|(name, source)| scan_test_block_checkout_reads(name, source))
        .filter(|violation| {
            let name = violation
                .split_once(':')
                .map(|(name, _)| name)
                .unwrap_or_default();
            !COMPILE_TIME_CHECKOUT_READ_ALLOWLIST
                .iter()
                .any(|(allowed_name, _)| *allowed_name == name)
        })
        .collect();
    violations.sort_unstable();
    violations.dedup();
    violations
}

fn scan_test_block_checkout_reads(name: &str, source: &str) -> Vec<String> {
    let masked = mask_non_code(source);
    let mut violations = Vec::new();
    let ranges = if name.starts_with("cas-cli/tests/") {
        vec![(0, masked.len())]
    } else {
        test_block_ranges(&masked)
    };
    for (start, end) in ranges {
        let block = &masked[start..end];
        for (offset, _) in block.match_indices("env") {
            let before = block[..offset].chars().next_back();
            let after = block[offset + 3..].chars().next();
            if before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
                || after
                    .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
            {
                continue;
            }
            let compact = compact_ascii_whitespace(&source[start + offset..]);
            if !compact.starts_with("env!(\"CARGO_MANIFEST_DIR\")") {
                continue;
            }
            let byte_offset = start + offset;
            let line_number = source[..byte_offset]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            violations.push(format!(
                "{name}:{line_number}: env!(\"CARGO_MANIFEST_DIR\")"
            ));
        }
    }
    violations
}

fn compact_ascii_whitespace(source: &str) -> String {
    source
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect()
}

fn test_block_ranges(masked: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut search_from = 0;
    while let Some(relative) = masked[search_from..].find("#[") {
        let attribute_start = search_from + relative;
        let Some(attribute_end) = masked[attribute_start..].find(']') else {
            break;
        };
        let attribute_end = attribute_start + attribute_end + 1;
        let attribute = &masked[attribute_start..attribute_end];
        if attribute.contains("test") {
            if let Some(open_relative) = masked[attribute_end..].find('{') {
                let open = attribute_end + open_relative;
                if let Some(close) = matching_brace(masked, open) {
                    ranges.push((attribute_start, close + 1));
                }
            }
        }
        search_from = attribute_end;
    }
    ranges
}

fn matching_brace(masked: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (offset, byte) in masked.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn mask_non_code(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut masked = bytes.to_vec();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                masked[index] = b' ';
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let mut depth = 1;
            masked[index] = b' ';
            masked[index + 1] = b' ';
            index += 2;
            while index < bytes.len() && depth > 0 {
                if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
                    depth += 1;
                    masked[index] = b' ';
                    masked[index + 1] = b' ';
                    index += 2;
                } else if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    depth -= 1;
                    masked[index] = b' ';
                    masked[index + 1] = b' ';
                    index += 2;
                } else {
                    if bytes[index] != b'\n' {
                        masked[index] = b' ';
                    }
                    index += 1;
                }
            }
            continue;
        }
        if bytes[index] == b'\'' || bytes[index] == b'"' {
            index = mask_quoted(bytes, &mut masked, index, bytes[index]);
            continue;
        }
        if bytes[index] == b'r' || (bytes[index] == b'b' && bytes.get(index + 1) == Some(&b'r')) {
            let quote = if bytes[index] == b'r' {
                index + 1
            } else {
                index + 2
            };
            if bytes.get(quote) == Some(&b'#') || bytes.get(quote) == Some(&b'"') {
                if let Some(end) = raw_string_end(bytes, quote) {
                    for position in index..=end {
                        if bytes[position] != b'\n' {
                            masked[position] = b' ';
                        }
                    }
                    index = end + 1;
                    continue;
                }
            }
        }
        index += 1;
    }
    String::from_utf8(masked).expect("source masking preserves UTF-8 bytes")
}

fn mask_quoted(bytes: &[u8], masked: &mut [u8], start: usize, quote: u8) -> usize {
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            masked[index] = b' ';
            if index + 1 < bytes.len() {
                if bytes[index + 1] != b'\n' {
                    masked[index + 1] = b' ';
                }
                index += 2;
            } else {
                index += 1;
            }
        } else {
            let end = bytes[index] == quote;
            if bytes[index] != b'\n' {
                masked[index] = b' ';
            }
            index += 1;
            if end {
                break;
            }
        }
    }
    if bytes[start] != b'\n' {
        masked[start] = b' ';
    }
    index
}

fn raw_string_end(bytes: &[u8], quote: usize) -> Option<usize> {
    let mut hashes = 0;
    let mut position = quote;
    while bytes.get(position) == Some(&b'#') {
        hashes += 1;
        position += 1;
    }
    if bytes.get(position) != Some(&b'"') {
        return None;
    }
    position += 1;
    while position < bytes.len() {
        let closing_hashes = position + 1 + hashes;
        if bytes[position] == b'"'
            && closing_hashes <= bytes.len()
            && bytes[position + 1..closing_hashes]
                .iter()
                .all(|byte| *byte == b'#')
        {
            return Some(closing_hashes - 1);
        }
        position += 1;
    }
    None
}

fn assert_allowlist_is_live(sources: &[(String, String)]) {
    let names: BTreeSet<_> = sources.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names.len(),
        sources.len(),
        "duplicate fixture source paths must not be silently merged"
    );
    for (allowed_name, reason) in COMPILE_TIME_CHECKOUT_READ_ALLOWLIST {
        assert!(
            !allowed_name.is_empty() && !reason.trim().is_empty(),
            "compile-time checkout read allowlist entries require a source path and reason"
        );
        assert!(
            names.contains(allowed_name),
            "stale compile-time checkout read allowlist entry: {allowed_name}"
        );
    }
}

#[test]
fn runtime_source_discovery_catches_an_unlisted_checkout_read() {
    let workspace = tempfile::tempdir().expect("create synthetic workspace");
    let tests_dir = workspace.path().join("cas-cli/tests");
    fs::create_dir_all(&tests_dir).expect("create synthetic test directory");
    let forbidden = concat!("env!", "(\"", "CARGO_MANIFEST_DIR", "\")");
    fs::write(
        tests_dir.join("unlisted.rs"),
        format!("const BAD: &str = {forbidden};\n"),
    )
    .expect("write synthetic unlisted test");

    let sources = discover_test_sources(workspace.path());
    let violations = scan_compile_time_checkout_reads(&sources);
    assert_eq!(
        violations,
        vec!["cas-cli/tests/unlisted.rs:1: env!(\"CARGO_MANIFEST_DIR\")"],
        "runtime discovery must scan a test file absent from any embedded manifest"
    );
}

#[test]
fn runtime_source_discovery_catches_an_unlisted_lib_test_checkout_read() {
    let workspace = tempfile::tempdir().expect("create synthetic workspace");
    let source_dir = workspace.path().join("cas-cli/src");
    fs::create_dir_all(&source_dir).expect("create synthetic source directory");
    fs::write(
        source_dir.join("unlisted.rs"),
        r#"const PRODUCTION_TEXT: &str = "env!(\"CARGO_MANIFEST_DIR\")";
#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_checkout_file() {
        let _ = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/check.sh"));
    }
}
"#,
    )
    .expect("write synthetic lib source");

    let sources = discover_workspace_sources(workspace.path());
    let violations = scan_compile_time_checkout_reads(&sources);
    assert_eq!(
        violations,
        vec!["cas-cli/src/unlisted.rs:6: env!(\"CARGO_MANIFEST_DIR\")"],
        "runtime discovery must scan a lib test file absent from any embedded manifest"
    );
}

#[test]
fn checkout_scan_ignores_production_text_and_embedded_include_str() {
    let source = r#"const PRODUCTION_TEXT: &str = "env!(\"CARGO_MANIFEST_DIR\")";
#[cfg(test)]
mod tests {
    const BUILTIN: &str = include_str!("builtins/skills/cas-worker.md");
}
"#;
    assert!(
        scan_compile_time_checkout_reads(&[(
            "cas-cli/src/embedded.rs".to_string(),
            source.to_string(),
        )])
        .is_empty()
    );
}
