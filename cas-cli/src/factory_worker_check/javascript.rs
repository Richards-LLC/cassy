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

/// The runner and its test files: `vitest|jest|playwright <files>`, or
/// `node --test <files>` for Node's built-in runner (cas-a4c3).
fn runner_files(args: &[String]) -> Option<(&str, &[String])> {
    let (runner, files) = match args {
        [node, flag, files @ ..] if node == "node" && flag == "--test" => ("node", files),
        [runner, files @ ..] if matches!(runner.as_str(), "vitest" | "jest" | "playwright") => {
            (runner.as_str(), files)
        }
        _ => return None,
    };
    (!files.is_empty() && files.iter().all(|path| safe_path(path) && javascript(path)))
        .then_some((runner, files))
}

pub(super) fn allowed_args(args: &[String]) -> bool {
    runner_files(args).is_some()
}

/// A `node --test` reporter emitting the Vitest/Jest JSON shape that
/// `validate_report` reads. Suite names join a test's own name with spaces,
/// as `--test-name-pattern` matches them.
const NODE_TEST_REPORTER: &str = r#"export default async function* (source) {
  const names = [], files = new Map();
  let success = true;
  for await (const { type, data } of source) {
    if (type === 'test:start') names[data.nesting] = data.name;
    if (type !== 'test:pass' && type !== 'test:fail') continue;
    const file = files.get(data.file) ?? { name: data.file, status: 'passed', assertionResults: [] };
    files.set(data.file, file);
    if (type === 'test:fail') { success = false; file.status = 'failed'; }
    if (data.details?.type === 'suite') continue;
    file.assertionResults.push({
      fullName: [...names.slice(0, data.nesting), data.name].join(' '),
      status: type === 'test:fail' ? 'failed' : data.skip ? 'skipped' : data.todo ? 'todo' : 'passed',
    });
  }
  yield JSON.stringify({ success, testResults: [...files.values()] });
}
"#;

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
        // A restored file has no delivered test change. Do not require
        // runtime identities for callbacks merely present in its final tree.
        if diff.is_empty() { continue; }
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
    callee(node.child_by_field_name("function")?, source, aliases)
}

/// cas-6a3c: `(cond ? describe : describe.skip)(…)` declares a suite when both
/// branches declare the same kind; jest names its tests with that title.
fn callee(function: Node<'_>, source: &str, aliases: &BTreeMap<String, String>) -> Option<(bool, bool)> {
    match function.kind() {
        "parenthesized_expression" => callee(function.named_child(0)?, source, aliases),
        "ternary_expression" => {
            let (suite, supported) = callee(function.child_by_field_name("consequence")?, source, aliases)?;
            let (other, also) = callee(function.child_by_field_name("alternative")?, source, aliases)?;
            (suite == other).then_some((suite, supported && also))
        }
        _ => named_declaration(text(function, source), aliases),
    }
}

/// A conditional callee that names the test API yet resolves to no single kind.
fn unresolved_callee(function: Node<'_>, source: &str, aliases: &BTreeMap<String, String>) -> bool {
    matches!(function.kind(), "parenthesized_expression" | "ternary_expression")
        && callee(function, source, aliases).is_none()
        && text(function, source)
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .any(|word| matches!(word, "test" | "it" | "describe") || aliases.contains_key(word))
}

fn named_declaration(name: &str, aliases: &BTreeMap<String, String>) -> Option<(bool, bool)> {
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
    let tree = source_tree(path, source)?;
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
    let mut unique = BTreeSet::new();
    if tests.iter().any(|(identity, _)| !unique.insert(identity)) {
        bail!("ambiguous source-to-runtime test identity in {path}");
    }
    Ok(tests)
}

#[derive(Debug)]
struct DeclaredSite {
    line: usize,
    column: usize,
    names: Vec<String>,
}

fn source_tree(path: &str, source: &str) -> Result<tree_sitter::Tree> {
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
    Ok(tree)
}

fn declared_identities<'a>(
    node: Node<'a>,
    source: &str,
) -> Result<Option<(Node<'a>, Vec<String>)>> {
    let statement = node
        .parent()
        .filter(|parent| parent.kind() == "expression_statement")
        .unwrap_or(node);
    let Some(comment) = statement
        .prev_named_sibling()
        .filter(|prev| prev.kind() == "comment")
    else {
        return Ok(None);
    };
    let Some(raw) = text(comment, source).strip_prefix("// cas-stability-identities:") else {
        return Ok(None);
    };
    if comment.end_position().row + 1 != statement.start_position().row {
        bail!("stability identities must immediately precede their registration");
    }
    let names: Vec<String> =
        serde_json::from_str(raw.trim()).context("invalid stability identity declaration")?;
    let unique: BTreeSet<_> = names.iter().collect();
    if names.is_empty()
        || unique.len() != names.len()
        || names.iter().any(|name| name.trim().is_empty())
    {
        bail!("stability identities must be nonempty and unique");
    }
    Ok(Some((comment, names)))
}

