//! Source-scoped JavaScript stability evidence. A failure is never retried.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tree_sitter::{Node, Parser};

use super::stability::{Identity, Proof};

#[derive(Debug, thiserror::Error)]
#[error("Worker check requires an isolated worktree or a managed review checkout")]
struct IsolatedCheckoutRequired;

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && !path.starts_with('-')
}

fn javascript(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|ext| ext.to_str()),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts")
    )
}

pub(super) fn allowed_args(args: &[String]) -> bool {
    args.len() > 1
        && matches!(args[0].as_str(), "vitest" | "jest" | "playwright")
        && args[1..]
            .iter()
            .all(|path| safe_path(path) && javascript(path))
}

/// Only hunks intersecting a test function (or its suite title) select it.
/// Dynamic titles cannot safely become a runtime selector and fail closed.
pub(crate) fn changed_tests(
    repo: &Path,
    base: &str,
    head: &str,
    paths: &[String],
) -> Result<Vec<Identity>> {
    let mut selected = BTreeSet::new();
    for path in paths.iter().filter(|path| javascript(path)) {
        if !safe_path(path) {
            bail!("invalid JavaScript stability path: {path}");
        }
        if super::git(repo, &["ls-tree", "--name-only", head, "--", path])?.is_empty() {
            continue;
        }
        let output = Command::new("git")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .current_dir(repo)
            .args(["show", &format!("{head}:{path}")])
            .output()?;
        if !output.status.success() {
            bail!("cannot read {head}:{path}");
        }
        let source = String::from_utf8(output.stdout)?;
        let diff = super::git(
            repo,
            &[
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--unified=0",
                base,
                head,
                "--",
                path,
            ],
        )?;
        let old = Command::new("git")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .current_dir(repo)
            .args(["show", &format!("{base}:{path}")])
            .output()?;
        let old = if old.status.success() {
            String::from_utf8(old.stdout)?
        } else {
            String::new()
        };
        let all = parsed_tests(path, &source, &[(1, usize::MAX)], false)?;
        let stem = Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        let test_file = stem.ends_with(".test")
            || stem.ends_with(".spec")
            || path.split('/').any(|part| part == "__tests__");
        if test_file && all.is_empty() && !source.trim().is_empty() && !diff.is_empty() {
            bail!(
                "cannot resolve tests in {path}: test factories require explicit source-to-runtime identities"
            );
        }
        let previous: BTreeMap<_, _> = parsed_tests(path, &old, &[(1, usize::MAX)], false)?
            .into_iter()
            .collect();
        for (identity, body) in parsed_tests(path, &source, &changed_lines(&diff)?, true)? {
            // Hunk lines may contain several calls. Repeating an unchanged
            // neighbour would violate the delivery-only selection contract.
            if previous.get(&identity) != Some(&body) {
                selected.insert(identity);
            }
        }
    }
    Ok(selected.into_iter().collect())
}

fn changed_lines(diff: &str) -> Result<Vec<(usize, usize)>> {
    diff.lines()
        .filter(|line| line.starts_with("@@ "))
        .map(|line| {
            let range = line
                .split_whitespace()
                .nth(2)
                .context("git hunk new range")?
                .strip_prefix('+')
                .context("git hunk new range prefix")?;
            let (start, count) = range.split_once(',').unwrap_or((range, "1"));
            let start: usize = start.parse()?;
            let count: usize = count.parse()?;
            // A pure deletion inside a surviving test still changes that test.
            Ok((start, start + count.saturating_sub(1)))
        })
        .collect()
}

