//! Resolve changed Rust tests through the delivered Cargo/module tree.
use super::*;
use tree_sitter::{Node, Parser};

#[derive(Clone)]
pub(crate) struct Test {
    pub identity: Identity,
    pub package: String,
    pub harness: Option<String>,
    source: String,
    parameterized: bool,
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn attributes<'a>(node: Node<'_>, source: &'a str) -> Vec<(usize, &'a str)> {
    let mut attributes = Vec::new();
    let mut previous = node.prev_named_sibling();
    while let Some(node) = previous {
        match node.kind() {
            "attribute_item" => attributes.push((node.start_byte(), text(node, source))),
            "line_comment" | "block_comment" => {},
            _ => break,
        }
        previous = node.prev_named_sibling();
    }
    attributes
}

fn join(directory: &str, path: &str) -> Result<String> {
    normalize_repo_path(directory, path).context("STABILITY_SOURCE_PATH: module escapes repository")
}

struct Inventory<'a> {
    sources: BTreeMap<&'a str, &'a str>,
    parser: Parser,
    tests: Vec<Test>,
    visited: BTreeSet<(String, String, String, Option<String>)>,
}

impl Inventory<'_> {
    fn file(&mut self, path: &str, prefix: &str, directory: &str, package: &str, harness: Option<&str>) -> Result<()> {
        if !self.visited.insert((path.into(), prefix.into(), package.into(), harness.map(str::to_owned))) {
            return Ok(());
        }
        let Some(source) = self.sources.get(path).copied() else { return Ok(()); };
        let tree = self.parser.parse(source, None).context("STABILITY_RUST_PARSE")?;
        // Recovery inside valid Rust bodies (for example a local `&raw`
        // borrow) must not reject unrelated tests. Discover function/module
        // structure here; the capped Cargo run checks Rust validity.
        self.items(tree.root_node(), source, path, prefix, directory, package, harness)
    }

    #[allow(clippy::too_many_arguments)]
    fn items(&mut self, node: Node<'_>, source: &str, path: &str, prefix: &str,
        directory: &str, package: &str, harness: Option<&str>) -> Result<()> {
        let mut cursor = node.walk();
        for node in node.named_children(&mut cursor) {
            let attrs = attributes(node, source);
            match node.kind() {
                "function_item" => {
                    let attribute = regex::Regex::new(r"^#\[\s*(?:\w+::)*(test|rstest|test_case|test_matrix)\b")?;
                    if !attrs.iter().any(|(_, attr)| attribute.is_match(attr)) { continue; }
                    let name = node.child_by_field_name("name").context("test name missing")?;
                    let name = text(name, source);
                    let name = if prefix.is_empty() { name.into() } else { format!("{prefix}::{name}") };
                    let begin = attrs.iter().map(|(start, _)| *start).min().unwrap_or(node.start_byte());
                    self.tests.push(Test {
                        identity: Identity { path: path.into(), name }, package: package.into(),
                        harness: harness.map(str::to_owned), source: source[begin..node.end_byte()].into(),
                        parameterized: attrs.iter().any(|(_, attr)| !attr.contains("test]") && attribute.is_match(attr)
                            && !attr.contains("::test")),
                    });
                }
                "mod_item" => {
                    let name = text(node.child_by_field_name("name").context("module name missing")?, source);
                    let child_prefix = if prefix.is_empty() { name.into() } else { format!("{prefix}::{name}") };
                    if let Some(body) = node.child_by_field_name("body") {
                        self.items(body, source, path, &child_prefix, &join(directory, name)?, package, harness)?;
                    } else {
                        let explicit = regex::Regex::new(r#"^#\[\s*path\s*=\s*("(?:[^"\\]|\\.)*")\s*\]$"#)?;
                        let explicit = attrs.iter().find_map(|(_, attr)| explicit.captures(attr))
                            .map(|capture| serde_json::from_str::<String>(&capture[1])).transpose()?;
                        let child = if let Some(explicit) = explicit {
                            join(path.rsplit_once('/').map_or("", |(parent, _)| parent), &explicit)?
                        } else {
                            let sibling = join(directory, &format!("{name}.rs"))?;
                            if self.sources.contains_key(sibling.as_str()) { sibling }
                            else { join(directory, &format!("{name}/mod.rs"))? }
                        };
                        let child_directory: String = if child.ends_with("/mod.rs") { child.trim_end_matches("/mod.rs").into() }
                            else { child.trim_end_matches(".rs").into() };
                        self.file(&child, &child_prefix, &child_directory, package, harness)?;
                    }
                }
                "macro_invocation" => {
                    let mut children = node.walk();
                    let body = node.named_children(&mut children).find(|child| child.kind() == "token_tree");
                    if let Some(body) = body {
                        let body = text(body, source);
                        if body.len() > 2 {
                            let inner = &body[1..body.len() - 1];
                            // Property-test macros retain static fn names even
                            // when their parameter grammar is not ordinary Rust.
                            let parsed = self.parser.parse(inner, None).context("STABILITY_MACRO_PARSE")?;
                            self.items(parsed.root_node(), inner, path, prefix, directory, package, harness)?;
                        }
                    }
                }
                "ERROR" => self.items(node, source, path, prefix, directory, package, harness)?,
                _ => {},
            }
        }
        Ok(())
    }
}

