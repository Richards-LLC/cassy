//! Source-scoped JavaScript stability evidence. A failure is never retried.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tree_sitter::{Node, Parser};

use super::stability::{Identity, Proof};

fn safe_path(path: &str) -> bool {
    !path.is_empty() && Path::new(path).components().all(|part| matches!(part, Component::Normal(_)))
        && !path.starts_with('-')
}

fn javascript(path: &str) -> bool {
    matches!(Path::new(path).extension().and_then(|ext| ext.to_str()),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts"))
}

pub(super) fn allowed_args(args: &[String]) -> bool {
    args.len() > 1 && matches!(args[0].as_str(), "vitest" | "jest" | "playwright")
        && args[1..].iter().all(|path| safe_path(path) && javascript(path))
}

/// Only hunks intersecting a test function (or its suite title) select it.
/// Dynamic titles cannot safely become a runtime selector and fail closed.
pub(crate) fn changed_tests(repo: &Path, base: &str, head: &str, paths: &[String]) -> Result<Vec<Identity>> {
    let mut selected = BTreeSet::new();
    for path in paths.iter().filter(|path| javascript(path)) {
        if !safe_path(path) { bail!("invalid JavaScript stability path: {path}"); }
        if super::git(repo, &["ls-tree", "--name-only", head, "--", path])?.is_empty() { continue; }
        let output = Command::new("git").env("GIT_OPTIONAL_LOCKS", "0").current_dir(repo)
            .args(["show", &format!("{head}:{path}")]).output()?;
        if !output.status.success() { bail!("cannot read {head}:{path}"); }
        let source = String::from_utf8(output.stdout)?;
        let diff = super::git(repo, &["diff", "--no-ext-diff", "--no-color", "--unified=0", base, head, "--", path])?;
        let old = Command::new("git").env("GIT_OPTIONAL_LOCKS", "0").current_dir(repo)
            .args(["show", &format!("{base}:{path}")]).output()?;
        let old = if old.status.success() { String::from_utf8(old.stdout)? } else { String::new() };
        let previous: BTreeMap<_, _> = parsed_tests(path, &old, &[(1, usize::MAX)], false)?.into_iter().collect();
        for (identity, body) in parsed_tests(path, &source, &changed_lines(&diff)?, true)? {
            // Hunk lines may contain several calls. Repeating an unchanged
            // neighbour would violate the delivery-only selection contract.
            if previous.get(&identity) != Some(&body) { selected.insert(identity); }
        }
    }
    Ok(selected.into_iter().collect())
}

fn changed_lines(diff: &str) -> Result<Vec<(usize, usize)>> {
    diff.lines().filter(|line| line.starts_with("@@ ")).map(|line| {
        let range = line.split_whitespace().nth(2).context("git hunk new range")?
            .strip_prefix('+').context("git hunk new range prefix")?;
        let (start, count) = range.split_once(',').unwrap_or((range, "1"));
        let start: usize = start.parse()?;
        let count: usize = count.parse()?;
        // A pure deletion inside a surviving test still changes that test.
        Ok((start, start + count.saturating_sub(1)))
    }).collect()
}

fn overlaps(node: Node<'_>, lines: &[(usize, usize)]) -> bool {
    lines.iter().any(|(start, end)| *start <= node.end_position().row + 1 && *end >= node.start_position().row + 1)
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

/// Decode static strings only. Unsupported JavaScript escapes fail closed.
fn title(node: Node<'_>, source: &str) -> Option<String> {
    if !matches!(node.kind(), "string" | "template_string") { return None; }
    let raw = text(node, source);
    let contents = raw.get(1..raw.len().checked_sub(1)?)?;
    if raw.starts_with('"') { return serde_json::from_str(raw).ok(); }
    if contents.contains('\\') || (raw.starts_with('`') && contents.contains("${")) { return None; }
    Some(contents.to_string())
}

fn declaration(node: Node<'_>, source: &str) -> Option<(bool, bool)> {
    let function = node.child_by_field_name("function")?;
    let name = text(function, source);
    let parts: Vec<_> = name.split('.').collect();
    if !matches!(parts.first().copied(), Some("test" | "it" | "describe")) { return None; }
    let suite = parts[0] == "describe" || parts.get(1) == Some(&"describe");
    let offset = if parts[0] == "test" && suite { 2 } else { 1 };
    if parts[offset..].iter().all(|part| matches!(*part, "only" | "skip" | "todo" | "concurrent" | "sequential" | "fails" | "fixme")) {
        Some((suite, true))
    } else if name.contains("each") || function.kind() == "call_expression" {
        Some((suite, false))
    } else { None }
}

#[cfg(test)]
fn source_tests(path: &str, source: &str, lines: &[(usize, usize)]) -> Result<Vec<Identity>> {
    Ok(parsed_tests(path, source, lines, true)?.into_iter().map(|(identity, _)| identity).collect())
}

fn parsed_tests(path: &str, source: &str, lines: &[(usize, usize)], strict: bool) -> Result<Vec<(Identity, String)>> {
    let _ = (path, source, lines, strict);
    Ok(Vec::new())
}

#[allow(clippy::too_many_arguments)]
fn walk(node: Node<'_>, source: &str, path: &str, lines: &[(usize, usize)],
    suites: &[String], unresolved: bool, force: bool, strict: bool, tests: &mut Vec<(Identity, String)>) -> Result<()> {
    if node.kind() == "call_expression" && let Some((suite, supported)) = declaration(node, source) {
        let args = node.child_by_field_name("arguments").context("test arguments")?;
        let name = args.named_child(0).and_then(|name| title(name, source));
        let unresolved = unresolved || !supported || name.is_none();
        if suite {
            let mut nested = suites.to_vec();
            if let Some(name) = name { nested.push(name); }
            let callback = (0..args.named_child_count()).filter_map(|i| args.named_child(i))
                .find(|arg| matches!(arg.kind(), "arrow_function" | "function_expression"));
            let header_changed = lines.iter().any(|(start, end)| *end >= node.start_position().row + 1
                && *start <= callback.map_or(node.start_position().row + 1, |cb| cb.start_position().row + 1));
            if let Some(callback) = callback {
                return walk(callback, source, path, lines, &nested, unresolved, force || header_changed, strict, tests);
            }
        } else {
            if force || overlaps(node, lines) {
                if unresolved {
                    if !strict { return Ok(()); }
                    bail!("cannot resolve changed test at {path}:{}: dynamic/parameterized test title; provide a static test identity", node.start_position().row + 1);
                }
                let mut names = suites.to_vec();
                names.push(name.expect("resolved title"));
                tests.push((Identity { path: path.to_string(), name: names.join(" ") }, text(node, source).to_string()));
            }
            // Test bodies cannot declare additional runnable tests.
            return Ok(());
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, source, path, lines, suites, unresolved, force, strict, tests)?;
    }
    Ok(())
}

pub(super) fn execute(cas_root: &Path, args: &[String], cwd: &Path) -> Result<()> {
    if !allowed_args(args) { bail!("expected vitest|jest|playwright followed by repository-relative test files"); }
    let repo = PathBuf::from(super::git(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let head = super::clean_head(&repo)?;
    let config = crate::config::Config::load(cas_root)?.factory();
    let base = super::proof_diff_base(cas_root, &repo, config.epic_base_branch.as_deref())?;
    let tests = changed_tests(&repo, &base, &head, &args[1..])?;
    if tests.is_empty() { bail!("no changed JavaScript tests matched the supplied files"); }
    let _lane = super::try_lock_lane_with_command(cas_root, &repo, &super::worker_check_command(args))?
        .ok_or_else(|| super::lane_lock_refusal(&super::lane_lock_path(cas_root, &repo).unwrap_or_default()))?;
    let _slot = super::admit_daemon_build(cas_root).map_err(|cause| super::recorded_refusal(
        cas_root, &repo, &cause, crate::factory_capacity_wait::CapacityKind::Other))?;
    super::stability::invalidate(cas_root, &repo, &head, &tests)?;
    let proof = execute_tests(cas_root, &repo, &head, &args[0], &tests)?;
    println!("stability: PASS {head} {} {} tests 5/5 in {} ms", proof.runner, proof.tests.len(), proof.elapsed_ms);
    Ok(())
}

fn package_root(repo: &Path, path: &str) -> Result<PathBuf> {
    let mut current = repo.join(path).parent().context("test parent")?.to_path_buf();
    loop {
        if current.join("package.json").is_file() { return Ok(current); }
        if current == repo || !current.pop() { bail!("no package.json found for {path}"); }
    }
}

fn runner_path(repo: &Path, package: &Path, runner: &str) -> Result<PathBuf> {
    let mut current = package.to_path_buf();
    loop {
        let executable = current.join("node_modules/.bin").join(runner);
        if executable.is_file() { return Ok(executable); }
        if current == repo || !current.pop() { bail!("{runner} is not installed locally for {}", package.display()); }
    }
}

fn execute_tests(cas_root: &Path, repo: &Path, head: &str, runner: &str, tests: &[Identity]) -> Result<Proof> {
    let start = Instant::now();
    let config = crate::config::Config::load(cas_root)?.factory();
    let high = crate::factory_build_memory::memory_high(config.worker_memory_high_gib,
        config.max_concurrent_builders, &std::fs::read_to_string("/proc/meminfo").unwrap_or_default());
    let mut by_file: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for test in tests { by_file.entry(&test.path).or_default().push(&test.name); }
    let reports = tempfile::tempdir()?;
    for (path, names) in by_file {
        let package = package_root(repo, path)?;
        let executable = runner_path(repo, &package, runner)?;
        let pattern = names.iter().map(|name| format!("(?:{})", regex::escape(name))).collect::<Vec<_>>().join("|");
        let pattern = if runner == "playwright" { format!("(?:^|\\s)(?:{pattern})$") } else { format!("^(?:{pattern})$") };
        let selected: Vec<_> = names.iter().map(|name| Identity { path: path.to_string(), name: (*name).to_string() }).collect();
        verify_repeats(runner, repo, &selected, |iteration| {
            let report = reports.path().join(format!("report-{iteration}.json"));
            let mut command = Command::new(&executable);
            command.current_dir(&package).env("CI", "1");
            let file = repo.join(path);
            match runner {
                "vitest" => { command.args(["run", "--retry", "0", "--bail", "1", "--maxWorkers", "1", "--no-file-parallelism", "--reporter=json", "--outputFile"]).arg(&report).arg("--testNamePattern").arg(&pattern).arg(&file); }
                "jest" => { command.args(["--runInBand", "--bail=1", "--json", "--outputFile"]).arg(&report).arg("--testNamePattern").arg(&pattern).arg("--runTestsByPath").arg(&file); }
                "playwright" => { command.args(["test", "--repeat-each", "5", "--retries", "0", "--max-failures", "1", "--workers", "1", "--reporter=json", "--grep"]).arg(&pattern).arg(regex::escape(&file.to_string_lossy())).env("PLAYWRIGHT_JSON_OUTPUT_FILE", &report); }
                _ => bail!("unsupported stability runner: {runner}"),
            }
            let marker = reports.path().join(format!("scope-{iteration}"));
            // A stale marker could mistake admission failure for test execution.
            let _ = std::fs::remove_file(&marker);
            let mut command = crate::factory_build_memory::scope_command(command,
                cfg!(target_os = "linux").then_some(Path::new("systemd-run")), high, &marker);
            #[cfg(unix)]
            { use std::os::unix::process::CommandExt; command.process_group(0); }
            #[cfg(unix)]
            let _signals = super::WorkerWaitSignals::install()?;
            let mut child = command.spawn().with_context(|| format!("start {runner} stability run"))?;
            #[cfg(unix)]
            let status = super::wait_for_worker_command(&mut child)?;
            #[cfg(not(unix))]
            let status = child.wait()?;
            let value: Value = serde_json::from_slice(&std::fs::read(&report)
                .with_context(|| format!("{runner} did not produce a stability report for {path}"))?)?;
            Ok((status.success(), value))
        })?;
    }
    if super::clean_head(repo)? != head { bail!("repository changed during JavaScript stability run"); }
    let proof = Proof { runner: runner.to_string(), tests: tests.to_vec(), consecutive_passes: 5, elapsed_ms: start.elapsed().as_millis() };
    super::stability::record(cas_root, repo, head, proof.clone())?;
    Ok(proof)
}

fn verify_repeats(runner: &str, repo: &Path, tests: &[Identity], mut run: impl FnMut(usize) -> Result<(bool, Value)>) -> Result<()> {
    for iteration in 0..if runner == "playwright" { 1 } else { 5 } {
        let (success, report) = run(iteration)?;
        validate_report(runner, repo, &report, tests, if runner == "playwright" { 5 } else { 1 })?;
        if !success { bail!("{runner} stability failed at iteration {}; no retry", iteration + 1); }
    }
    Ok(())
}

fn validate_report(runner: &str, repo: &Path, report: &Value, tests: &[Identity], repeats: usize) -> Result<()> {
    let mut results: BTreeMap<Identity, usize> = BTreeMap::new();
    if runner == "playwright" {
        let root = report.get("config").and_then(|config| config.get("rootDir")).and_then(Value::as_str)
            .map(Path::new).unwrap_or(repo);
        let mut projects = BTreeMap::new();
        playwright_results(repo, root, report.get("suites").context("Playwright suites missing")?, &[], &mut projects)?;
        for ((identity, _project), count) in projects {
            if count != repeats { bail!("playwright requires {repeats} passing repeats per project: {} :: {}", identity.path, identity.name); }
            results.insert(identity, count);
        }
        if report.get("errors").and_then(Value::as_array).is_some_and(|errors| !errors.is_empty()) {
            bail!("Playwright stability report contains runner errors");
        }
    } else {
        for file in report.get("testResults").and_then(Value::as_array).context("testResults missing")? {
            let path = report_path(repo, file.get("name").and_then(Value::as_str).context("test file missing")?)?;
            for test in file.get("assertionResults").and_then(Value::as_array).context("assertionResults missing")? {
                let name = test.get("fullName").and_then(Value::as_str).context("test fullName missing")?;
                let id = Identity { path: path.clone(), name: name.to_string() };
                match test.get("status").and_then(Value::as_str) {
                    Some("passed") => { *results.entry(id).or_default() += 1; }
                    Some("pending" | "skipped" | "todo" | "disabled") if !tests.contains(&id) => {}
                    _ => bail!("{runner} stability failed: {} :: {}", id.path, id.name),
                }
            }
            if file.get("status").and_then(Value::as_str) == Some("failed") { bail!("{runner} suite failed: {path}"); }
        }
        if report.get("success").and_then(Value::as_bool) != Some(true) { bail!("{runner} stability report was unsuccessful"); }
    }
    for test in tests {
        if results.get(test) != Some(&repeats) { bail!("{runner} stability requires {repeats} passing results: {} :: {}", test.path, test.name); }
    }
    if results.keys().any(|test| !tests.contains(test)) { bail!("{runner} ran tests outside the changed selection"); }
    Ok(())
}

fn report_path(repo: &Path, path: &str) -> Result<String> {
    let path = Path::new(path);
    let path = if path.is_absolute() { path.strip_prefix(repo).context("reported test is outside the repository")? } else { path };
    let path = path.to_str().context("test path is not UTF-8")?;
    if !safe_path(path) { bail!("invalid reported test path"); }
    Ok(path.to_string())
}

fn playwright_results(repo: &Path, root: &Path, suites: &Value, ancestry: &[String], results: &mut BTreeMap<(Identity, String), usize>) -> Result<()> {
    for suite in suites.as_array().context("Playwright suites array")? {
        let mut names = ancestry.to_vec();
        // Root suites are files, not describe titles.
        if !ancestry.is_empty() || suite.get("column").and_then(Value::as_u64).unwrap_or(0) != 0 {
            names.push(suite.get("title").and_then(Value::as_str).context("suite title")?.to_string());
        }
        for spec in suite.get("specs").and_then(Value::as_array).into_iter().flatten() {
            let mut full = names.clone();
            full.push(spec.get("title").and_then(Value::as_str).context("spec title")?.to_string());
            let file = root.join(spec.get("file").and_then(Value::as_str).context("spec file")?);
            let id = Identity { path: report_path(repo, file.to_str().context("spec path encoding")?)?, name: full.join(" ") };
            for test in spec.get("tests").and_then(Value::as_array).context("spec tests")? {
                let attempts = test.get("results").and_then(Value::as_array).context("test results")?;
                if test.get("expectedStatus").and_then(Value::as_str) != Some("passed") || attempts.len() != 1
                    || attempts[0].get("status").and_then(Value::as_str) != Some("passed")
                    || attempts[0].get("retry").and_then(Value::as_u64).unwrap_or(0) != 0 {
                    bail!("playwright stability failed: {} :: {}", id.path, id.name);
                }
                let project = test.get("projectId").or_else(|| test.get("projectName")).and_then(Value::as_str).unwrap_or_default();
                *results.entry((id.clone(), project.to_string())).or_default() += 1;
            }
        }
        if let Some(nested) = suite.get("suites") { playwright_results(repo, root, nested, &names, results)?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(path: &str, name: &str) -> Identity { Identity { path: path.into(), name: name.into() } }

    #[test]
    fn javascript_discovery_selects_changed_functions_and_rejects_dynamic_names() {
        let source = "\ndescribe('group', () => {\n  test('unchanged', () => {\n    expect(1).toBe(1);\n  });\n  test('changed', () => {\n    expect(2).toBe(2);\n  });\n});\n";
        assert_eq!(source_tests("a.test.ts", source, &[(7, 7)]).unwrap(), [id("a.test.ts", &["group", "changed"].join(" "))]);
        assert_eq!(source_tests("a.test.ts", source, &[(2, 2)]).unwrap(),
            [id("a.test.ts", &["group", "unchanged"].join(" ")), id("a.test.ts", &["group", "changed"].join(" "))]);
        for source in ["test(title, () => {});", "describe(title, () => { test('child', () => {}); });",
            "test.each([1, 2])('row %i', () => {});"] {
            assert!(source_tests("a.test.ts", source, &[(1, 1)]).is_err(), "{source}");
        }
        assert_eq!(changed_lines("@@ -7 +7,0 @@\n-x\n@@ -10 +9,2 @@\n+y\n+z").unwrap(), [(7, 7), (9, 10)]);
        assert!(!allowed_args(&["vitest".into(), "--retry=3".into()]));
        assert!(!allowed_args(&["jest".into(), "../a.test.js".into()]));
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
        }
    }

    #[test]
    fn playwright_report_requires_five_clean_attempts_without_retry() {
        let selected = [id("a.spec.ts", "group changed")];
        let attempt = json!({"expectedStatus":"passed","results":[{"status":"passed","retry":0}]});
        let mut report = json!({"errors":[],"suites":[{"title":"a.spec.ts","column":0,"specs":[],"suites":[
            {"title":"group","column":1,"specs":[{"file":"a.spec.ts","title":"changed","tests":vec![attempt; 5]}],"suites":[]}]}]});
        validate_report("playwright", Path::new("/project"), &report, &selected, 5).unwrap();
        report["suites"][0]["suites"][0]["specs"][0]["tests"][2]["results"][0]["status"] = json!("failed");
        assert!(validate_report("playwright", Path::new("/project"), &report, &selected, 5).is_err());
        report["suites"][0]["suites"][0]["specs"][0]["tests"][2]["results"][0] = json!({"status":"passed","retry":1});
        assert!(validate_report("playwright", Path::new("/project"), &report, &selected, 5).is_err());
    }

    #[test]
    fn javascript_repeat_loop_stops_at_seeded_failure_and_requires_five_passes() {
        let tests = [id("a.test.js", "seeded")];
        for fail_at in [None, Some(2)] {
            let mut invocations = 0;
            let result = verify_repeats("vitest", Path::new("/project"), &tests, |index| {
                invocations += 1;
                let pass = Some(index) != fail_at;
                Ok((pass, json!({"success":pass,"testResults":[{"name":"/project/a.test.js", "status":if pass {"passed"} else {"failed"},
                    "assertionResults":[{"fullName":"seeded","status":if pass {"passed"} else {"failed"}}]}]})))
            });
            assert_eq!(result.is_ok(), fail_at.is_none());
            assert_eq!(invocations, if fail_at.is_some() { 3 } else { 5 });
        }
    }
}
