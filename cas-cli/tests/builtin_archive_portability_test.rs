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

// Archive-mode has no producer checkout to enumerate at runtime. The build
// script embeds every Rust file under cas-cli/tests/**/*.rs; when a checkout is
// available, the guard also enumerates it at runtime so a newly-added file is
// covered before the next rebuild.
include!(concat!(env!("OUT_DIR"), "/archive_fixture_sources.rs"));

// These are unit-test fixture sources outside cas-cli/tests/. They are not part
// of the generated integration-test manifest, but still need the fixture-parent
// scan because they contain test-only filesystem setup.
const SOURCE_FIXTURE_SOURCES: &[(&str, &str)] = &[
    source!(
        "cas-cli/src/mcp/tools/core/task/repo_context.rs",
        "../src/mcp/tools/core/task/repo_context.rs"
    ),
    source!(
        "cas-cli/src/store/known_repos.rs",
        "../src/store/known_repos.rs"
    ),
    source!(
        "cas-cli/src/worktree/discovery.rs",
        "../src/worktree/discovery.rs"
    ),
    source!(
        "cas-cli/src/ui/factory/app/render_and_ops/epic_workers.rs",
        "../src/ui/factory/app/render_and_ops/epic_workers.rs"
    ),
];

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
    if let Some(runtime_sources) = runtime_test_sources() {
        for (name, source) in runtime_sources {
            sources.insert(name, source);
        }
    }
    for (name, source) in SOURCE_FIXTURE_SOURCES {
        sources
            .entry((*name).to_string())
            .or_insert_with(|| (*source).to_string());
    }
    sources.into_iter().collect()
}

fn runtime_test_sources() -> Option<Vec<(String, String)>> {
    runtime_workspace_root().map(|root| discover_test_sources(&root))
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
            .find(|candidate| candidate.join("cas-cli/tests").is_dir())
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

fn scan_compile_time_checkout_reads<'a>(
    sources: impl IntoIterator<Item = &'a (String, String)>,
) -> Vec<String> {
    let needle = "env!(\"CARGO_MANIFEST_DIR\")";
    sources
        .into_iter()
        .flat_map(|(name, source)| {
            source
                .lines()
                .enumerate()
                .filter(|(_, line)| line.contains(needle))
                .map(move |(line_number, _)| format!("{name}:{}: {needle}", line_number + 1))
        })
        .filter(|violation| {
            let name = violation
                .split_once(':')
                .map(|(name, _)| name)
                .unwrap_or_default();
            !COMPILE_TIME_CHECKOUT_READ_ALLOWLIST
                .iter()
                .any(|(allowed_name, _)| *allowed_name == name)
        })
        .collect()
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
