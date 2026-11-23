//! Source-scoped JavaScript stability evidence. A failure is never retried.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tree_sitter::{Node, Parser};

use super::stability::{Identity, Proof};

mod importers;
mod web_attribution;

pub(crate) use importers::playwright_importers;

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

pub(super) fn javascript(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|ext| ext.to_str()),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" | "vue")
    )
}

pub(super) fn test_file(path: &str) -> bool {
    let stem = Path::new(path).file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    [".test", ".spec", ".journey", ".integration"].iter().any(|suffix| stem.ends_with(suffix))
        || path.split('/').any(|part| matches!(part, "__tests__" | "tests" | "test" | "e2e"))
}

/// Files a runner's default matcher runs as tests (GH #2116, #2127): a test
/// name suffix, or jest's `__tests__` directory; declarations never are.
/// `test_file` stays the wide net for files that may hold tests. A file only
/// that net catches is a support module: a `File` run of it finds no tests,
/// so it widens to related tests for Jest/Vitest, or to its package otherwise.
pub(super) fn test_match(path: &str, runner: Option<&str>) -> bool {
    let stem = Path::new(path).file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    !stem.ends_with(".d")
        && ([".test", ".spec", ".journey", ".integration"].iter().any(|suffix| stem.ends_with(suffix))
            || (runner == Some("jest") && path.split('/').any(|part| part == "__tests__")))
}

/// A scoped JavaScript stability command, however the shell names its runner.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Invocation {
    pub(super) runner: &'static str,
    /// The runner's `--config` file, relative to the invocation directory.
    pub(super) config: Option<String>,
    /// Test files relative to the invocation directory.
    pub(super) files: Vec<String>,
}

pub(super) const SHAPE: &str = "JavaScript stability runs one runner on its test files: `[npx|pnpm exec|yarn|bunx] vitest|jest|playwright [run|test] [--config <file>] <test files>`, `vitest related --run <source files>`, `jest --findRelatedTests <source files>` or `node --test <test files>`";

/// Package-manager prefixes and runner subcommands are spelling. The capped
/// run owns selection, widening caller name filters rather than trusting them.
pub(super) fn invocation(args: &[String]) -> Option<Invocation> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let args = match args.as_slice() {
        ["pnpm" | "yarn" | "npm", "exec", rest @ ..] => rest,
        ["npx" | "bunx" | "pnpx" | "pnpm" | "yarn", rest @ ..] => rest,
        rest => rest,
    };
    let (runner, rest) = match args {
        ["vitest", "related", rest @ ..] => ("vitest", rest),
        ["vitest", "run", rest @ ..] | ["vitest", rest @ ..] => ("vitest", rest),
        ["jest", rest @ ..] => ("jest", rest),
        ["playwright", "test", rest @ ..] | ["playwright", rest @ ..] => ("playwright", rest),
        // Node's built-in runner (cas-a4c3) takes no config file.
        ["node", "--test", rest @ ..] => ("node", rest),
        _ => return None,
    };
    let mut config = None;
    let mut files = Vec::new();
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        let value = match *arg {
            "--config" | "-c" => Some(*rest.next()?),
            "--runInBand" | "-i" if runner == "jest" => continue,
            "--run" if runner == "vitest" => continue,
            "--findRelatedTests" if runner == "jest" => continue,
            "--testNamePattern" | "-t" if runner == "jest" => {
                if rest.next()?.is_empty() { return None; }
                continue;
            }
            other if runner == "jest" && other.starts_with("--testNamePattern=") => continue,
            other => other.strip_prefix("--config="),
        };
        if let Some(value) = value {
            if config.is_some() || runner == "node" || !safe_path(value) {
                return None;
            }
            config = Some(value.to_string());
        } else if safe_path(arg) && javascript(arg) {
            files.push(arg.to_string());
        } else {
            return None;
        }
    }
    (!files.is_empty()).then_some(Invocation { runner, config, files })
}

pub(super) fn allowed_args(args: &[String]) -> bool {
    invocation(args).is_some()
}

/// Whether a refused command was meant as a JavaScript stability run.
pub(super) fn names_runner(args: &[String]) -> bool {
    args.iter().take(3).any(|arg| matches!(arg.as_str(), "vitest" | "jest" | "playwright"))
        || args.windows(2).any(|pair| pair[0] == "node" && pair[1] == "--test")
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
/// `base` is the scoped proof base and only the delivered `head` is parsed:
/// intermediate commits never count. `owned`, when known, keeps only head
/// lines blamed on the task's own commits. A base revision that does not
/// parse has no prior tests to compare. Dynamic titles fail closed.
pub(crate) fn changed_tests(
    repo: &Path,
    base: &str,
    head: &str,
    paths: &[String],
    owned: Option<&BTreeSet<String>>,
) -> Result<Vec<Identity>> {
    let mut selected = BTreeSet::new();
    for path in paths.iter().filter(|path| javascript(path)) {
        if !safe_path(path) {
            bail!("invalid JavaScript stability path: {path}");
        }
        if super::git(repo, &["ls-tree", "--name-only", head, "--", path])?.is_empty() {
            continue;
        }
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
        let mut lines = changed_lines(&diff)?;
        if let Some(owned) = owned {
            let authored = authored_lines(repo, base, head, path, owned)?;
            lines = lines
                .into_iter()
                .flat_map(|(start, end)| {
                    if start > end {
                        // A pure deletion has no surviving line to blame.
                        return vec![(start, end)];
                    }
                    (start..=end)
                        .filter(|line| authored.contains(line))
                        .map(|line| (line, line))
                        .collect()
                })
                .collect();
        }
        // A restored file has no delivered test change. Do not require
        // runtime identities for callbacks merely present in its final tree.
        if lines.is_empty() {
            continue;
        }
        // Vue's template/SFC grammar is not JavaScript test registration.
        // Its runner owns dependency discovery, including the Vue transform.
        if path.ends_with(".vue") {
            bail!("Vue component {path} requires related-test proof");
        }
        let source = String::from_utf8(show(repo, head, path)?.context(format!("cannot read {head}:{path}"))?)?;
        let old = show(repo, base, path)?
            .map(String::from_utf8)
            .transpose()?
            .unwrap_or_default();
        let test_file = self::test_file(path);
        let all = match parsed_tests(path, &source, &[(1, usize::MAX)], false) {
            Ok(all) => all,
            // The caller widens an unclassifiable source to related/package proof.
            Err(error) => return Err(error),
        };
        let only_deletions = lines.iter().all(|(start, end)| start > end);
        if test_file && all.is_empty() && !source.trim().is_empty() && !only_deletions {
            bail!(
                "cannot resolve tests in {path}: test factories require explicit source-to-runtime identities"
            );
        }
        // The unchanged base can hold a construct the parser cannot read; only
        // the delivered tip must parse (cas-6aa9). No base inventory means
        // every changed test is new.
        let previous: BTreeMap<_, _> = parsed_tests(path, &old, &[(1, usize::MAX)], false)
            .unwrap_or_default()
            .into_iter()
            .collect();
        for (identity, body) in parsed_tests(path, &source, &lines, true)? {
            // Hunk lines may contain several calls. Repeating an unchanged
            // neighbour would violate the delivery-only selection contract.
            if previous.get(&identity) != Some(&body) {
                selected.insert(identity);
            }
        }
    }
    Ok(selected.into_iter().collect())
}

fn show(repo: &Path, revision: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(repo)
        .args(["show", &format!("{revision}:{path}")])
        .output()?;
    Ok(output.status.success().then_some(output.stdout))
}

/// Head lines whose blame, bounded at the proof base, is a task commit.
fn authored_lines(
    repo: &Path,
    base: &str,
    head: &str,
    path: &str,
    owned: &BTreeSet<String>,
) -> Result<BTreeSet<usize>> {
    let blame = super::git(
        repo,
        &["blame", "--porcelain", &format!("{base}..{head}"), "--", path],
    )?;
    let mut lines = BTreeSet::new();
    for header in blame.lines().filter(|line| !line.starts_with('\t')) {
        let mut fields = header.split(' ');
        if let (Some(commit), Some(_), Some(line)) = (fields.next(), fields.next(), fields.next())
            && commit.len() >= 40
            && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            && owned.contains(commit)
        {
            lines.insert(line.parse()?);
        }
    }
    Ok(lines)
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
            // A deletion is a gap after `start`, not a surviving changed line.
            // Reversed bounds intersect a containing scope but neither neighbour.
            Ok(if count == 0 { (start + 1, start) } else { (start, start + count - 1) })
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
    if raw.starts_with('"')
        && let Ok(decoded) = serde_json::from_str(raw)
    {
        return Some(decoded);
    }
    if raw.starts_with('`') && contents.contains("${") {
        return None;
    }
    unescape(contents)
}

/// Simple single-character JavaScript escapes (GH #2093: `'it\'s'`); anything
/// else, such as `\u0041` or a line continuation, is not a static identity.
fn unescape(contents: &str) -> Option<String> {
    let mut decoded = String::with_capacity(contents.len());
    let mut chars = contents.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        decoded.push(match chars.next()? {
            escaped @ ('\'' | '"' | '`' | '\\' | '/' | '$') => escaped,
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            _ => return None,
        });
    }
    Some(decoded)
}

/// GH #2112: a suite title built at runtime (`describe(api, …)`, a template
/// with `${…}`) names a family of suites. Literal parts are kept and each
/// dynamic part becomes a `%s` placeholder of the parameterized identity.
/// `None` for a static title.
fn dynamic_suite_title(node: Option<Node<'_>>, source: &str) -> Option<String> {
    let Some(node) = node else { return Some("%s".into()) };
    if title(node, source).is_some() {
        return None;
    }
    if node.kind() != "template_string" {
        return Some("%s".into());
    }
    let mut cursor = node.walk();
    let mut pattern = String::new();
    for part in node.children(&mut cursor) {
        match part.kind() {
            "string_fragment" | "string_content" => pattern.push_str(&unescape(text(part, source))?),
            "template_substitution" => pattern.push_str("%s"),
            "escape_sequence" => pattern.push_str(&unescape(text(part, source))?),
            _ => {}
        }
    }
    Some(pattern)
}