fn inventory(repo: &Path, head: &str, paths: &[String]) -> Result<Vec<Test>> {
    let tree = delivered_tree(repo, head).context("STABILITY_DELIVERY_TREE")?;
    let sources = tree.rust_sources(repo).context("STABILITY_SOURCE_READ")?;
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
    let mut inventory = Inventory { parser, sources: sources.iter().map(|(path, body)| (path.as_str(), body.as_str())).collect(),
        tests: Vec::new(), visited: BTreeSet::new() };
    let manifests = paths.iter().filter(|path| path.ends_with(".rs"))
        .filter_map(|path| tree.manifest_dir_for_source(path)).collect::<BTreeSet<_>>();
    for directory in manifests {
        let manifest_path = if directory.is_empty() { "Cargo.toml".into() } else { format!("{directory}/Cargo.toml") };
        let manifest: toml::Value = toml::from_str(&git(repo, &["show", &format!("{head}:{manifest_path}")])?)?;
        let Some(package) = manifest.get("package").and_then(|package| package.get("name")).and_then(toml::Value::as_str) else { continue; };
        let library = manifest.get("lib").and_then(|lib| lib.get("path")).and_then(toml::Value::as_str).unwrap_or("src/lib.rs");
        let library = join(&directory, library)?;
        let parent = library.rsplit_once('/').map_or("", |(parent, _)| parent);
        inventory.file(&library, "", parent, package, None)?;
        let mut harnesses = BTreeMap::new();
        for test in manifest.get("test").and_then(toml::Value::as_array).into_iter().flatten() {
            if let Some(name) = test.get("name").and_then(toml::Value::as_str) {
                let default = format!("tests/{name}.rs");
                harnesses.insert(name.to_owned(), join(&directory, test.get("path").and_then(toml::Value::as_str).unwrap_or(&default))?);
            }
        }
        if manifest.get("package").and_then(|package| package.get("autotests")).and_then(toml::Value::as_bool) != Some(false) {
            let test_directory = join(&directory, "tests")?;
            for path in inventory.sources.keys() {
                if let Some(relative) = path.strip_prefix(&format!("{test_directory}/")) {
                    if let Some(stem) = relative.strip_suffix(".rs").filter(|stem| !stem.contains('/')) {
                        harnesses.entry(stem.into()).or_insert_with(|| (*path).into());
                    } else if let Some(stem) = relative.strip_suffix("/main.rs").filter(|stem| !stem.contains('/')) {
                        harnesses.entry(stem.into()).or_insert_with(|| (*path).into());
                    }
                }
            }
        }
        for (name, path) in harnesses {
            let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
            inventory.file(&path, "", parent, package, Some(&name))?;
        }
    }
    Ok(inventory.tests)
}