fn contains_binding(node: Node<'_>, name: &str, source: &str) -> bool {
    if matches!(
        node.kind(),
        "identifier" | "shorthand_property_identifier_pattern"
    ) {
        return text(node, source) == name;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| contains_binding(child, name, source))
}

/// Resolve only a directly initialized lexical const. Unsupported nearer bindings
/// are terminal: never search past a shadowing parameter, let, import or pattern.
fn callback_binding<'a>(call: Node<'a>, callback: Node<'a>, source: &str) -> Result<Node<'a>> {
    if callback.kind() != "identifier" {
        bail!("declared stability callback must be a local const identifier");
    }
    let name = text(callback, source);
    let mut parent = call.parent();
    while let Some(scope) = parent {
        if matches!(scope.kind(), "program" | "statement_block") {
            let mut matches = Vec::new();
            let mut cursor = scope.walk();
            for statement in scope.named_children(&mut cursor) {
                if matches!(
                    statement.kind(),
                    "lexical_declaration" | "variable_declaration"
                ) {
                    let mut cursor = statement.walk();
                    for binding in statement.named_children(&mut cursor) {
                        if binding.kind() == "variable_declarator"
                            && binding
                                .child_by_field_name("name")
                                .is_some_and(|pattern| contains_binding(pattern, name, source))
                        {
                            matches.push((statement, binding));
                        }
                    }
                } else if statement
                    .child_by_field_name("name")
                    .is_some_and(|binding| text(binding, source) == name)
                    || (statement.kind() == "import_statement"
                        && contains_binding(statement, name, source))
                {
                    bail!("unsupported lexical stability callback binding: {name}");
                }
            }
            if !matches.is_empty() {
                if matches.len() != 1 {
                    bail!("ambiguous lexical stability callback binding: {name}");
                }
                let (statement, binding) = matches[0];
                if statement
                    .child(0)
                    .is_none_or(|keyword| keyword.kind() != "const")
                    || binding
                        .child_by_field_name("name")
                        .is_none_or(|id| id.kind() != "identifier")
                    || binding.child_by_field_name("value").is_none_or(|value| {
                        !matches!(value.kind(), "arrow_function" | "function_expression")
                    })
                {
                    bail!(
                        "stability callback must be initialized directly as a const function: {name}"
                    );
                }
                return Ok(binding);
            }
        }
        if matches!(
            scope.kind(),
            "arrow_function" | "function_expression" | "function_declaration"
        ) {
            // Do not infer captured callbacks across a function boundary. A var
            // in a nested block can hoist and shadow an outer lexical binding.
            bail!("stability callback must be local to the registering function: {name}");
        }
        if matches!(
            scope.kind(),
            "for_in_statement" | "for_statement" | "catch_clause"
        ) && ["left", "initializer", "parameter"]
            .into_iter()
            .any(|field| {
                scope
                    .child_by_field_name(field)
                    .is_some_and(|binding| contains_binding(binding, name, source))
            })
        {
            bail!("stability callback is shadowed by a control binding: {name}");
        }
        parent = scope.parent();
    }
    bail!("unresolved local stability callback: {name}")
}

fn control_headers<'a>(
    node: Node<'a>,
    source: &'a str,
    aliases: &BTreeMap<String, String>,
) -> Vec<(usize, usize, &'a str)> {
    let mut headers = Vec::new();
    let mut parent = node.parent();
    while let Some(scope) = parent {
        let body = match scope.kind() {
            "for_in_statement" | "for_statement" | "while_statement" | "switch_statement" => {
                scope.child_by_field_name("body")
            }
            "if_statement" => scope.child_by_field_name("consequence"),
            "call_expression"
                if declaration(scope, source, aliases).is_some_and(|(suite, _)| suite) =>
            {
                scope.child_by_field_name("arguments").and_then(|args| {
                    let mut cursor = args.walk();
                    args.named_children(&mut cursor)
                        .find(|arg| matches!(arg.kind(), "arrow_function" | "function_expression"))
                })
            }
            _ => None,
        };
        if let Some(body) = body {
            headers.push((
                scope.start_position().row + 1,
                body.start_position().row + 1,
                &source[scope.start_byte()..body.start_byte()],
            ));
        }
        parent = scope.parent();
    }
    headers
}