/// `(suite, supported, parameterized)` for a test registration call.
/// `it.each(table)(title, fn)` and its tagged-template form register one
/// runtime test per row; their title is a placeholder template.
fn declaration(
    node: Node<'_>,
    source: &str,
    aliases: &BTreeMap<String, String>,
) -> Option<(bool, bool, bool)> {
    let mut function = node.child_by_field_name("function")?;
    let mut parameterized = false;
    if function.kind() == "call_expression"
        && let Some(inner) = function.child_by_field_name("function")
        && inner.kind() == "member_expression"
        && text(inner, source).ends_with(".each")
    {
        function = inner;
        parameterized = true;
    }
    let (suite, supported) = callee(function, source, aliases, parameterized)?;
    Some((suite, supported, parameterized))
}

/// cas-6a3c: `(cond ? describe : describe.skip)(…)` declares a suite when both
/// branches declare the same kind; jest names its tests with that title.
fn callee(function: Node<'_>, source: &str, aliases: &BTreeMap<String, String>, parameterized: bool) -> Option<(bool, bool)> {
    match function.kind() {
        "call_expression" => {
            let inner = function.child_by_field_name("function")?;
            let property = inner.child_by_field_name("property")?;
            matches!(text(property, source), "skipIf" | "runIf")
                .then(|| callee(inner, source, aliases, parameterized)).flatten()
        }
        "parenthesized_expression" => callee(function.named_child(0)?, source, aliases, parameterized),
        "ternary_expression" => {
            let (suite, supported) = callee(function.child_by_field_name("consequence")?, source, aliases, parameterized)?;
            let (other, also) = callee(function.child_by_field_name("alternative")?, source, aliases, parameterized)?;
            (suite == other).then_some((suite, supported && also))
        }
        _ => {
            let name = text(function, source);
            named_declaration(if parameterized { name.strip_suffix(".each")? } else { name }, aliases)
        }
    }
}

/// A conditional callee that names the test API yet resolves to no single kind.
fn unresolved_callee(function: Node<'_>, source: &str, aliases: &BTreeMap<String, String>) -> bool {
    (matches!(function.kind(), "parenthesized_expression" | "ternary_expression")
        || text(function, source).split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .any(|word| aliases.get(word).is_some_and(|api| api == UNRESOLVED_API)))
        && callee(function, source, aliases, false).is_none()
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
                    | "skipIf"
                    | "runIf"
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
        .parse(mask_type_imports(source).as_ref(), None)
        .with_context(|| format!("JavaScript test parse of {path}"))?;
    if let Some(node) = first_parse_error(tree.root_node()) {
        let at = node.start_position();
        return Err(ParseError {
            path: path.to_string(),
            line: at.row + 1,
            column: at.column + 1,
            missing: node.is_missing(),
            near: source
                .get(node.start_byte()..node.end_byte())
                .unwrap_or_default()
                .split('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .take(40)
                .collect(),
        }
        .into());
    }
    Ok(tree)
}

/// A file the capped runner cannot parse, located for the person fixing it (cas-6aa9).
#[derive(Debug)]
pub(crate) struct ParseError {
    path: String,
    line: usize,
    column: usize,
    missing: bool,
    near: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ParseError { path, line, column, near, .. } = self;
        let kind = if self.missing { "missing token" } else { "unparsed" };
        write!(f, "cannot resolve changed tests in {path}: JavaScript parse error at {path}:{line}:{column} ({kind} `{near}`); rewrite that construct or move it out of the test file")
    }
}

impl std::error::Error for ParseError {}

/// The outermost unparsed or missing node, in source order.
fn first_parse_error(node: Node<'_>) -> Option<Node<'_>> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    if !node.has_error() {
        return None;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor).find_map(first_parse_error)
}

/// cas-6aa9: tree-sitter-typescript 0.23.2, the newest release, cannot parse
/// two valid type-argument forms: `import(…)` as a call's type argument
/// (`f<typeof import('m')>()`, pulse-card auth.test.ts:28) and type arguments
/// on a tagged template (`prisma.$queryRaw<Row[]>`…``, GH #2054). Blank the
/// first to an identifier and the second to spaces of the same byte length,
/// so every offset, line and test title is unchanged.
fn mask_type_imports(source: &str) -> std::borrow::Cow<'_, str> {
    static TYPE_IMPORT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"<\s*(?:typeof\s+)?(import\s*\(\s*(?:'[^'\n]*'|"[^"\n]*")\s*\))"#).unwrap()
    });
    static TEMPLATE_TYPE_ARGUMENTS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"[\w$](<[^<>`;=()\n]*(?:<[^<>`;=()\n]*>[^<>`;=()\n]*)*>)\s*`").unwrap()
    });
    let imports = TYPE_IMPORT.replace_all(source, |caps: &regex::Captures<'_>| {
        let (whole, query) = (&caps[0], caps.get(1).unwrap());
        let start = query.start() - caps.get(0).unwrap().start();
        let masked = query.as_str().bytes().enumerate().map(|(index, byte)|
            if matches!(byte, b'\n' | b'\r') { byte as char } else if index == 0 { '_' } else { ' ' }).collect::<String>();
        format!("{}{}", &whole[..start], masked)
    });
    if !TEMPLATE_TYPE_ARGUMENTS.is_match(&imports) {
        return imports;
    }
    std::borrow::Cow::Owned(TEMPLATE_TYPE_ARGUMENTS.replace_all(&imports, |caps: &regex::Captures<'_>| {
        let (whole, arguments) = (&caps[0], caps.get(1).unwrap());
        let start = arguments.start() - caps.get(0).unwrap().start();
        format!("{}{}{}", &whole[..start], " ".repeat(arguments.len()), &whole[start + arguments.len()..])
    }).into_owned())
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
                if declaration(scope, source, aliases).is_some_and(|(suite, _, _)| suite) =>
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
                .is_some_and(|(suite, supported, _)| !suite && supported)
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