pub(crate) fn changed(repo: &Path, base: &str, head: &str, paths: &[String]) -> Result<Vec<Test>> {
    let prior = inventory(repo, base, paths)?.into_iter()
        .map(|test| ((test.identity, test.package, test.harness), test.source)).collect::<BTreeMap<_, _>>();
    let current = inventory(repo, head, paths)?;
    // A new test in an unreferenced source file must not silently disappear
    // from the gate. It either resolves through a Cargo root or needs repair.
    let tree = delivered_tree(repo, head).context("STABILITY_DELIVERY_TREE")?;
    let sources = tree.rust_sources(repo).context("STABILITY_SOURCE_READ")?;
    for path in paths.iter().filter(|path| path.ends_with(".rs")) {
        if tree.manifest_dir_for_source(path).is_none() { continue; }
        let Some((_, source)) = sources.iter().find(|(source, _)| source == path) else { continue; };
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
        let parsed = parser.parse(source, None).context("STABILITY_RUST_PARSE")?;
        let mut standalone = Inventory { parser, sources: BTreeMap::new(), tests: Vec::new(), visited: BTreeSet::new() };
        standalone.items(parsed.root_node(), source, path, "", "", "", None)?;
        let source_tests = std::mem::take(&mut standalone.tests);
        let old = git(repo, &["show", &format!("{base}:{path}")]).unwrap_or_default();
        let parsed = standalone.parser.parse(&old, None).context("STABILITY_RUST_PARSE")?;
        standalone.items(parsed.root_node(), &old, path, "", "", "", None)?;
        let previous = standalone.tests.into_iter().map(|test| (test.identity.name, test.source)).collect::<BTreeMap<_, _>>();
        for test in source_tests {
            if previous.get(&test.identity.name) != Some(&test.source) && !current.iter().any(|resolved| resolved.identity.path == *path
                && (resolved.identity.name == test.identity.name
                    || resolved.identity.name.ends_with(&format!("::{}", test.identity.name))))
            {
                bail!("STABILITY_TEST_NOT_DISCOVERABLE: {}::{}; wire the test into a Cargo target before park", path, test.identity.name);
            }
        }
    }
    Ok(current.into_iter().filter(|test| paths.contains(&test.identity.path)
        && prior.get(&(test.identity.clone(), test.package.clone(), test.harness.clone())) != Some(&test.source)).collect())
}

pub(crate) fn live(repo: &Path, head: &str, paths: &[String]) -> Result<BTreeSet<Identity>> {
    Ok(inventory(repo, head, paths)?.into_iter().map(|test| test.identity).collect())
}

fn filter_matches(filter: &str, name: &str) -> bool {
    filter.split('|').any(|union| union.split('&').all(|intersection| {
        let value = intersection.trim().trim_start_matches("test(").trim_end_matches(')');
        value.strip_prefix('=').map_or_else(|| name.contains(value), |exact| name == exact)
    }))
}

pub(crate) fn selected(repo: &Path, base: &str, head: &str, test: &TargetedTest) -> Result<Vec<Test>> {
    let paths = git_paths(repo, &["diff", "--name-only", "--no-renames", "-z", base, head, "--"])
        .context("STABILITY_DIFF_READ")?;
    let (package, _) = package_selector(&test.package).context("test package")?;
    Ok(changed(repo, base, head, &paths)?.into_iter().filter(|own| own.package == package
        && own.harness == test.harness && filter_matches(&test.filter, &own.identity.name)).collect())
}

pub(crate) fn repeat_filter(tests: &[Test]) -> String {
    tests.iter().map(|test| if test.parameterized {
        format!("test(/^{}(?:::|$)/)", regex::escape(&test.identity.name))
    }
        else { format!("test(={})", test.identity.name) }).collect::<Vec<_>>().join(" | ")
}

/// All five attempts stay in the original capped process and builder slot.
/// The first selection keeps existing coverage; only owned tests repeat.
pub(crate) fn wrap(command: Command, test: &TargetedTest, tests: &[Test], count: &Path) -> Command {
    let mut args = vec!["nextest".to_owned(), "run".into(), "-p".into(), test.package.clone()];
    if let Some(harness) = &test.harness { args.extend(["--test".into(), harness.clone()]); }
    else { args.push("--lib".into()); }
    args.extend(["--retries".into(), "0".into(), "-E".into(), repeat_filter(tests)]);
    let mut wrapper = Command::new("bash");
    wrapper.args(["-c", r#"
verified=$1
count_file=$2
extra_count=$3
shift 3
extra=("${@:1:extra_count}")
shift "$extra_count"
"$@" || exit $?
cp "$count_file" "$count_file.initial" || exit $?
trap 'rm -f "$count_file.initial"' EXIT
original_log=${VERIFIED_TEST_LOG:-}
for attempt in 2 3 4 5; do
    if [ -n "$original_log" ]; then export VERIFIED_TEST_LOG="$original_log.repeat-$attempt"; fi
    bash -c "$verified" worker-repeat "${extra[@]}" || exit $?
done
mv "$count_file.initial" "$count_file" || exit $?
"#, "worker-stability"])
        .arg(include_str!("../../../../scripts/run-verified-tests.sh"))
        .arg(count).arg(args.len().to_string()).args(args)
        .arg(command.get_program()).args(command.get_args());
    if let Some(directory) = command.get_current_dir() { wrapper.current_dir(directory); }
    for (key, value) in command.get_envs() {
        if let Some(value) = value { wrapper.env(key, value); } else { wrapper.env_remove(key); }
    }
    wrapper
}