fn declared_sites(path: &str, source: &str) -> Result<Vec<DeclaredSite>> {
    fn collect(
        node: Node<'_>,
        source: &str,
        aliases: &BTreeMap<String, String>,
        sites: &mut Vec<DeclaredSite>,
    ) -> Result<()> {
        if node.kind() == "call_expression"
            && declaration(node, source, aliases)
                .is_some_and(|(suite, supported)| !suite && supported)
            && let Some((_, names)) = declared_identities(node, source)?
        {
            sites.push(DeclaredSite {
                line: node.start_position().row + 1,
                column: node.start_position().column + 1,
                names,
            });
            return Ok(());
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect(child, source, aliases, sites)?;
        }
        Ok(())
    }
    let tree = source_tree(path, source)?;
    let mut aliases = BTreeMap::new();
    imported_aliases(tree.root_node(), source, &mut aliases);
    let mut sites = Vec::new();
    collect(tree.root_node(), source, &aliases, &mut sites)?;
    Ok(sites)
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
    // `const suite = cond ? describe : describe.skip;` aliases a conditional callee.
    if node.kind() == "variable_declarator"
        && let (Some(name), Some(value)) = (node.child_by_field_name("name"), node.child_by_field_name("value"))
        && name.kind() == "identifier"
        && matches!(value.kind(), "parenthesized_expression" | "ternary_expression")
        && let Some((suite, true)) = callee(value, source, aliases)
    {
        aliases.insert(text(name, source).to_string(), if suite { "describe" } else { "test" }.to_string());
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

// gate-audit[force]: Suite-header changes include descendant tests in the selected proof; this broadens test coverage and never bypasses a refusal.
// gate-audit[strict]: selector Chooses the changed-scope pass that raises G-61 for an unresolvable callee or title; the non-strict inventory pass never selects tests for proof.
// gate-audit[unresolved]: selector Inherited title-resolution state; it only makes a changed test fail closed and never accepts a refused identity.
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
    if strict
        && node.kind() == "call_expression"
        && (force || overlaps(node, lines))
        && let Some(function) = node.child_by_field_name("function")
        && unresolved_callee(function, source, aliases)
    {
        bail!(crate::gates::tag(
            "G-61",
            format!(
                "cannot resolve the test callee at {path}:{}: both branches of a conditional must be the same describe/test kind; use them directly",
                node.start_position().row + 1
            )
        ));
    }
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
        if !suite && let Some((comment, names)) = declared_identities(node, source)? {
            if !supported || callback.is_some() {
                bail!("explicit stability identities require a supported local shared callback");
            }
            let mut cursor = args.walk();
            let argument = args
                .named_children(&mut cursor)
                .last()
                .context("declared stability callback missing")?;
            let binding = callback_binding(node, argument, source)?;
            let headers = control_headers(node, source, aliases);
            let changed = force
                || overlaps(node, lines)
                || overlaps(comment, lines)
                || overlaps(binding, lines)
                || headers.iter().any(|(first, last, _)| {
                    lines
                        .iter()
                        .any(|(start, end)| start <= last && end >= first)
                });
            if changed {
                if strict {
                    reject_retry_configuration(node, source, path, aliases, true)?;
                }
                let fingerprint = serde_json::to_string(&(
                    text(comment, source),
                    text(node, source),
                    text(binding, source),
                    headers.iter().map(|(_, _, text)| text).collect::<Vec<_>>(),
                ))?;
                for name in names {
                    tests.push((
                        Identity {
                            path: path.to_string(),
                            name,
                        },
                        fingerprint.clone(),
                    ));
                }
            }
            return Ok(());
        }
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
    let Some((runner, files)) = runner_files(args) else {
        bail!(
            "expected vitest|jest|playwright or node --test, followed by repository-relative test files"
        );
    };
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
    let tests = changed_tests(&repo, &base, &head, files)?;
    if tests.is_empty() {
        bail!(crate::gates::tag(
            "G-61",
            "no changed JavaScript tests matched the supplied files"
        ));
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
    let proof = execute_tests(cas_root, &repo, &head, runner, &tests)?;
    println!(
        "stability: PASS {head} {} {} tests 5/5 in {} ms",
        proof.runner,
        proof.tests.len(),
        proof.elapsed_ms
    );
    Ok(())
}

const JEST_CONFIGS: &[&str] = &[
    "jest.config.js",
    "jest.config.ts",
    "jest.config.mjs",
    "jest.config.cjs",
    "jest.config.mts",
    "jest.config.cts",
    "jest.config.json",
];
// Vitest prefers its own config over the Vite config in the same directory.
const VITEST_CONFIGS: &[&str] = &[
    "vitest.config.ts",
    "vitest.config.mts",
    "vitest.config.cts",
    "vitest.config.js",
    "vitest.config.mjs",
    "vitest.config.cjs",
    "vite.config.ts",
    "vite.config.mts",
    "vite.config.cts",
    "vite.config.js",
    "vite.config.mjs",
    "vite.config.cjs",
];
const PLAYWRIGHT_CONFIGS: &[&str] = &[
    "playwright.config.ts",
    "playwright.config.mts",
    "playwright.config.cts",
    "playwright.config.js",
    "playwright.config.mjs",
    "playwright.config.cjs",
];

/// The runner's config in `dir`; a `"jest"` key in package.json counts for Jest.
fn runner_config(dir: &Path, runner: &str) -> Option<PathBuf> {
    let names = match runner {
        "jest" => JEST_CONFIGS,
        "vitest" => VITEST_CONFIGS,
        "playwright" => PLAYWRIGHT_CONFIGS,
        _ => &[],
    };
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|config| config.is_file())
        .or_else(|| {
            let manifest = dir.join("package.json");
            (runner == "jest"
                && std::fs::read(&manifest)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .is_some_and(|manifest| manifest.get("jest").is_some()))
            .then_some(manifest)
        })
}

/// The runner's working directory and explicit config for a test file: the
/// nearest ancestor holding that runner's config, else the nearest
/// package.json. A nested package.json must not hide the config and its
/// transforms (cas-1468).
fn runner_root(repo: &Path, path: &str, runner: &str) -> Result<(PathBuf, Option<PathBuf>)> {
    let mut current = repo
        .join(path)
        .parent()
        .context("test parent")?
        .to_path_buf();
    let mut package = None;
    loop {
        if let Some(config) = runner_config(&current, runner) {
            return Ok((current, Some(config)));
        }
        if package.is_none() && current.join("package.json").is_file() {
            package = Some(current.clone());
        }
        if current == repo || !current.pop() {
            return package
                .map(|package| (package, None))
                .with_context(|| format!("no package.json found for {path}"));
        }
    }
}

/// Runners evidenced in `dir`: config files, package.json dependencies or
/// scripts, then locally installed executables. The first nonempty kind wins.
fn local_runners(dir: &Path) -> BTreeSet<&'static str> {
    const RUNNERS: [&str; 3] = ["jest", "vitest", "playwright"];
    let configs: BTreeSet<_> = RUNNERS
        .into_iter()
        .filter(|runner| runner_config(dir, runner).is_some())
        .collect();
    if !configs.is_empty() {
        return configs;
    }
    let manifest: Value = std::fs::read(dir.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let declared: BTreeSet<_> = RUNNERS
        .into_iter()
        .filter(|runner| {
            let package = if *runner == "playwright" {
                "@playwright/test"
            } else {
                *runner
            };
            ["dependencies", "devDependencies"]
                .iter()
                .any(|field| manifest[field].get(package).is_some())
                || manifest["scripts"].as_object().is_some_and(|scripts| {
                    scripts.values().filter_map(Value::as_str).any(|script| {
                        script
                            .split(|ch: char| ch.is_whitespace() || ch == '&' || ch == ';')
                            .any(|word| word == *runner)
                    })
                })
        })
        .collect();
    if !declared.is_empty() {
        return declared;
    }
    RUNNERS
        .into_iter()
        .filter(|runner| dir.join("node_modules/.bin").join(runner).is_file())
        .collect()
}

/// The runner a G-61 refusal names for a changed test file: its runner
/// import, else the nearest directory with runner evidence. Conflicting or
/// absent evidence leaves the runner unknown.
pub(crate) fn runner_hint(repo: &Path, path: &str) -> Option<&'static str> {
    let source = std::fs::read_to_string(repo.join(path)).unwrap_or_default();
    if source.contains("@playwright/test") {
        return Some("playwright");
    }
    if source.contains("'vitest'") || source.contains("\"vitest\"") {
        return Some("vitest");
    }
    if source.contains("@jest/globals") {
        return Some("jest");
    }
    let mut current = repo.join(path);
    while current.pop() && current.starts_with(repo) {
        let mut runners = local_runners(&current);
        // A Playwright spec imports its runner; a plain test file beside a
        // Playwright config belongs to the unit runner.
        if runners.len() > 1 {
            runners.remove("playwright");
        }
        match runners.len() {
            0 => {}
            1 => return runners.pop_first(),
            _ => return None,
        }
    }
    None
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
        let (package, config) = runner_root(repo, path, runner)?;
        let config: Vec<std::ffi::OsString> = config
            .into_iter()
            .flat_map(|config| ["--config".into(), config.into_os_string()])
            .collect();
        let executable = if runner == "node" {
            PathBuf::from("node")
        } else {
            runner_path(repo, &package, runner)?
        };
        let source = std::fs::read_to_string(repo.join(path))?;
        let sites = declared_sites(path, &source)?;
        if !sites.is_empty() {
            if runner != "playwright" {
                bail!("declared stability identities require native Playwright discovery");
            }
            let report = reports.path().join("discovery.json");
            let mut command = Command::new(&executable);
            command
                .current_dir(&package)
                .env("CI", "1")
                .arg("test")
                .args(&config)
                .args(["--list", "--reporter=json"])
                .arg(regex::escape(&repo.join(path).to_string_lossy()))
                .env("PLAYWRIGHT_JSON_OUTPUT_FILE", &report);
            if !run_scoped_javascript(command, high, &reports.path().join("discovery-scope"))? {
                bail!("Playwright native discovery failed for {path}");
            }
            let report = serde_json::from_slice(
                &std::fs::read(report).context("Playwright discovery report missing")?,
            )?;
            validate_declared_discovery(repo, &report, path, &sites)?;
        }
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
                        .arg("run")
                        .args(&config)
                        .args([
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
                        .args(&config)
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
                        .arg("test")
                        .args(&config)
                        .args([
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
                "node" => {
                    let reporter = reports.path().join("node-test-reporter.mjs");
                    std::fs::write(&reporter, NODE_TEST_REPORTER)?;
                    command
                        .args(["--test", "--test-concurrency=1", "--test-reporter=spec"])
                        .arg("--test-reporter-destination=stdout")
                        .arg(format!("--test-reporter={}", reporter.display()))
                        .arg(format!("--test-reporter-destination={}", report.display()))
                        .arg(format!("--test-name-pattern={pattern}"))
                        .arg(&file);
                }
                _ => bail!("unsupported stability runner: {runner}"),
            }
            let marker = reports.path().join(format!("scope-{iteration}"));
            let success = run_scoped_javascript(command, high, &marker)?;
            let value: Value =
                serde_json::from_slice(&std::fs::read(&report).with_context(|| {
                    format!("{runner} did not produce a stability report for {path}")
                })?)?;
            Ok((success, value))
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

fn validate_declared_discovery(
    repo: &Path,
    report: &Value,
    path: &str,
    sites: &[DeclaredSite],
) -> Result<()> {
    type Location = (String, usize, usize);
    type Discovered = BTreeMap<Location, BTreeMap<String, BTreeSet<String>>>;
    fn collect(
        repo: &Path,
        root: &Path,
        suites: &Value,
        ancestry: &[String],
        discovered: &mut Discovered,
    ) -> Result<()> {
        for suite in suites.as_array().context("Playwright discovery suites")? {
            let mut names = ancestry.to_vec();
            if !ancestry.is_empty() || suite.get("column").and_then(Value::as_u64).unwrap_or(0) != 0
            {
                names.push(
                    suite
                        .get("title")
                        .and_then(Value::as_str)
                        .context("discovery suite title")?
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
                        .context("discovery spec title")?
                        .to_string(),
                );
                let file = root.join(
                    spec.get("file")
                        .and_then(Value::as_str)
                        .context("discovery spec file")?,
                );
                let location = (
                    report_path(repo, file.to_str().context("discovery path encoding")?)?,
                    spec.get("line")
                        .and_then(Value::as_u64)
                        .context("discovery spec line")? as usize,
                    spec.get("column")
                        .and_then(Value::as_u64)
                        .context("discovery spec column")? as usize,
                );
                let tests = spec
                    .get("tests")
                    .and_then(Value::as_array)
                    .context("discovery spec tests")?;
                if tests.is_empty() {
                    bail!("discovery spec has no projects");
                }
                for test in tests {
                    let project = test
                        .get("projectId")
                        .or_else(|| test.get("projectName"))
                        .and_then(Value::as_str)
                        .context("discovery project missing")?;
                    if !discovered
                        .entry(location.clone())
                        .or_default()
                        .entry(project.to_string())
                        .or_default()
                        .insert(full.join(" "))
                    {
                        bail!(
                            "ambiguous duplicate discovered identity at {}:{}:{}",
                            location.0,
                            location.1,
                            location.2
                        );
                    }
                }
            }
            if let Some(nested) = suite.get("suites") {
                collect(repo, root, nested, &names, discovered)?;
            }
        }
        Ok(())
    }
    if report
        .get("errors")
        .and_then(Value::as_array)
        .is_none_or(|errors| !errors.is_empty())
    {
        bail!("Playwright discovery did not produce an error-free report");
    }
    let root = Path::new(
        report
            .get("config")
            .and_then(|config| config.get("rootDir"))
            .and_then(Value::as_str)
            .context("discovery rootDir missing")?,
    );
    let mut discovered = BTreeMap::new();
    collect(
        repo,
        root,
        report.get("suites").context("discovery suites missing")?,
        &[],
        &mut discovered,
    )?;
    let mut declared = BTreeMap::new();
    for site in sites {
        let location = (path.to_string(), site.line, site.column);
        let expected: BTreeSet<_> = site.names.iter().cloned().collect();
        for name in &site.names {
            if declared.insert(name.clone(), location.clone()).is_some() {
                bail!("ambiguous declared runtime identity: {name}");
            }
        }
        let projects = discovered
            .get(&location)
            .context("declared registration missing from native discovery")?;
        if projects.is_empty() || projects.values().any(|actual| actual != &expected) {
            bail!(
                "declared identities do not match native expansion at {path}:{}:{}",
                site.line,
                site.column
            );
        }
    }
    for (location, projects) in &discovered {
        if location.0 != path {
            continue;
        }
        for name in projects.values().flatten() {
            if declared
                .get(name)
                .is_some_and(|expected| expected != location)
            {
                bail!("declared identity has an ambiguous native registration: {name}");
            }
        }
    }
    Ok(())
}

fn run_scoped_javascript(command: Command, high: u64, marker: &Path) -> Result<bool> {
    let _ = std::fs::remove_file(marker);
    let mut command = crate::factory_build_memory::scope_command(
        command,
        cfg!(target_os = "linux").then_some(Path::new("systemd-run")),
        high,
        marker,
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
        .context("start JavaScript stability command")?;
    #[cfg(unix)]
    let status = super::wait_for_worker_command(&mut child)?;
    #[cfg(not(unix))]
    let status = child.wait()?;
    Ok(status.success())
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

    /// cas-a4c3: park demanded five capped passes for hub-web's `node:test`
    /// files while worker-check refused `node --test`. The changed case runs
    /// five times alone; its unchanged neighbour would fail the proof if run.
    #[test]
    fn node_test_files_get_five_capped_passes_cas_a4c3() {
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
                ])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "--quiet"]);
        std::fs::write(repo.path().join("package.json"), "{}").unwrap();
        let file = "e2e/serve.test.mjs";
        std::fs::create_dir_all(repo.path().join("e2e")).unwrap();
        let source = "import { describe, test } from 'node:test';\n\
            describe('server', () => {\n  test('unchanged', () => { throw new Error('ran'); });\n\
            test('changed', () => {});\n});\n";
        std::fs::write(
            repo.path().join(file),
            source.replace("=> {});", "=> { throw 0; });"),
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.path().join(file), source).unwrap();
        git(&["commit", "--quiet", "-am", "head"]);
        let head = git(&["rev-parse", "HEAD"]);
        let args: Vec<String> = ["node", "--test", file].map(String::from).into();
        let (runner, files) = runner_files(&args).expect("node --test is a capped runner");
        let tests = changed_tests(repo.path(), &base, &head, files).unwrap();
        assert_eq!(tests, [id(file, "server changed")]);
        let cas_root = tempfile::tempdir().unwrap();
        let proof = execute_tests(cas_root.path(), repo.path(), &head, runner, &tests).unwrap();
        assert_eq!(
            (proof.runner.as_str(), proof.consecutive_passes),
            ("node", 5)
        );
    }

    /// pulse-card cas-1bf9d (GH #2036): `(url ? describe : describe.skip)(…)`
    /// lost its suite title, so the anchored name pattern filtered the changed
    /// case out and the capped run reported it skipped. A conditional or an
    /// alias of one keeps the title; a mixed one is refused by name.
    #[test]
    fn conditional_describe_keeps_its_title_in_the_capped_identity_cas_6a3c() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(repo.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "--quiet"]);
        std::fs::write(repo.path().join("package.json"), "{}").unwrap();
        let file = "legal.integration.test.mjs";
        let source = |body: &str, prelude: &str| format!(
            "import {{ describe, test }} from 'node:test';\nconst url = 'postgresql://127.0.0.1/db';\n{prelude}\n\
             (url ? describe : describe.skip)('legal', () => {{\n  test('keeps consent', () => {{ {body} }});\n}});\n\
             suite('aliased', () => {{\n  test('keeps consent', () => {{ {body} }});\n}});\n"
        );
        let prelude = "const suite = url ? describe : describe.skip;";
        std::fs::write(repo.path().join(file), source("throw 0;", prelude)).unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.path().join(file), source("", prelude)).unwrap();
        git(&["commit", "--quiet", "-am", "head"]);
        let head = git(&["rev-parse", "HEAD"]);
        let tests = changed_tests(repo.path(), &base, &head, &[file.to_string()]).unwrap();
        assert_eq!(tests, [id(file, "aliased keeps consent"), id(file, "legal keeps consent")]);
        let cas_root = tempfile::tempdir().unwrap();
        let proof = execute_tests(cas_root.path(), repo.path(), &head, "node", &tests).unwrap();
        assert_eq!(proof.consecutive_passes, 5);

        // Branches of different kinds cannot name a runtime identity.
        std::fs::write(repo.path().join(file), source("", prelude).replace("describe.skip)(", "test)(")).unwrap();
        git(&["commit", "--quiet", "-am", "mixed"]);
        let mixed = git(&["rev-parse", "HEAD"]);
        let refusal = changed_tests(repo.path(), &head, &mixed, &[file.to_string()]).unwrap_err().to_string();
        assert!(refusal.contains("G-61"), "{refusal}");
    }

    /// pulse-card (GH #2011): apps/backend/scripts/package.json hid
    /// apps/backend/jest.config.js and its ts-jest transform.
    #[test]
    fn nested_package_json_runs_from_the_jest_config_root_cas_1468() {
        let repo = tempfile::tempdir().unwrap();
        let backend = repo.path().join("apps/backend");
        std::fs::create_dir_all(backend.join("scripts")).unwrap();
        std::fs::write(repo.path().join("package.json"), "{}").unwrap();
        std::fs::write(backend.join("package.json"), "{}").unwrap();
        std::fs::write(backend.join("jest.config.js"), "module.exports = {};").unwrap();
        std::fs::write(backend.join("scripts/package.json"), "{}").unwrap();
        let spec = "apps/backend/scripts/merge-creator-workspaces.spec.ts";
        std::fs::write(repo.path().join(spec), "it('merges', () => {});").unwrap();

        assert_eq!(
            runner_root(repo.path(), spec, "jest").unwrap(),
            (backend.clone(), Some(backend.join("jest.config.js")))
        );
        assert_eq!(runner_hint(repo.path(), spec), Some("jest"));
        // Without a runner config, the nearest package.json stays the root.
        assert_eq!(
            runner_root(repo.path(), spec, "vitest").unwrap(),
            (backend.join("scripts"), None)
        );
    }

    /// A config-free global test resolves its runner from package metadata;
    /// with no evidence the refusal offers each valid form, never a shell
    /// metacharacter alternative.
    #[test]
    fn config_free_test_runner_comes_from_package_evidence_cas_1468() {
        let repo = tempfile::tempdir().unwrap();
        for (package, manifest) in [
            ("web", r#"{"devDependencies":{"vitest":"3"}}"#),
            ("api", r#"{"scripts":{"test":"jest --ci"}}"#),
            ("bare", "{}"),
        ] {
            std::fs::create_dir_all(repo.path().join(package)).unwrap();
            std::fs::write(repo.path().join(package).join("package.json"), manifest).unwrap();
            std::fs::write(
                repo.path().join(package).join("a.test.ts"),
                "test('a', () => {});",
            )
            .unwrap();
        }
        std::fs::create_dir_all(repo.path().join("bin/node_modules/.bin")).unwrap();
        std::fs::write(repo.path().join("bin/node_modules/.bin/vitest"), "").unwrap();
        std::fs::write(repo.path().join("bin/a.test.ts"), "test('a', () => {});").unwrap();

        assert_eq!(runner_hint(repo.path(), "web/a.test.ts"), Some("vitest"));
        assert_eq!(runner_hint(repo.path(), "api/a.test.ts"), Some("jest"));
        assert_eq!(runner_hint(repo.path(), "bin/a.test.ts"), Some("vitest"));
        assert_eq!(runner_hint(repo.path(), "bare/a.test.ts"), None);

        let commands = super::super::stability::rerun_commands(
            Path::new("/cas"),
            repo.path(),
            &[id("bare/a.test.ts", "a")],
        );
        let argv: Vec<_> = commands
            .concat()
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect();
        assert_eq!(
            argv,
            ["jest", "vitest", "playwright"].map(|runner| format!(
                "cas factory worker-check --cas-root /cas -- {runner} bare/a.test.ts"
            ))
        );
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
    fn shared_paste_callback_selects_declared_expanded_cases_cas_13c9() {
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
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        let path = "paste-content.journey.ts";
        let source = r#"import { test, expect } from '@playwright/test';
for (const [width, height, colorScheme] of [[1440, 800, 'light'], [1440, 800, 'dark'], [390, 844, 'light'], [390, 844, 'dark']] as const) {
  test.describe(`composer paste ${width} ${colorScheme}`, () => {
    const run: Parameters<typeof test>[2] = async () => {
      expect(1 + 1).toBe(2);
    };
    if (width === 1440 && colorScheme === 'light') {
      // cas-stability-identities: ["composer paste 1440 light HUB-J24 paste content into the composer"]
      test('HUB-J24 paste content into the composer', run);
    } else {
      // cas-stability-identities: ["composer paste 1440 dark HUB-J24 paste content 1440 dark", "composer paste 390 light HUB-J24 paste content 390 light", "composer paste 390 dark HUB-J24 paste content 390 dark"]
      test(`HUB-J24 paste content ${width} ${colorScheme}`, { tag: '@part' }, run);
    }
  });
}
test('unrelated', () => { throw new Error('must remain unselected'); });
"#;
        git(&["init", "--quiet"]);
        std::fs::write(repo.path().join(path), source).unwrap();
        git(&["add", path]);
        git(&["commit", "--quiet", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(
            repo.path().join(path),
            source.replace("expect(1 + 1).toBe(2)", "expect(1 + 2).toBe(3)"),
        )
        .unwrap();
        git(&["add", path]);
        git(&["commit", "--quiet", "-m", "shared body changed"]);
        let head = git(&["rev-parse", "HEAD"]);
        let expected = [
            id(
                path,
                "composer paste 1440 dark HUB-J24 paste content 1440 dark",
            ),
            id(
                path,
                "composer paste 1440 light HUB-J24 paste content into the composer",
            ),
            id(
                path,
                "composer paste 390 dark HUB-J24 paste content 390 dark",
            ),
            id(
                path,
                "composer paste 390 light HUB-J24 paste content 390 light",
            ),
        ];
        assert_eq!(
            changed_tests(repo.path(), &base, &head, &[path.into()]).unwrap(),
            expected
        );
        for invalid in [
            source.replace("const run:", "let run:"),
            source.replace("}, run);", "}, missing);"),
            source.replace(
                "async () => {\n      expect(1 + 1).toBe(2);\n    }",
                "importedCallback",
            ),
            source.replace(
                "[\"composer paste 1440 light HUB-J24 paste content into the composer\"]",
                "[\"duplicate_id\", \"duplicate_id\"]",
            ),
            source.replace(
                "[\"composer paste 1440 light HUB-J24 paste content into the composer\"]",
                "[]",
            ),
        ] {
            assert!(source_tests(path, &invalid, &[(1, usize::MAX)]).is_err());
        }
        let shadowed = "const run = async () => {};\ntest.describe('suite', (run) => {\n// cas-stability-identities: [\"suite case_1\"]\ntest('case_1', run);\n});";
        assert!(source_tests(path, shadowed, &[(1, usize::MAX)]).is_err());
        let duplicate_sites = "const run = async () => {};\n// cas-stability-identities: [\"case_1\"]\ntest('case_1', run);\n// cas-stability-identities: [\"case_1\"]\ntest('case_2', run);";
        assert!(source_tests(path, duplicate_sites, &[(1, usize::MAX)]).is_err());
        for changed in [
            source.replace("1440, 800", "1280, 800"),
            source.replace("width === 1440", "width === 1280"),
        ] {
            std::fs::write(repo.path().join(path), changed).unwrap();
            git(&["add", path]);
            git(&["commit", "--quiet", "-m", "control header changed"]);
            assert_eq!(
                changed_tests(
                    repo.path(),
                    &base,
                    &git(&["rev-parse", "HEAD"]),
                    &[path.into()]
                )
                .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn declared_playwright_discovery_requires_exact_registration_expansions_cas_13c9() {
        let path = "hub-web/e2e/paste.journey.ts";
        let sites = [DeclaredSite {
            line: 8,
            column: 5,
            names: vec!["suite case_1".into(), "suite case_2".into()],
        }];
        let spec = |name: &str| {
            json!({"title":name,"file":"paste.journey.ts","line":8,"column":5,
            "tests":[{"projectId":"journeys","results":[]}]})
        };
        let report = json!({"config":{"rootDir":"/project/hub-web/e2e"},"errors":[],"suites":[
            {"title":"paste.journey.ts","column":0,"specs":[],"suites":[
                {"title":"suite","column":1,"specs":[spec("case_1"),spec("case_2")],"suites":[]}]}]});
        validate_declared_discovery(Path::new("/project"), &report, path, &sites).unwrap();
        for variant in 0..6 {
            let mut invalid = report.clone();
            let specs = invalid["suites"][0]["suites"][0]["specs"]
                .as_array_mut()
                .unwrap();
            match variant {
                0 => {
                    specs.pop();
                }
                1 => {
                    specs.push(spec("case_extra"));
                }
                2 => {
                    specs[0]["line"] = json!(9);
                }
                3 => {
                    specs[0]["column"] = json!(6);
                }
                4 => {
                    specs.push(specs[0].clone());
                }
                5 => {
                    let mut misplaced = specs[0].clone();
                    misplaced["line"] = json!(9);
                    specs.push(misplaced);
                }
                _ => unreachable!(),
            }
            assert!(
                validate_declared_discovery(Path::new("/project"), &invalid, path, &sites).is_err(),
                "variant {variant}"
            );
        }
        let mut second_project = report.clone();
        for spec in second_project["suites"][0]["suites"][0]["specs"]
            .as_array_mut()
            .unwrap()
        {
            spec["tests"]
                .as_array_mut()
                .unwrap()
                .push(json!({"projectId":"second","results":[]}));
        }
        validate_declared_discovery(Path::new("/project"), &second_project, path, &sites).unwrap();
        second_project["suites"][0]["suites"][0]["specs"][0]["tests"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(
            validate_declared_discovery(Path::new("/project"), &second_project, path, &sites)
                .is_err()
        );
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