const UNRESOLVED_API: &str = "unresolved-test-api";

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
    {
        let api = match callee(value, source, aliases, false) {
            Some((suite, true)) => Some(if suite { "describe" } else { "test" }),
            _ if unresolved_callee(value, source, aliases) => Some(UNRESOLVED_API),
            _ => None,
        };
        if let Some(api) = api { aliases.insert(text(name, source).to_string(), api.to_string()); }
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

/// Explicit disabled registrations do not promise a passing runtime result.
/// Conditional skipIf/runIf and mixed conditional aliases still owe real proof.
fn skipped_registration(function: Node<'_>, source: &str) -> bool {
    match function.kind() {
        "member_expression" => {
            function.child_by_field_name("property").is_some_and(|property|
                matches!(text(property, source), "skip" | "todo" | "fixme"))
                || function.child_by_field_name("object").is_some_and(|object| skipped_registration(object, source))
        }
        "call_expression" => function.child_by_field_name("function").is_some_and(|callee| skipped_registration(callee, source)),
        "parenthesized_expression" => function.named_child(0).is_some_and(|inner| skipped_registration(inner, source)),
        _ => false,
    }
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
    templated: bool,
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
        && let Some((suite, supported, each)) = declaration(node, source, aliases)
    {
        if node.child_by_field_name("function").is_some_and(|function| skipped_registration(function, source)) {
            return Ok(());
        }
        let args = node
            .child_by_field_name("arguments")
            .context("test arguments")?;
        let first = args.named_child(0);
        let mut name = first.and_then(|name| title(name, source));
        let mut templated = templated || each;
        if suite && supported && name.is_none() && let Some(pattern) = dynamic_suite_title(first, source) {
            name = Some(pattern);
            templated = true;
        }
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
                    tests.push((Identity::exact(path, name), fingerprint.clone()));
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
        let external_changed = external_callback && {
            let mut cursor = args.walk();
            force || overlaps(node, lines)
                || args.named_children(&mut cursor).skip(1)
                    .filter(|arg| arg.kind() == "identifier")
                    .any(|argument| callback_scope_changed(tree_root(node), text(argument, source), source, lines))
                || control_headers(node, source, aliases).iter().any(|(first, last, _)|
                    lines.iter().any(|(start, end)| start <= last && end >= first))
        };
        if strict && (external_changed || (suite && supported && callback.is_none() && (force || overlaps(node, lines)))) {
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
                    templated,
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
                        parameterized: templated,
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
            child, source, path, lines, suites, unresolved, templated, force, strict, aliases,
            tests,
        )?;
    }
    Ok(())
}

fn tree_root(mut node: Node<'_>) -> Node<'_> {
    while let Some(parent) = node.parent() { node = parent; }
    node
}

/// An unchanged registration can still owe proof when its callback changed.
/// Include unsupported binding forms conservatively; discovery will widen them.
fn callback_scope_changed(node: Node<'_>, name: &str, source: &str, lines: &[(usize, usize)]) -> bool {
    if matches!(node.kind(), "variable_declarator" | "function_declaration" | "generator_function_declaration" | "import_statement")
        && (node.child_by_field_name("name").is_some_and(|binding| contains_binding(binding, name, source))
            || (node.kind() == "import_statement" && contains_binding(node, name, source)))
        && overlaps(node, lines)
    {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(|child| callback_scope_changed(child, name, source, lines))
}

pub(crate) fn execute(cas_root: &Path, args: &[String], cwd: &Path, test_env: &[String]) -> Result<()> {
    let invocation = invocation(args).context(SHAPE)?;
    if args.iter().any(|arg| arg == "-t" || arg.starts_with("--testNamePattern")) {
        eprintln!("Widening the caller's name filter to the capped changed-test selection");
    }
    let repo = PathBuf::from(super::git(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    // Paths are as the shell sees them: relative to the invocation directory.
    let cwd = cwd.canonicalize()?;
    let relative = |path: &str| -> Result<String> {
        cwd.join(path)
            .strip_prefix(&repo)
            .ok()
            .and_then(Path::to_str)
            .filter(|path| safe_path(path))
            .map(str::to_owned)
            .with_context(|| format!("{path} is outside the repository"))
    };
    let files = invocation.files.iter().map(|file| relative(file)).collect::<Result<Vec<_>>>()?;
    let runner_config = invocation
        .config
        .as_deref()
        .map(|config| {
            let mut config = repo.join(relative(config)?);
            if !config.is_file() && let Some(file) = files.first() {
                let (package, _) = runner_root(&repo, file, invocation.runner)?;
                config = package.join(invocation.config.as_deref().expect("config"));
            }
            if !config.is_file() {
                bail!("{} config not found: {}", invocation.runner, config.display());
            }
            Ok(config)
        })
        .transpose()?;
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
    let mut tests = Vec::new();
    let mut widened = BTreeMap::new();
    for file in &files {
        match changed_tests(&repo, &base, &head, std::slice::from_ref(file), None) {
            Ok(selected) if !selected.is_empty() => tests.extend(selected),
            result => {
                let scope = super::widened::scope(&repo, file);
                let cause = result.err().map(|error| format!("{error:#}")).unwrap_or_else(|| "no static changed-test selection".into());
                eprintln!("Widening {file} to {scope:?} for one capped pass: {cause}");
                widened.entry(scope).or_insert_with(|| (file.clone(), cause));
            }
        }
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
    for (scope, (file, cause)) in &widened {
        execute_tests(cas_root, &repo, &head, invocation.runner, runner_config.as_deref(), &[], Some((scope, file, cause)), test_env)?;
    }
    if tests.is_empty() { return Ok(()); }
    let proof = execute_tests(cas_root, &repo, &head, invocation.runner, runner_config.as_deref(), &tests, None, test_env)?;
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

/// Whether the local Jest, run with `config`, lists `spec`: Jest's own
/// discovery (`roots`, `projects`, `testMatch`, `testRegex`) decides ownership.
fn jest_config_selects(repo: &Path, dir: &Path, config: &Path, spec: &Path) -> bool {
    let Ok(jest) = runner_path(repo, dir, "jest") else { return false };
    let Ok(output) = Command::new(jest)
        .current_dir(dir)
        .env("CI", "1")
        .arg("--config")
        .arg(config)
        .args(["--listTests", "--json"])
        .output()
    else {
        return false;
    };
    let (Ok(listed), Ok(spec)) = (serde_json::from_slice::<Vec<String>>(&output.stdout), spec.canonicalize()) else {
        return false;
    };
    output.status.success()
        && listed.iter().any(|path| Path::new(path).canonicalize().is_ok_and(|path| path == spec))
}

/// The repository-relative Jest config that lists `path`, when Jest can be asked.
pub(crate) fn jest_owner(repo: &Path, path: &str) -> Option<PathBuf> {
    let (dir, config) = runner_root(repo, path, "jest").ok()?;
    let config = config?;
    if !jest_config_selects(repo, &dir, &config, &repo.join(path)) {
        return None;
    }
    config.strip_prefix(repo).ok().map(Path::to_path_buf)
}

/// The runner configs in `dir`, nearest default first. Jest repositories keep
/// further configs beside it (`jest-integration.config.js`) for other specs.
fn runner_configs(dir: &Path, runner: &str) -> Vec<PathBuf> {
    let mut configs: Vec<PathBuf> = runner_config(dir, runner).into_iter().collect();
    if runner == "jest" {
        let mut extra: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.contains("jest") && name.contains(".config.") && !JEST_CONFIGS.contains(&name.as_str())
            })
            .map(|entry| entry.path())
            .collect();
        extra.sort();
        configs.extend(extra);
    }
    configs
}

/// The runner's working directory and explicit config for a test file: the
/// nearest ancestor holding that runner's config, else the nearest
/// package.json. A nested package.json must not hide the config and its
/// transforms (cas-1468). For Jest the config must select the file: a spec
/// outside the default config's `testRegex` belongs to the config that
/// matches it (GH #2058); with no such config the nearest one stays.
fn runner_root(repo: &Path, path: &str, runner: &str) -> Result<(PathBuf, Option<PathBuf>)> {
    let spec = repo.join(path);
    let mut current = spec.parent().context("test parent")?.to_path_buf();
    let (mut package, mut nearest) = (None, None);
    loop {
        for config in runner_configs(&current, runner) {
            if runner != "jest" || jest_config_selects(repo, &current, &config, &spec) {
                return Ok((current, Some(config)));
            }
            nearest.get_or_insert_with(|| (current.clone(), Some(config)));
        }
        if package.is_none() && current.join("package.json").is_file() {
            package = Some(current.clone());
        }
        if current == repo || !current.pop() {
            return nearest
                .or_else(|| package.map(|package| (package, None)))
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
    // GH #2055: a node:test file beside a Jest config is Node's own runner.
    if source.contains("'node:test'") || source.contains("\"node:test\"") {
        return Some("node --test");
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
    runner_config: Option<&Path>,
    tests: &[Identity],
    widened: Option<(&super::widened::Scope, &str, &str)>,
    test_env: &[String],
) -> Result<Proof> {
    let environment = test_env.iter().map(|spec| {
        super::javascript_test_environment_spec(spec).context("invalid JavaScript test input")
    }).collect::<Result<Vec<_>>>()?;
    let start = Instant::now();
    let config = crate::config::Config::load(cas_root)?.factory();
    let high = crate::factory_build_memory::memory_high(
        config.worker_memory_high_gib,
        config.max_concurrent_builders,
        &std::fs::read_to_string("/proc/meminfo").unwrap_or_default(),
    );
    let mut by_file: BTreeMap<&str, Vec<&Identity>> = BTreeMap::new();
    for test in tests {
        by_file.entry(&test.path).or_default().push(test);
    }
    // GH #2146: a Playwright helper holds no tests; each spec importing it
    // runs under the config that owns that spec.
    let consumers = match widened {
        Some((super::widened::Scope::Related(helper), _, _)) if runner == "playwright" => {
            let specs = playwright_importers(repo, helper);
            if specs.is_empty() { bail!(crate::gates::tag("G-61", format!("no Playwright spec imports {helper}"))); }
            eprintln!("Running {helper} through its importing specs: {}", specs.join(" "));
            specs
        }
        _ => Vec::new(),
    };
    if let Some((scope, file, _)) = widened {
        super::widened::invalidate(cas_root, repo, head, scope)?;
        if consumers.is_empty() { by_file.insert(file, Vec::new()); }
        for spec in &consumers { by_file.insert(spec, Vec::new()); }
    }
    let mut configs = BTreeMap::new();
    let mut count = 0;
    let mut attribution_notes = Vec::new();
    // GH #2172: a Playwright run writes its reports under the task's artifact
    // directory; a failure keeps them so the traces it printed stay readable.
    let reports = if runner == "playwright" {
        let parent = javascript_reports_parent(cas_root, repo);
        std::fs::create_dir_all(&parent)?;
        tempfile::Builder::new().prefix(&format!("playwright-{}-", head.get(..12).unwrap_or(head))).tempdir_in(parent)?
    } else {
        tempfile::tempdir()?
    };
    let outcome = (|| -> Result<()> {
    for (path, selected) in by_file {
        // One config path: the caller's own `--config` wins over the
        // discovered runner config (cas-1468); never pass both.
        let (package, discovered) = if let Some((super::widened::Scope::Package(directory), _, _)) = widened {
            let package = repo.join(directory);
            let config = self::runner_config(&package, runner);
            (package, config)
        } else { runner_root(repo, path, runner)? };
        let discovered = if consumers.is_empty() || runner_config.is_some() { discovered } else {
            Some(playwright_owner(repo, path, &package, &environment, high, reports.path())?)
        };
        let used = runner_config.map(Path::to_path_buf).or(discovered);
        if runner == "playwright" && matches!(widened, Some((super::widened::Scope::File(_) | super::widened::Scope::Related(_), _, _)))
            && let Some(config) = used.as_deref().and_then(|config| config.strip_prefix(repo).ok()).and_then(Path::to_str)
        {
            configs.insert(path.to_string(), config.to_string());
        }
        let config: Vec<std::ffi::OsString> = used
            .into_iter()
            .flat_map(|config| ["--config".into(), config.into_os_string()])
            .collect();
        let executable = if runner == "node" {
            PathBuf::from("node")
        } else {
            runner_path(repo, &package, runner)?
        };
        let source = std::fs::read_to_string(repo.join(path))?;
        let sites = if widened.is_none() { declared_sites(path, &source)? } else { Vec::new() };
        if !sites.is_empty() {
            if runner != "playwright" {
                bail!("declared stability identities require native Playwright discovery");
            }
            let report = reports.path().join("discovery.json");
            let mut command = Command::new(&executable);
            command
                .envs(environment.iter().copied())
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
        let pattern = selected
            .iter()
            .map(|test| format!("(?:{})", test.name_pattern()))
            .collect::<Vec<_>>()
            .join("|");
        let pattern = if runner == "playwright" {
            format!("(?:^|\\s)(?:{pattern})$")
        } else {
            format!("^(?:{pattern})$")
        };
        let selected: Vec<_> = selected.into_iter().cloned().collect();
        let run = |iteration| -> Result<(bool, Value)> {
            let report = reports.path().join(format!("report-{iteration}.json"));
            let mut command = Command::new(&executable);
            command.current_dir(&package).env("CI", "1").envs(environment.iter().copied());
            let file = repo.join(path);
            match runner {
                "vitest" => {
                    if matches!(widened, Some((super::widened::Scope::Related(_), _, _))) {
                        command.args(["related", "--run"]);
                    } else { command.arg("run"); }
                    command
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
                        .arg(&report);
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
                        .arg(&report);
                }
                "playwright" => {
                    command
                        .arg("test")
                        .args(&config)
                        .args([
                            "--repeat-each",
                            if widened.is_some() { "1" } else { "5" },
                            "--retries",
                            "0",
                            "--max-failures",
                            if widened.is_some() { "0" } else { "1" },
                            "--workers",
                            "1",
                            "--reporter=json",
                            "--output",
                        ])
                        .arg(reports.path().join("playwright-output"))
                        .env("PLAYWRIGHT_JSON_OUTPUT_FILE", &report);
                }
                "node" => {
                    let reporter = reports.path().join("node-test-reporter.mjs");
                    std::fs::write(&reporter, NODE_TEST_REPORTER)?;
                    command
                        .args(["--test", "--test-concurrency=1", "--test-reporter=spec"])
                        .arg("--test-reporter-destination=stdout")
                        .arg(format!("--test-reporter={}", reporter.display()))
                        .arg(format!("--test-reporter-destination={}", report.display()));
                }
                _ => bail!("unsupported stability runner: {runner}"),
            }
            if widened.is_none() {
                match runner {
                    "node" => { command.arg(format!("--test-name-pattern={pattern}")); }
                    "playwright" => { command.args(["--grep", &pattern]); }
                    _ => { command.args(["--testNamePattern", &pattern]); }
                }
            }
            if !matches!(widened, Some((super::widened::Scope::Package(_), _, _))) {
                if runner == "jest" {
                    command.arg(if matches!(widened, Some((super::widened::Scope::Related(_), _, _))) {
                        "--findRelatedTests"
                    } else { "--runTestsByPath" });
                }
                if runner == "playwright" { command.arg(regex::escape(&file.to_string_lossy())); }
                else { command.arg(&file); }
            }
            let marker = reports.path().join(format!("scope-{iteration}"));
            let success = run_scoped_javascript(command, high, &marker)?;
            let value: Value =
                serde_json::from_slice(&std::fs::read(&report).with_context(|| {
                    format!("{runner} did not produce a stability report for {path}")
                })?)?;
            Ok((success, value))
        };
        count += if widened.is_some() && runner == "playwright" {
            let (success, report) = run(0)?;
            let inherited = if success { BTreeSet::new() } else {
                let (inherited, note) = web_attribution::attribute(cas_root, repo, head, &package,
                    &executable, &config, &report, &environment, high)
                    .map_err(|error| anyhow::anyhow!(crate::gates::tag("G-61", format!("{error:#}"))))?;
                attribution_notes.push(note);
                inherited
            };
            validate_report_with_inherited(runner, repo, &report, &selected, 1, &inherited,
                if consumers.is_empty() { &[] } else { std::slice::from_ref(&path) })?
        } else { verify_repeats(runner, repo, &selected, run)? };
    }
    Ok(())
    })();
    if let Err(error) = outcome {
        if runner == "vitest" {
            return Err(match retain_vitest_reports(cas_root, repo, head, reports.path()) {
                Ok(kept) => anyhow::anyhow!(
                    "{error:#}\nFailed Vitest assertion reports (redacted JSON) are kept at {}.",
                    kept.display()
                ),
                Err(capture) => anyhow::anyhow!(
                    "{error:#}\nCould not retain Vitest assertion reports: {capture:#}"
                ),
            });
        }
        if runner != "playwright" {
            return Err(error);
        }
        // The failure stays first; the kept directory follows it.
        let kept = reports.keep();
        return Err(anyhow::anyhow!(
            "{error:#}\nFailed Playwright evidence (traces, error contexts, JSON reports) is kept at {}.",
            kept.display()
        ));
    }
    if super::clean_head(repo)? != head {
        bail!("repository changed during JavaScript stability run");
    }
    let proof = Proof {
        runner: runner.to_string(),
        tests: tests.to_vec(),
        consecutive_passes: if widened.is_some() { 1 } else { 5 },
        elapsed_ms: start.elapsed().as_millis(),
    };
    if let Some((scope, _, cause)) = widened {
        super::widened::record(cas_root, repo, head, super::widened::Proof { runner: runner.into(), scope: scope.clone(), reason: format!("widened:{cause} {}", attribution_notes.join("; ")), configs }, count as u64)?;
        println!("widened: PASS {head} {scope:?} {count} tests; passes=1");
    } else { super::stability::record(cas_root, repo, head, proof.clone())?; }
    Ok(proof)
}

/// Copy only the report protocol payload; temporary scope files stay private
/// and success receipts retain their existing lifecycle.
fn retain_vitest_reports(cas_root: &Path, repo: &Path, head: &str, reports: &Path) -> Result<PathBuf> {
    let parent = javascript_reports_parent(cas_root, repo);
    std::fs::create_dir_all(&parent)?;
    let kept = tempfile::Builder::new()
        .prefix(&format!("vitest-{}-", head.get(..12).unwrap_or(head)))
        .tempdir_in(parent)?;
    let mut count = 0;
    for entry in std::fs::read_dir(reports)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.file_name().to_string_lossy().starts_with("report-")
            && entry.path().extension().is_some_and(|extension| extension == "json")
        {
            let report: Value = serde_json::from_slice(&std::fs::read(entry.path())?)?;
            let redacted = crate::ai_enrichment::redact_json(&report);
            std::fs::write(kept.path().join(entry.file_name()), serde_json::to_vec_pretty(&redacted)?)?;
            count += 1;
        }
    }
    anyhow::ensure!(count > 0, "no Vitest JSON report was produced");
    Ok(kept.keep())
}

/// Where failure reports live: the task's artifact directory, or a shared
/// worker-check one when no task owns the checkout.
fn javascript_reports_parent(cas_root: &Path, repo: &Path) -> PathBuf {
    let configured = crate::config::Config::load(cas_root).ok().and_then(|config| config.factory().artifacts_root.clone());
    let base = crate::config::project_factory_artifacts_root(
        cas_root,
        &crate::config::resolved_factory_artifacts_root(configured.as_deref()),
    );
    let owner = crate::factory_capacity_wait::bound_task(cas_root, repo).unwrap_or_else(|| "worker-check".into());
    base.join(owner).join("worker-check")
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

/// GH #2146: the Playwright config that lists `spec`. Configs beside the
/// spec's nearest default, `playwright.responsive.config.ts` among them, each
/// own different specs; Playwright's own `--list` decides, nearest directory
/// first and a directory's default before its siblings.
fn playwright_owner(
    repo: &Path, spec: &str, package: &Path, environment: &[(&str, &str)], high: u64, scratch: &Path,
) -> Result<PathBuf> {
    let executable = runner_path(repo, package, "playwright")?;
    let mut directory = repo.join(spec);
    while directory.pop() && directory.starts_with(repo) {
        let mut configs: Vec<PathBuf> = runner_config(&directory, "playwright").into_iter().collect();
        let mut extra: Vec<PathBuf> = std::fs::read_dir(&directory).into_iter().flatten().flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.contains("playwright") && name.contains(".config.") && !PLAYWRIGHT_CONFIGS.contains(&name.as_str())
            })
            .map(|entry| entry.path()).collect();
        extra.sort();
        configs.extend(extra);
        for config in configs {
            let report = scratch.join("owner.json");
            let _ = std::fs::remove_file(&report);
            let mut command = Command::new(&executable);
            command.envs(environment.iter().copied()).current_dir(&directory).env("CI", "1")
                .arg("test").arg("--config").arg(&config).args(["--list", "--reporter=json"])
                .arg(regex::escape(&repo.join(spec).to_string_lossy()))
                .env("PLAYWRIGHT_JSON_OUTPUT_FILE", &report);
            if !run_scoped_javascript(command, high, &scratch.join("owner-scope"))? { continue; }
            let Ok(listed) = std::fs::read(&report).map_err(anyhow::Error::from)
                .and_then(|bytes| Ok(serde_json::from_slice::<Value>(&bytes)?)) else { continue };
            let root = listed.get("config").and_then(|config| config.get("rootDir")).and_then(Value::as_str)
                .map(Path::new).unwrap_or(repo);
            let mut files = BTreeSet::new();
            listed_files(repo, root, listed.get("suites").unwrap_or(&Value::Null), &mut files);
            if files.contains(spec) { return Ok(config); }
        }
        if directory == repo { break; }
    }
    bail!(crate::gates::tag("G-61", format!("no Playwright config lists {spec}")))
}

fn listed_files(repo: &Path, root: &Path, suites: &Value, files: &mut BTreeSet<String>) {
    for suite in suites.as_array().into_iter().flatten() {
        for spec in suite.get("specs").and_then(Value::as_array).into_iter().flatten() {
            if let Some(file) = spec.get("file").and_then(Value::as_str)
                && let Ok(file) = report_path(repo, &root.join(file).to_string_lossy())
            {
                files.insert(file);
            }
        }
        listed_files(repo, root, suite.get("suites").unwrap_or(&Value::Null), files);
    }
}

fn verify_repeats(
    runner: &str,
    repo: &Path,
    tests: &[Identity],
    mut run: impl FnMut(usize) -> Result<(bool, Value)>,
) -> Result<usize> {
    let mut count = 0;
    for iteration in 0..if runner == "playwright" || tests.is_empty() { 1 } else { 5 } {
        let (success, report) = run(iteration)?;
        count = validate_report(
            runner,
            repo,
            &report,
            tests,
            if runner == "playwright" && !tests.is_empty() { 5 } else { 1 },
        )?;
        if !success {
            bail!(
                "{runner} stability failed at iteration {}; no retry",
                iteration + 1
            );
        }
    }
    Ok(count)
}

fn validate_report(
    runner: &str,
    repo: &Path,
    report: &Value,
    tests: &[Identity],
    repeats: usize,
) -> Result<usize> {
    validate_report_with_inherited(runner, repo, report, tests, repeats, &BTreeSet::new(), &[])
}

/// `files` are the spec files a widened run must have executed, and the only
/// ones it may have: a missing or unrelated file is not proof (GH #2146).
fn validate_report_with_inherited(
    runner: &str, repo: &Path, report: &Value, tests: &[Identity], repeats: usize,
    inherited: &BTreeSet<(Identity, String)>, files: &[&str],
) -> Result<usize> {
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
            inherited,
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
                let id = Identity::exact(path.clone(), name);
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
                    Some("pending" | "skipped" | "todo" | "disabled")
                        if !tests.iter().any(|test| test.selects(&id)) => {}
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
    // A template identity is proved when every row it selected passed.
    for test in tests {
        let mut rows = results.iter().filter(|(ran, _)| test.selects(ran)).peekable();
        if rows.peek().is_none() || rows.any(|(_, count)| *count != repeats) {
            bail!(
                "{runner} stability requires {repeats} passing results: {} :: {}",
                test.path,
                test.name
            );
        }
    }
    if !tests.is_empty() && results.keys().any(|ran| !tests.iter().any(|test| test.selects(ran))) {
        bail!("{runner} ran tests outside the changed selection");
    }
    if results.is_empty() { bail!(crate::gates::tag("G-61", format!("{runner} executed zero tests"))); }
    for file in files {
        if !results.keys().any(|ran| ran.path == *file) {
            bail!(crate::gates::tag("G-61", format!("{runner} executed no test of the required spec {file}")));
        }
    }
    if let Some(ran) = results.keys().find(|ran| !files.is_empty() && !files.contains(&ran.path.as_str())) {
        bail!(crate::gates::tag("G-61", format!("{runner} ran {} outside the required specs", ran.path)));
    }
    Ok(results.len())
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
    inherited: &BTreeSet<(Identity, String)>,
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
            let id = Identity::exact(
                report_path(repo, file.to_str().context("spec path encoding")?)?,
                full.join(" "),
            );
            for test in spec
                .get("tests")
                .and_then(Value::as_array)
                .context("spec tests")?
            {
                let attempts = test
                    .get("results")
                    .and_then(Value::as_array)
                    .context("test results")?;
                let project = test.get("projectId").or_else(|| test.get("projectName"))
                    .and_then(Value::as_str).unwrap_or_default();
                if test.get("expectedStatus").and_then(Value::as_str) != Some("passed")
                    || attempts.len() != 1
                    || (attempts[0].get("status").and_then(Value::as_str) != Some("passed")
                        && !inherited.contains(&(id.clone(), project.to_string())))
                    || attempts[0]
                        .get("retry")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        != 0
                {
                    bail!("playwright stability failed: {} :: {}", id.path, id.name);
                }
                *results
                    .entry((id.clone(), project.to_string()))
                    .or_default() += 1;
            }
        }
        if let Some(nested) = suite.get("suites") {
            playwright_results(repo, root, nested, &names, results, inherited)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(path: &str, name: &str) -> Identity {
        Identity::exact(path, name)
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
        let invocation = invocation(&args).expect("node --test is a capped runner");
        let tests = changed_tests(repo.path(), &base, &head, &invocation.files, None).unwrap();
        assert_eq!(tests, [id(file, "server changed")]);
        let cas_root = tempfile::tempdir().unwrap();
        let proof = execute_tests(cas_root.path(), repo.path(), &head, invocation.runner, None, &tests, None, &[]).unwrap();
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
        let tests = changed_tests(repo.path(), &base, &head, &[file.to_string()], None).unwrap();
        assert_eq!(tests, [id(file, "aliased keeps consent"), id(file, "legal keeps consent")]);
        let cas_root = tempfile::tempdir().unwrap();
        let proof = execute_tests(cas_root.path(), repo.path(), &head, "node", None, &tests, None, &[]).unwrap();
        assert_eq!(proof.consecutive_passes, 5);

        for wrapper in ["runIf(true)", "skipIf(false)"] {
            let (suite, case) = ("legal", "consent");
            let source = format!("describe.{wrapper}('{suite}', () => {{ test('{case}', () => {{}}); }});");
            let selected = source_tests(file, &source, &[(1, 1)]).unwrap();
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].path, file);
            assert_eq!(selected[0].name.split(' ').collect::<Vec<_>>(), [suite, case]);
        }
        let unknown = "const suite = true ? describe : unknown; suite('legal', () => { test('consent', () => {}); });";
        assert!(source_tests(file, unknown, &[(1, 1)]).is_err());

        // Branches of different kinds cannot name a runtime identity.
        std::fs::write(repo.path().join(file), source("", prelude).replace("describe.skip)(", "test)(")).unwrap();
        git(&["commit", "--quiet", "-am", "mixed"]);
        let mixed = git(&["rev-parse", "HEAD"]);
        let refusal = changed_tests(repo.path(), &head, &mixed, &[file.to_string()], None).unwrap_err().to_string();
        assert!(refusal.contains("G-61"), "{refusal}");
    }

    #[test]
    fn inactive_prerequisite_skip_does_not_require_passing_evidence_gh2046() {
        let file = "legal.test.ts";
        let source = "describe('Signup legal consent', () => {\n\
            if (!databaseUrl) { test.skip('requires disposable DB', () => {}); return; }\n\
            test('stores consent', () => {});\n});";
        let selected = source_tests(file, source, &[(1, 4)]).unwrap();
        assert_eq!(selected, [id(file, "Signup legal consent stores consent")]);
        let report = serde_json::json!({"success":true,"testResults":[{"name":file,"status":"passed",
            "assertionResults":[{"fullName":"Signup legal consent stores consent","status":"passed"}]}]});
        validate_report("jest", Path::new("/project"), &report, &selected, 1).unwrap();
        let skipped = serde_json::json!({"success":true,"testResults":[{"name":file,"status":"passed",
            "assertionResults":[{"fullName":"Signup legal consent stores consent","status":"pending"}]}]});
        assert!(validate_report("jest", Path::new("/project"), &skipped, &selected, 1).is_err());
    }

    /// pulse-card auth.test.ts (GH #2052): `importOriginal<typeof import('#imports')>()`
    /// is valid TypeScript that tree-sitter-typescript 0.23.2 cannot parse, so
    /// the capped run refused the whole file. It now resolves its changed test;
    /// a real parse error names its path:line:column.
    #[test]
    fn typescript_type_import_argument_parses_and_parse_errors_name_their_location_cas_6aa9() {
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
        };
        git(&["init", "--quiet"]);
        let file = "auth.test.ts";
        // Both constructs pulse-card bisected: GH #2052 and GH #2054.
        let source = |body: &str| format!(
            "vi.mock('#imports', async importOriginal => ({{\n  ...(await importOriginal<typeof import('#imports')>()),\n}}));\n\
             describe('auth', () => {{\n  test('signs out', async () => {{ {body} await prisma.$queryRaw<CrossPostCommenter[]>`SELECT 1`; }});\n}});\n"
        );
        std::fs::write(repo.path().join(file), source("expect(1).toBe(1);")).unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        std::fs::write(repo.path().join(file), source("expect(2).toBe(2);")).unwrap();
        git(&["commit", "--quiet", "-am", "head"]);
        let tests = changed_tests(repo.path(), "HEAD~1", "HEAD", &[file.to_string()], None).unwrap();
        assert_eq!(tests, [id(file, "auth signs out")]);

        // A broken tip is refused at its location; a broken base is not.
        std::fs::write(repo.path().join(file), source("const value = ;")).unwrap();
        git(&["commit", "--quiet", "-am", "broken"]);
        let refusal = changed_tests(repo.path(), "HEAD~1", "HEAD", &[file.to_string()], None).unwrap_err();
        let located = refusal.downcast_ref::<ParseError>().expect("a located parse refusal");
        assert_eq!((located.path.as_str(), located.line), (file, 5));
        std::fs::write(repo.path().join(file), source("expect(3).toBe(3);")).unwrap();
        git(&["commit", "--quiet", "-am", "repaired"]);
        let tests = changed_tests(repo.path(), "HEAD~1", "HEAD", &[file.to_string()], None).unwrap();
        assert_eq!(tests, [id(file, "auth signs out")]);

        // GH #2054: uncertain source parsing widens to package proof without
        // erasing identified changed tests in the neighbouring test file.
        let service = "comment-analytics.service.ts";
        std::fs::write(repo.path().join(service), "export const total = ;\n").unwrap();
        std::fs::write(repo.path().join(file), source("expect(4).toBe(4);")).unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "service"]);
        let (tests, widened) = super::super::stability::requirements(repo.path(), "HEAD", "HEAD~1", &[service.to_string(), file.to_string()], None).unwrap();
        assert_eq!(tests, [id(file, "auth signs out")]);
        assert_eq!(widened, [super::super::widened::Scope::Package(String::new())]);
    }

    /// GH #2116, #2127: a data module, a `.d.mts` declaration and a helper under
    /// `test/` or `tests/` hold no tests. The gate widened them to a `File`
    /// scope whose printed `worker-check -- <runner> <file>` run finds nothing
    /// and writes no report. They widen to their package, which runs their
    /// consumers; a real spec stays a `File` scope.
    #[test]
    fn support_files_under_test_dirs_widen_to_the_package_gh2116() {
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
        };
        git(&["init", "--quiet"]);
        std::fs::write(repo.path().join("package.json"), "{}").unwrap();
        let support = [
            "test/scenarios/conversation-flows/flows.mjs",
            "tests/config/staging-target.d.mts",
            "tests/qa/journeys/staging-guard.ts",
        ];
        let spec = "tests/scenarios/flows.scenario.spec.ts";
        for (index, path) in support.iter().chain([&spec]).enumerate() {
            let full = repo.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, format!("export const value = {index};\n")).unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        for path in support {
            std::fs::write(repo.path().join(path), "export const value = 'changed';\n").unwrap();
        }
        git(&["commit", "--quiet", "-am", "head"]);
        let paths = support.map(str::to_owned);
        let (tests, widened) = super::super::stability::requirements(repo.path(), "HEAD", "HEAD~1", &paths, None).unwrap();
        assert!(tests.is_empty());
        assert_eq!(widened, [super::super::widened::Scope::Package(String::new())]);
        assert_eq!(
            super::super::widened::scope(repo.path(), spec),
            super::super::widened::Scope::File(spec.into())
        );
    }

    /// GH #2146: hub-web's responsive-goals.ts helper holds no tests; its
    /// assertions run through pair-link.journey.ts. The gate widened it to the
    /// whole package. It now runs through the Playwright specs importing it,
    /// directly or through other helpers; a helper no spec imports, or one a
    /// unit test also imports, keeps its previous proof. hub-double.ts imports
    /// Playwright only for types, and a Vitest test imports it: its helpers
    /// keep their Vitest route (cas-87b5).
    #[test]
    fn playwright_helper_runs_through_its_importing_specs_gh2146() {
        use super::super::widened::{self, Scope};
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git").current_dir(repo.path())
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
        };
        git(&["init", "--quiet"]);
        let files = [
            (".gitignore", "node_modules/\n"),
            ("package.json", "{}"),
            ("playwright.config.ts", "export default {};"),
            ("playwright.responsive.config.ts", "export default {};"),
            ("vitest.config.ts", "export default {};"),
            ("e2e/journeys/world.ts", "export const world = 1;"),
            ("e2e/journeys/journey.ts", "import { test } from \"@playwright/test\";\nimport { world } from \"./world\";\nexport { test, world };"),
            ("e2e/journeys/responsive-goals.ts", "import { expect } from \"@playwright/test\";\nexport const goals = () => expect(1).toBe(1);"),
            ("e2e/journeys/orphan.ts", "import { expect } from \"@playwright/test\";\nexport const unused = 1;"),
            ("e2e/journeys/pair-link.journey.ts", "import { test } from \"./journey\";\nimport { goals } from \"./responsive-goals.js\";\nimport { shared } from \"../../src/shared\";\ntest(\"pairs\", goals);"),
            ("e2e/journeys/read-history.journey.ts", "import { test } from \"./journey\";\ntest(\"reads\", () => {});"),
            ("e2e/journeys/summary.journey.ts", "import { test } from \"./journey\";\nimport { goals } from \"./responsive-goals\";\ntest(\"sums\", goals);"),
            ("src/shared.ts", "export const shared = 1;"),
            ("src/shared.test.ts", "import { test } from 'vitest';\nimport { shared } from './shared';\nimport { unused } from '../e2e/journeys/orphan';\ntest('shared', () => {});"),
            ("e2e/journeys/hub-double.ts", "import type { Page } from \"@playwright/test\";\nimport { runtime } from \"./hub-double-runtime\";\nexport const double = (page: Page) => runtime;"),
            ("e2e/journeys/hub-double-runtime.ts", "export const runtime = 1;"),
            ("preview/browser-page.test.ts", "import { double } from \"../e2e/journeys/hub-double\";\nit(\"serves\", () => double);"),
        ];
        for (path, body) in files {
            std::fs::create_dir_all(repo.path().join(path).parent().unwrap()).unwrap();
            std::fs::write(repo.path().join(path), body).unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        let helper = "e2e/journeys/responsive-goals.ts";
        std::fs::write(repo.path().join(helper), "import { expect } from \"@playwright/test\";\nexport const goals = () => expect(2).toBe(2);").unwrap();
        git(&["commit", "--quiet", "-am", "head"]);
        let (tests, scopes) = super::super::stability::requirements(repo.path(), "HEAD", "HEAD~1", &[helper.into()], None).unwrap();
        assert!(tests.is_empty());
        assert_eq!(scopes, [Scope::Related(helper.into())]);
        let (pair, history, summary) = ("e2e/journeys/pair-link.journey.ts", "e2e/journeys/read-history.journey.ts", "e2e/journeys/summary.journey.ts");
        assert_eq!(playwright_importers(repo.path(), helper), [pair, summary]);
        assert_eq!(playwright_importers(repo.path(), "e2e/journeys/world.ts"), [pair, history, summary]);
        let command = widened::commands(Path::new("/cas"), repo.path(), &[helper.into()], &scopes);
        assert_eq!(command, [format!("cas factory worker-check --cas-root /cas -- playwright {helper}")]);
        assert!(widened::valid(&widened::Proof { runner: "playwright".into(), scope: scopes[0].clone(), reason: String::new(), configs: Default::default() }));

        // Park's receipt matcher (receipt_scope.rs reads it): the helper is
        // covered only when every importing spec ran under the config that
        // owns it. The stand-in Playwright lists a spec only under its owner:
        // pair-link under the responsive config, the others under the default.
        let head = String::from_utf8(Command::new("git").current_dir(repo.path()).args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap().trim().to_string();
        let scratch = tempfile::tempdir().unwrap();
        let omit = scratch.path().join("omit");
        let playwright = repo.path().join("node_modules/.bin/playwright");
        std::fs::create_dir_all(playwright.parent().unwrap()).unwrap();
        std::fs::write(&playwright, format!(r#"#!/usr/bin/env python3
import json, os, re, sys
from pathlib import Path
a = sys.argv[1:]
config = Path(a[a.index('--config') + 1]).name
owned = {{'playwright.config.ts': ['summary.journey.ts', 'read-history.journey.ts'], 'playwright.responsive.config.ts': ['pair-link.journey.ts']}}[config]
spec = os.path.relpath(re.sub(r'\\(.)', r'\1', a[-1]), os.getcwd())
listing = '--list' in a
shown = spec if Path(spec).name in owned else None
if shown and not listing and os.path.exists({omit:?}) and Path(spec).name == 'summary.journey.ts':
    shown = 'e2e/journeys/read-history.journey.ts'
# Real Playwright always emits one test entry per spec; discovery leaves its results empty.
tests = [{{'projectId': 'journeys', 'projectName': 'journeys', 'expectedStatus': 'passed', 'annotations': [], 'results': [] if listing else [{{'status': 'passed', 'retry': 0}}]}}]
suites = [{{'title': Path(shown).name, 'column': 0, 'specs': [{{'file': shown, 'title': 'case', 'tests': tests}}], 'suites': []}}] if shown else []
Path(os.environ['PLAYWRIGHT_JSON_OUTPUT_FILE']).write_text(json.dumps({{'config': {{'rootDir': os.getcwd()}}, 'errors': [], 'suites': suites}}))
raise SystemExit(0 if shown else 1)
"#, omit = omit.display().to_string())).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&playwright, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let missing = |cas: &Path| widened::missing(&super::super::PassingTests::new(cas, repo.path(), &head), &scopes);
        let run = |cas: &Path, scope: &Scope, file: &str, config: Option<&Path>|
            execute_tests(cas, repo.path(), &head, "playwright", config, &[], Some((scope, file, "fixture")), &[]);
        let responsive = repo.path().join("playwright.responsive.config.ts");
        // Specs run one by one: an omitted consumer or an unrelated spec leaves the helper uncovered.
        let by_spec = tempfile::tempdir().unwrap();
        assert_eq!(missing(by_spec.path()), scopes);
        run(by_spec.path(), &Scope::File(pair.into()), pair, Some(&responsive)).unwrap();
        assert_eq!(missing(by_spec.path()), scopes, "summary.journey.ts has not run");
        run(by_spec.path(), &Scope::File(history.into()), history, None).unwrap();
        assert_eq!(missing(by_spec.path()), scopes, "an unrelated spec is no proof");
        run(by_spec.path(), &Scope::File(summary.into()), summary, None).unwrap();
        assert!(missing(by_spec.path()).is_empty(), "every importing spec ran");
        // The helper's own run finds each owning config and records it.
        let through_helper = tempfile::tempdir().unwrap();
        run(through_helper.path(), &scopes[0], helper, None).unwrap();
        assert!(missing(through_helper.path()).is_empty());
        let passing = super::super::PassingTests::new(through_helper.path(), repo.path(), &head);
        let proof = passing.records().iter().find_map(|(_, _, test)| test.widened.clone()).unwrap();
        assert_eq!(proof.configs, BTreeMap::from([
            (pair.to_string(), "playwright.responsive.config.ts".to_string()),
            (summary.to_string(), "playwright.config.ts".to_string()),
        ]));
        // A run that executes some other spec in place of a consumer is no proof.
        std::fs::write(&omit, "").unwrap();
        let partial = tempfile::tempdir().unwrap();
        let error = run(partial.path(), &scopes[0], helper, None).unwrap_err();
        assert!(format!("{error:#}").contains("[G-61]"), "{error:#}");
        assert_eq!(missing(partial.path()), scopes);
        assert_eq!(widened::scope(repo.path(), "e2e/journeys/orphan.ts"), Scope::Package(String::new()));
        let runtime = "e2e/journeys/hub-double-runtime.ts";
        let scopes = [widened::scope(repo.path(), runtime)];
        assert_eq!(widened::commands(Path::new("/cas"), repo.path(), &[runtime.into()], &scopes),
            [format!("cas factory worker-check --cas-root /cas -- vitest related --run {runtime}")]);
        assert!(playwright_importers(repo.path(), "src/shared.ts").is_empty());
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

    /// GH #2058 (and #2066, #2117, #2122): an integration spec that only a
    /// sibling Jest config selects ran under the default `.spec.ts` config and
    /// found no tests, so no stability proof could ever be earned.
    #[test]
    fn jest_spec_runs_under_the_config_that_selects_it_gh_2058() {
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
        let backend = repo.path().join("apps/backend");
        let test = backend.join("test");
        let write = |path: &Path, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        git(&["init", "--quiet"]);
        write(&repo.path().join("package.json"), "{}");
        write(&backend.join("package.json"), "{}");
        // The default config selects `.spec.ts`; a sibling config selects the integration spec.
        write(&backend.join("jest.config.js"),
            r"module.exports = { testRegex: '.*\\.spec\\.ts$' };");
        let sibling = test.join("jest-inbox-tools.integration.config.js");
        write(&sibling, r"module.exports = { testRegex: '.*\\.integration\\.ts$' };");
        // A stand-in Jest: lists the files its config selects under its own directory and
        // runs a spec only when its config lists it ("No tests found" otherwise).
        let jest = backend.join("node_modules/.bin/jest");
        write(&jest, r#"#!/usr/bin/env node
const fs = require('fs'), path = require('path');
const a = process.argv.slice(2), at = (f) => a[a.indexOf(f) + 1];
const config = path.resolve(at('--config')), pattern = new RegExp(require(config).testRegex);
const walk = (dir) => fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
  entry.name === 'node_modules' ? [] : entry.isDirectory() ? walk(path.join(dir, entry.name)) : [path.join(dir, entry.name)]);
const listed = walk(path.dirname(config)).filter((file) => pattern.test(file));
if (a.includes('--listTests')) { console.log(JSON.stringify(listed)); process.exit(0); }
const spec = a[a.indexOf('--runTestsByPath') + 1], ok = listed.includes(spec);
fs.writeFileSync(at('--outputFile'), JSON.stringify({ success: ok, testResults: ok ? [{ name: spec, status: 'passed',
  assertionResults: [{ fullName: 'pool drains', status: 'passed' }] }] : [] }));
process.exit(ok ? 0 : 1);
"#);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&jest, std::fs::Permissions::from_mode(0o755)).unwrap();
        let spec = "apps/backend/test/inbox-analysis-pool.integration.ts";
        let source = |body: &str| format!("describe('pool', () => {{\n  it('drains', () => {{ {body} }});\n}});\n");
        write(&repo.path().join(spec), &source("throw 0;"));
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        write(&repo.path().join(spec), &source(""));
        git(&["commit", "--quiet", "-am", "head"]);
        let head = git(&["rev-parse", "HEAD"]);

        // The real path: changed-test identity, the prescribed command, five capped runs, close's receipt read.
        let tests = changed_tests(repo.path(), &base, &head, &[spec.to_string()], None).unwrap();
        assert_eq!(tests, [id(spec, "pool drains")]);
        let cas_root = tempfile::tempdir().unwrap();
        let config = "apps/backend/test/jest-inbox-tools.integration.config.js";
        let passing = || super::super::PassingTests::new(cas_root.path(), repo.path(), &head);
        assert_eq!(super::super::stability::missing(&passing(), &tests), tests);

        // Worker-check discovers the owning config and counts the spec's tests five times.
        let proof = execute_tests(cas_root.path(), repo.path(), &head, "jest", None, &tests, None, &[]).unwrap();
        assert_eq!((proof.consecutive_passes, &proof.tests), (5, &tests));
        // The prescribed rerun names that config, and running it as printed passes the same way.
        let prescribed = super::super::stability::rerun_commands(cas_root.path(), repo.path(), &tests);
        assert_eq!(prescribed.len(), 1);
        assert!(prescribed[0].contains(&format!("-- jest --config {config} {spec}`")), "{prescribed:?}");
        let args: Vec<String> = ["jest", "--config", config, spec].map(String::from).into();
        let invocation = invocation(&args).expect("the prescribed command is a capped runner");
        let explicit = repo.path().join(invocation.config.as_deref().unwrap());
        execute_tests(cas_root.path(), repo.path(), &head, "jest", Some(&explicit), &tests, None, &[]).unwrap();
        assert!(super::super::stability::missing(&passing(), &tests).is_empty());
        assert_eq!(passing().receipts().len(), 1);
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
        // GH #2055 (cas-6aa9): a node:test file in a Jest package uses Node's runner.
        std::fs::write(
            repo.path().join("api/app-boot.test.cjs"),
            "const { test } = require('node:test');\ntest('boots', () => {});\n",
        )
        .unwrap();
        assert_eq!(runner_hint(repo.path(), "api/app-boot.test.cjs"), Some("node --test"));
        assert_eq!(invocation(&["node".into(), "--test".into(), "api/app-boot.test.cjs".into()]).map(|invocation| invocation.runner), Some("node"));
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
            "test('case_id', callback);",
            "describe('suite_id', declareTests);",
        ] {
            assert!(
                source_tests("a.test.ts", source, &[(1, 1)]).is_err(),
                "{source}"
            );
        }
        // GH #1977/#1978: an `.each` table registers one runtime test per row.
        let rows = "describe.each(['x'])('suite %s', () => {\n  it.each([1, 2])('row %i', () => {});\n});";
        let template = Identity { parameterized: true, ..id("a.test.ts", "suite %s row %i") };
        assert_eq!(source_tests("a.test.ts", rows, &[(2, 2)]).unwrap(), [template.clone()]);
        let runtime = [id("a.test.ts", "suite x row 2"), id("a.test.ts", "suite x other 2")];
        assert_eq!(runtime.map(|row| template.selects(&row)), [true, false]);
        // GH #2093: escaped quotes in a static literal are still a static title.
        let escaped = "it('reads each post\\'s \\\"depth\\\" from its creator\\'s capture', () => {});";
        assert_eq!(
            source_tests("a.test.ts", escaped, &[(1, 1)]).unwrap(),
            [id("a.test.ts", "reads each post's \"depth\" from its creator's capture")]
        );
        let unicode = "it('a\\u0041', () => {});";
        assert!(source_tests("a.test.ts", unicode, &[(1, 1)]).is_err());
        // GH #2112: a parameterized describe around a static test title selects
        // that test under every runtime suite name; the test title stays literal.
        let looped = "for (const [mode, api] of rows) test.describe(api, () => {\n  test('p100%_ok', () => {});\n});\ntest.describe(`ssr_${mode}_run`, () => {\n  test('defers_ok', () => {});\n});";
        let wildcard = |name: &str| Identity { parameterized: true, ..id("a.test.ts", name) };
        let (dynamic, interpolated) = (wildcard("%s p100%_ok"), wildcard("ssr_%s_run defers_ok"));
        assert_eq!(source_tests("a.test.ts", looped, &[(2, 2)]).unwrap(), [dynamic.clone()]);
        assert_eq!(source_tests("a.test.ts", looped, &[(5, 5)]).unwrap(), [interpolated]);
        assert_eq!(
            [("a.test.ts", "api_hangs p100%_ok"), ("a.test.ts", "p100%_ok_x")]
                .map(|(path, name)| dynamic.selects(&id(path, name))),
            [true, false]
        );
        // GH #1973: a TypeScript generic over an import type parses.
        let generic = "vi.mock('vue-i18n', async importOriginal => ({\n  ...(await importOriginal<typeof import('vue-i18n')>()),\n}));\nit('a', () => { expect(1).toBe(1); });";
        assert_eq!(source_tests("a.test.ts", generic, &[(1, 4)]).unwrap(), [id("a.test.ts", "a")]);
        assert_eq!(
            changed_lines("@@ -7 +7,0 @@\n-x\n@@ -10 +9,2 @@\n+y\n+z").unwrap(),
            [(8, 7), (9, 10)]
        );
        assert!(!allowed_args(&["vitest".into(), "--retry=3".into()]));
        assert!(!allowed_args(&["jest".into(), "../a.test.js".into()]));
        // GH #1985, #1992, #1973, #1978, #1998: the shapes worker discipline and
        // package scripts use, plus a package-local jest config.
        let parse = |command: &str| {
            invocation(&command.split(' ').map(String::from).collect::<Vec<_>>())
        };
        let file = || vec!["src/a.spec.ts".to_string()];
        for (command, runner, config) in [
            ("npx vitest run src/a.spec.ts", "vitest", None),
            ("pnpm exec vitest run src/a.spec.ts", "vitest", None),
            ("npx jest src/a.spec.ts --runInBand", "jest", None),
            ("npx jest -t name src/a.spec.ts", "jest", None),
            ("pnpm exec jest --config jest.conversations.config.js --runInBand src/a.spec.ts", "jest", Some("jest.conversations.config.js")),
            ("yarn playwright test -c e2e/playwright.config.ts src/a.spec.ts", "playwright", Some("e2e/playwright.config.ts")),
        ] {
            assert_eq!(
                parse(command),
                Some(Invocation { runner, config: config.map(String::from), files: file() }),
                "{command}"
            );
        }
        for command in ["vitest run --retry 2 src/a.spec.ts", "jest --config ../x.js src/a.spec.ts"] {
            assert_eq!(parse(command), None, "{command}");
        }
        // Exact command shapes supplied in the G-61 reports. Caller filters
        // cannot narrow the capped run's own source-derived selection.
        for (issue, command) in [
            (2058, "jest apps/backend/test/inbox-analysis-pool.integration.ts --config apps/backend/test/jest-inbox-tools.integration.config.js"),
            (2059, "jest apps/backend/test/workspace-reports.integration.ts --config test/jest-workspace-reports.integration.config.js"),
            (2064, "jest apps/backend/test/inbox-tools.integration.ts --config apps/backend/test/jest-inbox-tools.integration.config.js --runInBand --testNamePattern=positive-filter"),
            (2064, "jest apps/backend/test/inbox-analysis-pool.integration.ts --config apps/backend/test/jest-inbox-tools.integration.config.js -t positive-filter"),
            (2066, "pnpm exec jest apps/backend/test/sales-opportunity.integration.ts --config test/jest-sales-opportunity.integration.config.js --runInBand"),
        ] {
            assert!(parse(command).is_some(), "GH #{issue}: {command}");
        }
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
            changed_tests(repo.path(), &base, &head, &["a.test.ts".into()], None).unwrap(),
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
                &["a.test.ts".into()],
                None
            )
            .is_err()
        );
        let cas_root = tempfile::tempdir().unwrap();
        let error = execute(
            cas_root.path(),
            &["vitest".into(), "a.test.ts".into()],
            repo.path(),
            &[],
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
            changed_tests(repo.path(), &base, &head, &[path.into()], None).unwrap(),
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
                    &[path.into()],
                    None
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

    /// GH #1977, #2000, #1971: park asks only for tests the task's own
    /// commits changed in the delivered tree.
    #[test]
    fn park_requires_only_tests_the_delivery_changed_at_its_tip() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(repo.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output);
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        let spec = "a.spec.ts";
        let commit = |source: &str, message: &str| {
            std::fs::write(repo.path().join(spec), source).unwrap();
            git(&["add", spec]);
            git(&["commit", "--quiet", "-m", message]);
            git(&["rev-parse", "HEAD"])
        };
        let file = |table: &str, own: &str, foreign: &str| format!(
            "it.each([{table}])('row %s', () => {{}});\nit('own', () => {{ {own} }});\nit('foreign', () => {{ {foreign} }});\n"
        );
        git(&["init", "--quiet", "-b", "main"]);
        let base = commit(&file("1", "", ""), "base");
        git(&["checkout", "--quiet", "-b", "lane"]);
        // #1977: an edited table row, then restored, is not in the net delivery.
        let edited = commit(&file("1, 2", "", ""), "cas-1 edit row");
        // #1971: another author's commit on the lane changes its own test.
        let foreign = commit(&file("1, 2", "", "foreign();"), "foreign PR work");
        // #2000: the target edits its table; the lane merges the target.
        git(&["checkout", "--quiet", "main"]);
        commit(&file("3", "", ""), "target edits its table");
        git(&["checkout", "--quiet", "lane"]);
        // The table conflicts; the merge resolves to the target's row.
        let _ = Command::new("git")
            .current_dir(repo.path())
            .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid"])
            .args(["merge", "--quiet", "--no-ff", "--no-commit", "main"])
            .output()
            .unwrap();
        std::fs::write(repo.path().join(spec), file("3", "", "foreign();")).unwrap();
        git(&["add", spec]);
        git(&["commit", "--quiet", "-m", "merge main"]);
        let merge = git(&["rev-parse", "HEAD"]);
        let head = commit(&file("3", "own();", "foreign();"), "cas-1 own test");
        let ranges = [(base, edited), (merge, head.clone())];
        let required = crate::factory_worker_check::stability::required(
            repo.path(), &head, "main", &[spec.into()], Some(&ranges),
        )
        .unwrap();
        assert_eq!(required, [id(spec, "own")], "foreign commit {foreign}");
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
            // GH #1978: a template is proved when every row it selects passes.
            let template = [Identity { parameterized: true, ..id("a.test.js", "row %s") }];
            let rows = json!({"success":true,"testResults":[{"name":"/project/a.test.js","status":"passed", "assertionResults":[
                {"fullName":"row a","status":"passed"}, {"fullName":"row b","status":"passed"}]}]});
            validate_report(runner, repo, &rows, &template, 1).unwrap();
            let mut failed_row = rows.clone();
            failed_row["testResults"][0]["assertionResults"][1]["status"] = json!("failed");
            assert!(validate_report(runner, repo, &failed_row, &template, 1).is_err());
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
    fn deletion_beside_legacy_callbacks_only_classifies_surviving_edits_cas_31eb() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git").current_dir(repo.path())
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        let path = "whole-picture.journey.ts";
        let kept = "for (const width of [390, 1280]) {\n  test.describe(`surface ${width}`, () => {\n    const callback = async () => {\n      work();\n    };\n    test(`kept ${width}`, callback);\n  });\n}\n";
        std::fs::write(repo.path().join(path), format!("test('removed', () => {{}});\n\n{kept}")).unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.path().join(path), kept).unwrap();
        git(&["commit", "-qam", "delete unrelated inline test"]);
        let deleted = git(&["rev-parse", "HEAD"]);
        assert!(changed_tests(repo.path(), &base, &deleted, &[path.into()], None).unwrap().is_empty());
        std::fs::write(repo.path().join(path), kept.replace("work();", "changed();")).unwrap();
        git(&["commit", "-qam", "edit callback"]);
        assert!(changed_tests(repo.path(), &deleted, "HEAD", &[path.into()], None).is_err());
    }

    #[test]
    fn multiline_type_import_keeps_changed_test_locations_cas_31eb() {
        let source = "vi.mock('m', async load => ({ ...(await load<typeof import(\n  'm'\n)>()) }));\ntest('before', () => {});\ntest('after', () => { work(); });\n";
        assert_eq!(source_tests("a.test.ts", source, &[(5, 5)]).unwrap(), [id("a.test.ts", "after")]);
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

    /// GH #2172/cas-7b09: producer exit must leave readable traces or assertion
    /// reports, without retaining credentials from a JSON failure payload.
    #[cfg(unix)]
    #[test]
    fn failed_javascript_reports_outlive_worker_check_cas_7b09() {
        use std::os::unix::fs::PermissionsExt;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        for runner in ["playwright", "vitest"] {
            let repo = tempfile::tempdir().unwrap();
            let git = |args: &[&str]| {
                let output = Command::new("git").current_dir(repo.path())
                    .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                    .args(args).output().unwrap();
                assert!(output.status.success(), "{output:?}");
                String::from_utf8(output.stdout).unwrap().trim().to_string()
            };
            git(&["init", "--quiet"]);
            std::fs::write(repo.path().join(".gitignore"), "node_modules/\nsource-report.txt\n").unwrap();
            std::fs::write(repo.path().join("package.json"), "{}").unwrap();
            let file = "e2e/retry.spec.ts";
            std::fs::create_dir_all(repo.path().join("e2e")).unwrap();
            std::fs::write(repo.path().join(file), "test('fails', async () => { expect(1).toBe(2); });\n").unwrap();
            // A deterministic failure that writes a trace, as Playwright does.
            let bin = repo.path().join("node_modules/.bin");
            std::fs::create_dir_all(&bin).unwrap();
            let secret = "fixture-credential-7b09";
            let payload = json!({"success":false,"testResults":[{
                "name":repo.path().join(file),"status":"failed","assertionResults":[{
                    "fullName":"fails","status":"failed",
                    "failureMessages":["AssertionError: expected 1 to be 2\n at retry.spec.ts:1:32"],
                    "apiKey":secret
                }]
            }]});
            std::fs::write(repo.path().join("failure.json"), payload.to_string()).unwrap();
            let script = if runner == "playwright" {
                "#!/bin/sh\nout=; prev=\nfor arg in \"$@\"; do [ \"$prev\" = --output ] && out=$arg; prev=$arg; done\n\
                mkdir -p \"$out/retry-fails-chromium\" && printf trace > \"$out/retry-fails-chromium/trace.zip\"\n\
                printf '{\"errors\":[],\"suites\":[]}' > \"$PLAYWRIGHT_JSON_OUTPUT_FILE\"\nexit 1\n"
            } else {
                "#!/bin/sh\nreport=; prev=\nfor arg in \"$@\"; do [ \"$prev\" = --outputFile ] && report=$arg; prev=$arg; done\n\
                cp failure.json \"$report\"\nprintf '%s\\n' \"$report\" > source-report.txt\nexit 1\n"
            };
            std::fs::write(bin.join(runner), script).unwrap();
            std::fs::set_permissions(bin.join(runner), std::fs::Permissions::from_mode(0o755)).unwrap();
            git(&["add", "."]);
            git(&["commit", "--quiet", "-m", "failing spec"]);
            let head = git(&["rev-parse", "HEAD"]);
            let cas_root = tempfile::tempdir().unwrap();
            let artifacts = tempfile::tempdir().unwrap();
            std::fs::write(cas_root.path().join("config.toml"), format!("[factory]\nartifacts_root = {:?}\n", artifacts.path().display().to_string())).unwrap();

            let error = execute_tests(cas_root.path(), repo.path(), &head, runner, None, &[id(file, "fails")], None, &[])
                .unwrap_err();
            let message = format!("{error:#}");
            // One durable copy under the artifacts root, named by the failure.
            let kept: Vec<PathBuf> = walkdir::WalkDir::new(artifacts.path()).into_iter().filter_map(Result::ok)
                .filter(|entry| entry.file_name() == "report-0.json").map(|entry| entry.path().parent().unwrap().to_path_buf())
                .collect();
            assert_eq!(kept.len(), 1, "{message}");
            assert!(message.contains(&kept[0].display().to_string()), "{message}");
            if runner == "playwright" {
                assert_eq!(std::fs::read(kept[0].join("playwright-output/retry-fails-chromium/trace.zip")).unwrap(), b"trace");
            } else {
                let temporary = std::fs::read_to_string(repo.path().join("source-report.txt")).unwrap();
                assert!(!Path::new(temporary.trim()).exists());
                let retained = std::fs::read_to_string(kept[0].join("report-0.json")).unwrap();
                let report: Value = serde_json::from_str(&retained).unwrap();
                // Preserve the runner-supplied diagnostic protocol payload exactly.
                assert_eq!(report["testResults"][0]["assertionResults"][0]["failureMessages"],
                    payload["testResults"][0]["assertionResults"][0]["failureMessages"]);
                assert!(!retained.contains(secret));
            }
        }
    }

    #[test]
    fn park_discovery_uses_delivered_identities_for_historical_ranges() {
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
        let path = "hub-web/e2e/journeys/fleet-ops.journey.ts";
        let inline = r#"import { test, expect } from '@playwright/test';
for (const width of [390, 1280]) {
  test.describe(`fleet failure ${width}`, () => {
    test(`case ${width}`, async () => { expect(1).toBe(1); });
  });
}
"#;
        git(&["init", "--quiet", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join("hub-web/e2e/journeys")).unwrap();
        std::fs::write(repo.path().join(path), inline).unwrap();
        git(&["add", path]);
        git(&["commit", "--quiet", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        let historical = inline.replace("expect(1).toBe(1)", "expect(1).toBe(2)");
        std::fs::write(repo.path().join(path), historical).unwrap();
        git(&["commit", "--quiet", "-am", "changed inline test"]);
        let historical_head = git(&["rev-parse", "HEAD"]);
        let shared = r#"import { test, expect } from '@playwright/test';
for (const width of [390, 1280]) {
  test.describe(`fleet failure ${width}`, () => {
    const run = async () => { expect(1).toBe(2); };
    // cas-stability-identities: ["fleet failure 390 case 390", "fleet failure 1280 case 1280"]
    test(`case ${width}`, run);
  });
}
"#;
        std::fs::write(repo.path().join(path), shared).unwrap();
        git(&["commit", "--quiet", "-am", "share callback and declare identities"]);
        let head = git(&["rev-parse", "HEAD"]);
        let paths = [path.to_string()];
        // A real park records the delivery range up to the parked tip; a
        // range that stops short of head is not a state park produces. The
        // historical commit stays inside the range, so its edit is owned.
        let _ = historical_head;
        let ranges = [(base.clone(), head.clone())];
        let required = crate::factory_worker_check::stability::required(
            repo.path(), &head, "main", &paths, Some(&ranges),
        )
        .unwrap();
        assert_eq!(
            required,
            [
                id(path, "fleet failure 1280 case 1280"),
                id(path, "fleet failure 390 case 390"),
            ]
        );

        let missing_identity = shared.replace(
            "    // cas-stability-identities: [\"fleet failure 390 case 390\", \"fleet failure 1280 case 1280\"]\n",
            "",
        );
        std::fs::write(repo.path().join(path), missing_identity).unwrap();
        git(&["commit", "--quiet", "-am", "remove declared identities"]);
        let missing_head = git(&["rev-parse", "HEAD"]);
        assert!(
            crate::factory_worker_check::stability::required(
                repo.path(), &missing_head, "main", &paths, Some(&[(base, missing_head.clone())]),
            )
            .is_err(),
            "park must continue to refuse shared callbacks without declared identities"
        );
    }
}