fn overlaps(node: Node<'_>, lines: &[(usize, usize)]) -> bool {
    lines.iter().any(|(start, end)| {
        *start <= node.end_position().row + 1 && *end >= node.start_position().row + 1
    })
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

/// Decode static strings only. Unsupported JavaScript escapes fail closed.
fn title(node: Node<'_>, source: &str) -> Option<String> {
    if !matches!(node.kind(), "string" | "template_string") {
        return None;
    }
    let raw = text(node, source);
    let contents = raw.get(1..raw.len().checked_sub(1)?)?;
    if raw.starts_with('"') {
        return serde_json::from_str(raw).ok();
    }
    if contents.contains('\\') || (raw.starts_with('`') && contents.contains("${")) {
        return None;
    }
    Some(contents.to_string())
}

fn declaration(
    node: Node<'_>,
    source: &str,
    aliases: &BTreeMap<String, String>,
) -> Option<(bool, bool)> {
    let function = node.child_by_field_name("function")?;
    let name = text(function, source);
    let parts: Vec<_> = name.split('.').collect();
    let raw_root = name
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .next()?;
    let root = aliases
        .get(raw_root)
        .map(String::as_str)
        .unwrap_or(raw_root);
    if !matches!(root, "test" | "it" | "describe") {
        return None;
    }
    let suite = root == "describe" || parts.get(1) == Some(&"describe");
    let offset = if root == "test" && suite { 2 } else { 1 };
    if parts[0] == raw_root
        && parts[offset..].iter().all(|part| {
            matches!(
                *part,
                "only"
                    | "skip"
                    | "todo"
                    | "concurrent"
                    | "sequential"
                    | "serial"
                    | "parallel"
                    | "fails"
                    | "fail"
                    | "fixme"
            )
        })
    {
        Some((suite, true))
    } else if parts.last().is_some_and(|part| {
        matches!(
            *part,
            "configure"
                | "use"
                | "extend"
                | "info"
                | "setTimeout"
                | "step"
                | "slow"
                | "beforeAll"
                | "afterAll"
                | "beforeEach"
                | "afterEach"
        )
    }) {
        None
    } else {
        Some((suite, false))
    }
}

#[cfg(test)]
fn source_tests(path: &str, source: &str, lines: &[(usize, usize)]) -> Result<Vec<Identity>> {
    Ok(parsed_tests(path, source, lines, true)?
        .into_iter()
        .map(|(identity, _)| identity)
        .collect())
}

fn parsed_tests(
    path: &str,
    source: &str,
    lines: &[(usize, usize)],
    strict: bool,
) -> Result<Vec<(Identity, String)>> {
    let mut parser = Parser::new();
    let language = if path.ends_with("tsx") || path.ends_with("jsx") {
        tree_sitter_typescript::LANGUAGE_TSX
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT
    };
    parser.set_language(&language.into())?;
    let tree = parser
        .parse(source, None)
        .context("JavaScript test parse")?;
    if tree.root_node().has_error() {
        bail!("cannot resolve changed tests in {path}: JavaScript parse error");
    }
    let mut aliases = BTreeMap::new();
    imported_aliases(tree.root_node(), source, &mut aliases);
    if strict {
        reject_retry_configuration(tree.root_node(), source, path, &aliases, false)?;
    }
    let mut tests = Vec::new();
    walk(
        tree.root_node(),
        source,
        path,
        lines,
        &[],
        false,
        false,
        strict,
        &aliases,
        &mut tests,
    )?;
    Ok(tests)
}

fn imported_aliases(node: Node<'_>, source: &str, aliases: &mut BTreeMap<String, String>) {
    if node.kind() == "import_specifier"
        && let Some(name) = node.child_by_field_name("name")
    {
        let name = text(name, source);
        if matches!(name, "test" | "it" | "describe")
            && let Some(alias) = node.child_by_field_name("alias")
        {
            aliases.insert(text(alias, source).to_string(), name.to_string());
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        imported_aliases(child, source, aliases);
    }
}

fn reject_retry_configuration(
    node: Node<'_>,
    source: &str,
    path: &str,
    aliases: &BTreeMap<String, String>,
    test_options: bool,
) -> Result<()> {
    if node.kind() == "pair"
        && let Some(key) = node.child_by_field_name("key")
    {
        let key = text(key, source).trim_matches(['\'', '"']);
        let mut call = node.parent();
        while call.is_some_and(|parent| parent.kind() != "call_expression") {
            call = call.and_then(|parent| parent.parent());
        }
        let runner_option = call.is_some_and(|call| {
            (test_options && declaration(call, source, aliases).is_some())
                || call
                    .child_by_field_name("function")
                    .is_some_and(|function| text(function, source) == "test.describe.configure")
        });
        if runner_option
            && matches!(key, "retry" | "retries")
            && node
                .child_by_field_name("value")
                .is_none_or(|value| text(value, source) != "0")
        {
            bail!(
                "{path}: nonzero or dynamic retry configuration cannot supply stability evidence; disable retries"
            );
        }
    }
    if node.kind() == "call_expression"
        && node
            .child_by_field_name("function")
            .is_some_and(|function| text(function, source).ends_with(".retryTimes"))
    {
        let count = node
            .child_by_field_name("arguments")
            .and_then(|args| args.named_child(0));
        if count.is_none_or(|value| text(value, source) != "0") {
            bail!("{path}: jest.retryTimes must be zero for stability evidence");
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        reject_retry_configuration(child, source, path, aliases, test_options)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn walk(
    node: Node<'_>,
    source: &str,
    path: &str,
    lines: &[(usize, usize)],
    suites: &[String],
    unresolved: bool,
    force: bool,
    strict: bool,
    aliases: &BTreeMap<String, String>,
    tests: &mut Vec<(Identity, String)>,
) -> Result<()> {
    if node.kind() == "call_expression"
        && let Some((suite, supported)) = declaration(node, source, aliases)
    {
        let args = node
            .child_by_field_name("arguments")
            .context("test arguments")?;
        let name = args.named_child(0).and_then(|name| title(name, source));
        let unresolved = unresolved || !supported || name.is_none();
        let mut cursor = args.walk();
        let callback = args
            .named_children(&mut cursor)
            .find(|arg| matches!(arg.kind(), "arrow_function" | "function_expression"));
        let mut cursor = args.walk();
        let external_callback = callback.is_none()
            && args
                .named_children(&mut cursor)
                .skip(1)
                .any(|arg| arg.kind() == "identifier");
        if strict && (external_callback || (suite && supported && callback.is_none())) {
            bail!(
                "cannot resolve changed test scope at {path}:{}: out-of-line callbacks require explicit source-to-runtime identities",
                node.start_position().row + 1
            );
        }
        if suite {
            let mut nested = suites.to_vec();
            if let Some(name) = name {
                nested.push(name);
            }
            let header_changed = lines.iter().any(|(start, end)| {
                *end >= node.start_position().row + 1
                    && *start
                        <= callback.map_or(node.start_position().row + 1, |cb| {
                            cb.start_position().row + 1
                        })
            });
            if let Some(callback) = callback {
                return walk(
                    callback,
                    source,
                    path,
                    lines,
                    &nested,
                    unresolved,
                    force || header_changed,
                    strict,
                    aliases,
                    tests,
                );
            }
        } else {
            if force || overlaps(node, lines) {
                if strict {
                    reject_retry_configuration(node, source, path, aliases, true)?;
                }
                if unresolved {
                    if !strict {
                        return Ok(());
                    }
                    bail!(
                        "cannot resolve changed test at {path}:{}: dynamic/parameterized test title; provide a static test identity",
                        node.start_position().row + 1
                    );
                }
                let mut names = suites.to_vec();
                names.push(name.expect("resolved title"));
                tests.push((
                    Identity {
                        path: path.to_string(),
                        name: names.join(" "),
                    },
                    text(node, source).to_string(),
                ));
            }
            // Test bodies cannot declare additional runnable tests.
            return Ok(());
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(
            child, source, path, lines, suites, unresolved, force, strict, aliases, tests,
        )?;
    }
    Ok(())
}

pub(super) fn execute(cas_root: &Path, args: &[String], cwd: &Path) -> Result<()> {
    if !allowed_args(args) {
        bail!("expected vitest|jest|playwright followed by repository-relative test files");
    }
    let repo = PathBuf::from(super::git(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let cas_root = cas_root.canonicalize()?;
    if !repo.starts_with(cas_root.join("worktrees"))
        && !crate::factory_review_scratch::is_managed_checkout(&cas_root, &repo)
    {
        return Err(IsolatedCheckoutRequired.into());
    }
    let cas_root = cas_root.as_path();
    let head = super::clean_head(&repo)?;
    let config = crate::config::Config::load(cas_root)?.factory();
    let base = super::proof_diff_base(cas_root, &repo, config.epic_base_branch.as_deref())?;
    let tests = changed_tests(&repo, &base, &head, &args[1..])?;
    if tests.is_empty() {
        bail!("no changed JavaScript tests matched the supplied files");
    }
    let _lane =
        super::try_lock_lane_with_command(cas_root, &repo, &super::worker_check_command(args))?
            .ok_or_else(|| {
                super::lane_lock_refusal(
                    &super::lane_lock_path(cas_root, &repo).unwrap_or_default(),
                )
            })?;
    let _slot = super::admit_daemon_build(cas_root).map_err(|cause| {
        super::recorded_refusal(
            cas_root,
            &repo,
            &cause,
            crate::factory_capacity_wait::CapacityKind::Other,
        )
    })?;
    super::stability::invalidate(cas_root, &repo, &head, &tests)?;
    let proof = execute_tests(cas_root, &repo, &head, &args[0], &tests)?;
    println!(
        "stability: PASS {head} {} {} tests 5/5 in {} ms",
        proof.runner,
        proof.tests.len(),
        proof.elapsed_ms
    );
    Ok(())
}

fn package_root(repo: &Path, path: &str) -> Result<PathBuf> {
    let mut current = repo
        .join(path)
        .parent()
        .context("test parent")?
        .to_path_buf();
    loop {
        if current.join("package.json").is_file() {
            return Ok(current);
        }
        if current == repo || !current.pop() {
            bail!("no package.json found for {path}");
        }
    }
}

fn runner_path(repo: &Path, package: &Path, runner: &str) -> Result<PathBuf> {
    let mut current = package.to_path_buf();
    loop {
        let executable = current.join("node_modules/.bin").join(runner);
        if executable.is_file() {
            return Ok(executable);
        }
        if current == repo || !current.pop() {
            bail!(
                "{runner} is not installed locally for {}",
                package.display()
            );
        }
    }
}

fn execute_tests(
    cas_root: &Path,
    repo: &Path,
    head: &str,
    runner: &str,
    tests: &[Identity],
) -> Result<Proof> {
    let start = Instant::now();
    let config = crate::config::Config::load(cas_root)?.factory();
    let high = crate::factory_build_memory::memory_high(
        config.worker_memory_high_gib,
        config.max_concurrent_builders,
        &std::fs::read_to_string("/proc/meminfo").unwrap_or_default(),
    );
    let mut by_file: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for test in tests {
        by_file.entry(&test.path).or_default().push(&test.name);
    }
    let reports = tempfile::tempdir()?;
    for (path, names) in by_file {
        let package = package_root(repo, path)?;
        let executable = runner_path(repo, &package, runner)?;
        let pattern = names
            .iter()
            .map(|name| format!("(?:{})", regex::escape(name)))
            .collect::<Vec<_>>()
            .join("|");
        let pattern = if runner == "playwright" {
            format!("(?:^|\\s)(?:{pattern})$")
        } else {
            format!("^(?:{pattern})$")
        };
        let selected: Vec<_> = names
            .iter()
            .map(|name| Identity {
                path: path.to_string(),
                name: (*name).to_string(),
            })
            .collect();
        verify_repeats(runner, repo, &selected, |iteration| {
            let report = reports.path().join(format!("report-{iteration}.json"));
            let mut command = Command::new(&executable);
            command.current_dir(&package).env("CI", "1");
            let file = repo.join(path);
            match runner {
                "vitest" => {
                    command
                        .args([
                            "run",
                            "--retry",
                            "0",
                            "--bail",
                            "1",
                            "--maxWorkers",
                            "1",
                            "--maxConcurrency",
                            "1",
                            "--no-file-parallelism",
                            "--reporter=json",
                            "--outputFile",
                        ])
                        .arg(&report)
                        .arg("--testNamePattern")
                        .arg(&pattern)
                        .arg(&file);
                }
                "jest" => {
                    command
                        .args([
                            "--runInBand",
                            "--maxConcurrency=1",
                            "--bail=1",
                            "--json",
                            "--outputFile",
                        ])
                        .arg(&report)
                        .arg("--testNamePattern")
                        .arg(&pattern)
                        .arg("--runTestsByPath")
                        .arg(&file);
                }
                "playwright" => {
                    command
                        .args([
                            "test",
                            "--repeat-each",
                            "5",
                            "--retries",
                            "0",
                            "--max-failures",
                            "1",
                            "--workers",
                            "1",
                            "--reporter=json",
                            "--output",
                        ])
                        .arg(reports.path().join("playwright-output"))
                        .arg("--grep")
                        .arg(&pattern)
                        .arg(regex::escape(&file.to_string_lossy()))
                        .env("PLAYWRIGHT_JSON_OUTPUT_FILE", &report);
                }
                _ => bail!("unsupported stability runner: {runner}"),
            }
            let marker = reports.path().join(format!("scope-{iteration}"));
            // A stale marker could mistake admission failure for test execution.
            let _ = std::fs::remove_file(&marker);
            let mut command = crate::factory_build_memory::scope_command(
                command,
                cfg!(target_os = "linux").then_some(Path::new("systemd-run")),
                high,
                &marker,
            );
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            #[cfg(unix)]
            let _signals = super::WorkerWaitSignals::install()?;
            let mut child = command
                .spawn()
                .with_context(|| format!("start {runner} stability run"))?;
            #[cfg(unix)]
            let status = super::wait_for_worker_command(&mut child)?;
            #[cfg(not(unix))]
            let status = child.wait()?;
            let value: Value =
                serde_json::from_slice(&std::fs::read(&report).with_context(|| {
                    format!("{runner} did not produce a stability report for {path}")
                })?)?;
            Ok((status.success(), value))
        })?;
    }
    if super::clean_head(repo)? != head {
        bail!("repository changed during JavaScript stability run");
    }
    let proof = Proof {
        runner: runner.to_string(),
        tests: tests.to_vec(),
        consecutive_passes: 5,
        elapsed_ms: start.elapsed().as_millis(),
    };
    super::stability::record(cas_root, repo, head, proof.clone())?;
    Ok(proof)
}

fn verify_repeats(
    runner: &str,
    repo: &Path,
    tests: &[Identity],
    mut run: impl FnMut(usize) -> Result<(bool, Value)>,
) -> Result<()> {
    for iteration in 0..if runner == "playwright" { 1 } else { 5 } {
        let (success, report) = run(iteration)?;
        validate_report(
            runner,
            repo,
            &report,
            tests,
            if runner == "playwright" { 5 } else { 1 },
        )?;
        if !success {
            bail!(
                "{runner} stability failed at iteration {}; no retry",
                iteration + 1
            );
        }
    }
    Ok(())
}

fn validate_report(
    runner: &str,
    repo: &Path,
    report: &Value,
    tests: &[Identity],
    repeats: usize,
) -> Result<()> {
    let mut results: BTreeMap<Identity, usize> = BTreeMap::new();
    if runner == "playwright" {
        let root = report
            .get("config")
            .and_then(|config| config.get("rootDir"))
            .and_then(Value::as_str)
            .map(Path::new)
            .unwrap_or(repo);
        let mut projects = BTreeMap::new();
        playwright_results(
            repo,
            root,
            report.get("suites").context("Playwright suites missing")?,
            &[],
            &mut projects,
        )?;
        for ((identity, _project), count) in projects {
            if count != repeats {
                bail!(
                    "playwright requires {repeats} passing repeats per project: {} :: {}",
                    identity.path,
                    identity.name
                );
            }
            results.insert(identity, count);
        }
        if report
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| !errors.is_empty())
        {
            bail!("Playwright stability report contains runner errors");
        }
    } else {
        for file in report
            .get("testResults")
            .and_then(Value::as_array)
            .context("testResults missing")?
        {
            let path = report_path(
                repo,
                file.get("name")
                    .and_then(Value::as_str)
                    .context("test file missing")?,
            )?;
            for test in file
                .get("assertionResults")
                .and_then(Value::as_array)
                .context("assertionResults missing")?
            {
                let name = test
                    .get("fullName")
                    .and_then(Value::as_str)
                    .context("test fullName missing")?;
                let id = Identity {
                    path: path.clone(),
                    name: name.to_string(),
                };
                match test.get("status").and_then(Value::as_str) {
                    Some("passed") => {
                        if test
                            .get("invocations")
                            .and_then(Value::as_u64)
                            .is_some_and(|count| count != 1)
                            || test
                                .get("retryReasons")
                                .and_then(Value::as_array)
                                .is_some_and(|reasons| !reasons.is_empty())
                        {
                            bail!(
                                "{runner} retried a failing test: {} :: {}",
                                id.path,
                                id.name
                            );
                        }
                        *results.entry(id).or_default() += 1;
                    }
                    Some("pending" | "skipped" | "todo" | "disabled") if !tests.contains(&id) => {}
                    _ => bail!("{runner} stability failed: {} :: {}", id.path, id.name),
                }
            }
            if file.get("status").and_then(Value::as_str) == Some("failed") {
                bail!("{runner} suite failed: {path}");
            }
        }
        if report.get("success").and_then(Value::as_bool) != Some(true) {
            bail!("{runner} stability report was unsuccessful");
        }
    }
    for test in tests {
        if results.get(test) != Some(&repeats) {
            bail!(
                "{runner} stability requires {repeats} passing results: {} :: {}",
                test.path,
                test.name
            );
        }
    }
    if results.keys().any(|test| !tests.contains(test)) {
        bail!("{runner} ran tests outside the changed selection");
    }
    Ok(())
}

fn report_path(repo: &Path, path: &str) -> Result<String> {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.strip_prefix(repo)
            .context("reported test is outside the repository")?
    } else {
        path
    };
    let path = path.to_str().context("test path is not UTF-8")?;
    if !safe_path(path) {
        bail!("invalid reported test path");
    }
    Ok(path.to_string())
}

fn playwright_results(
    repo: &Path,
    root: &Path,
    suites: &Value,
    ancestry: &[String],
    results: &mut BTreeMap<(Identity, String), usize>,
) -> Result<()> {
    for suite in suites.as_array().context("Playwright suites array")? {
        let mut names = ancestry.to_vec();
        // Root suites are files, not describe titles.
        if !ancestry.is_empty() || suite.get("column").and_then(Value::as_u64).unwrap_or(0) != 0 {
            names.push(
                suite
                    .get("title")
                    .and_then(Value::as_str)
                    .context("suite title")?
                    .to_string(),
            );
        }
        for spec in suite
            .get("specs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut full = names.clone();
            full.push(
                spec.get("title")
                    .and_then(Value::as_str)
                    .context("spec title")?
                    .to_string(),
            );
            let file = root.join(
                spec.get("file")
                    .and_then(Value::as_str)
                    .context("spec file")?,
            );
            let id = Identity {
                path: report_path(repo, file.to_str().context("spec path encoding")?)?,
                name: full.join(" "),
            };
            for test in spec
                .get("tests")
                .and_then(Value::as_array)
                .context("spec tests")?
            {
                let attempts = test
                    .get("results")
                    .and_then(Value::as_array)
                    .context("test results")?;
                if test.get("expectedStatus").and_then(Value::as_str) != Some("passed")
                    || attempts.len() != 1
                    || attempts[0].get("status").and_then(Value::as_str) != Some("passed")
                    || attempts[0]
                        .get("retry")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        != 0
                {
                    bail!("playwright stability failed: {} :: {}", id.path, id.name);
                }
                let project = test
                    .get("projectId")
                    .or_else(|| test.get("projectName"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                *results
                    .entry((id.clone(), project.to_string()))
                    .or_default() += 1;
            }
        }
        if let Some(nested) = suite.get("suites") {
            playwright_results(repo, root, nested, &names, results)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(path: &str, name: &str) -> Identity {
        Identity {
            path: path.into(),
            name: name.into(),
        }
    }

    #[test]
    fn javascript_discovery_selects_changed_functions_and_rejects_dynamic_names() {
        let source = "\ndescribe('group', () => {\n  test('unchanged', () => {\n    expect(1).toBe(1);\n  });\n  test('changed', () => {\n    expect(2).toBe(2);\n  });\n});\n";
        assert_eq!(
            source_tests("a.test.ts", source, &[(7, 7)]).unwrap(),
            [id("a.test.ts", &["group", "changed"].join(" "))]
        );
        assert_eq!(
            source_tests("a.test.ts", source, &[(2, 2)]).unwrap(),
            [
                id("a.test.ts", &["group", "unchanged"].join(" ")),
                id("a.test.ts", &["group", "changed"].join(" "))
            ]
        );
        for source in [
            "test(title, () => {});",
            "describe(title, () => { test('child', () => {}); });",
            "test.each([1, 2])('row %i', () => {});",
            "test('case_id', callback);",
            "describe('suite_id', declareTests);",
        ] {
            assert!(
                source_tests("a.test.ts", source, &[(1, 1)]).is_err(),
                "{source}"
            );
        }
        assert_eq!(
            changed_lines("@@ -7 +7,0 @@\n-x\n@@ -10 +9,2 @@\n+y\n+z").unwrap(),
            [(7, 7), (9, 10)]
        );
        assert!(!allowed_args(&["vitest".into(), "--retry=3".into()]));
        assert!(!allowed_args(&["jest".into(), "../a.test.js".into()]));
        let aliased = "import { test as check } from 'vitest'; check('case_id', () => {});";
        assert_eq!(
            source_tests("a.test.ts", aliased, &[(1, 1)]).unwrap(),
            [id("a.test.ts", "case_id")]
        );
        for source in [
            "test('x', {retry: 2}, () => {});",
            "jest.retryTimes(2); test('x', () => {});",
        ] {
            assert!(source_tests("a.test.ts", source, &[(1, 1)]).is_err());
        }
        let scoped_retry =
            "test('selected', () => {});\ntest('unchanged', { retry: 2 }, () => {});";
        assert_eq!(
            source_tests("a.test.ts", scoped_retry, &[(1, 1)]).unwrap(),
            [id("a.test.ts", "selected")]
        );

        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(repo.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output);
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "--quiet"]);
        let original =
            "\ntest('case_1', () => expect(1).toBe(1)); test('case_2', () => expect(2).toBe(2));\n";
        std::fs::write(repo.path().join("a.test.ts"), original).unwrap();
        git(&["add", "a.test.ts"]);
        git(&["commit", "--quiet", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(
            repo.path().join("a.test.ts"),
            original.replace("expect(2)", "expect(3)"),
        )
        .unwrap();
        git(&["add", "a.test.ts"]);
        git(&["commit", "--quiet", "-m", "head"]);
        let head = git(&["rev-parse", "HEAD"]);
        assert_eq!(
            changed_tests(repo.path(), &base, &head, &["a.test.ts".into()]).unwrap(),
            [id("a.test.ts", "case_2")]
        );
        std::fs::write(repo.path().join("a.test.ts"), "declareTests();").unwrap();
        git(&["add", "a.test.ts"]);
        git(&["commit", "--quiet", "-m", "factory"]);
        assert!(
            changed_tests(
                repo.path(),
                &head,
                &git(&["rev-parse", "HEAD"]),
                &["a.test.ts".into()]
            )
            .is_err()
        );
        let cas_root = tempfile::tempdir().unwrap();
        let error = execute(
            cas_root.path(),
            &["vitest".into(), "a.test.ts".into()],
            repo.path(),
        )
        .unwrap_err();
        assert!(error.is::<IsolatedCheckoutRequired>());
    }

    #[test]
    fn javascript_reports_require_every_selected_test_and_reject_failure_or_extra_execution() {
        let repo = Path::new("/project");
        let selected = [id("a.test.js", "group changed")];
        for runner in ["vitest", "jest"] {
            let passing = json!({"success":true,"testResults":[{"name":"/project/a.test.js","status":"passed", "assertionResults":[
                {"fullName":"group changed","status":"passed"}, {"fullName":"unchanged","status":"pending"}]}]});
            validate_report(runner, repo, &passing, &selected, 1).unwrap();
            for status in ["failed", "pending", "skipped"] {
                let mut failed = passing.clone();
                failed["testResults"][0]["assertionResults"][0]["status"] = json!(status);
                assert!(validate_report(runner, repo, &failed, &selected, 1).is_err());
            }
            let mut extra = passing.clone();
            extra["testResults"][0]["assertionResults"][1]["status"] = json!("passed");
            assert!(validate_report(runner, repo, &extra, &selected, 1).is_err());
            assert!(validate_report(runner, repo, &passing, &selected, 5).is_err());
            let mut retried = passing.clone();
            retried["testResults"][0]["assertionResults"][0]["invocations"] = json!(2);
            assert!(validate_report(runner, repo, &retried, &selected, 1).is_err());
        }
    }

    #[test]
    fn playwright_report_requires_five_clean_attempts_without_retry() {
        let selected = [id("a.spec.ts", "group changed")];
        let attempt = json!({"expectedStatus":"passed","results":[{"status":"passed","retry":0}]});
        let mut report = json!({"errors":[],"suites":[{"title":"a.spec.ts","column":0,"specs":[],"suites":[
            {"title":"group","column":1,"specs":[{"file":"a.spec.ts","title":"changed","tests":vec![attempt; 5]}],"suites":[]}]}]});
        validate_report("playwright", Path::new("/project"), &report, &selected, 5).unwrap();
        report["suites"][0]["suites"][0]["specs"][0]["tests"][2]["results"][0]["status"] =
            json!("failed");
        assert!(
            validate_report("playwright", Path::new("/project"), &report, &selected, 5).is_err()
        );
        report["suites"][0]["suites"][0]["specs"][0]["tests"][2]["results"][0] =
            json!({"status":"passed","retry":1});
        assert!(
            validate_report("playwright", Path::new("/project"), &report, &selected, 5).is_err()
        );
    }

    #[test]
    fn javascript_repeat_loop_stops_at_seeded_failure_and_requires_five_passes() {
        let tests = [id("a.test.js", "seeded")];
        for fail_at in [None, Some(2)] {
            let mut invocations = 0;
            let result = verify_repeats("vitest", Path::new("/project"), &tests, |index| {
                invocations += 1;
                let pass = Some(index) != fail_at;
                Ok((
                    pass,
                    json!({"success":pass,"testResults":[{"name":"/project/a.test.js", "status":if pass {"passed"} else {"failed"},
                    "assertionResults":[{"fullName":"seeded","status":if pass {"passed"} else {"failed"}}]}]}),
                ))
            });
            assert_eq!(result.is_ok(), fail_at.is_none());
            assert_eq!(invocations, if fail_at.is_some() { 3 } else { 5 });
        }
    }
}
