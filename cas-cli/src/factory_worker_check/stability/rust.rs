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
    /// GH #2320: the operating systems its `cfg(target_os)` gates (on the
    /// function, its modules or its file) let it build on; `None` is any OS.
    pub platforms: Option<BTreeSet<String>>,
}

#[derive(Debug, thiserror::Error)]
#[error("STABILITY_TEST_NOT_DISCOVERABLE: {0}::{1}; wire the test into a Cargo target before park")]
pub(crate) struct MissingTest(String, String);

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

/// The operating systems a `#[cfg(..)]` or `#![cfg(..)]` attribute allows, when
/// it decides `target_os`; `None` when it does not (or cannot be read), so an
/// unrecognised gate never excuses a test from its host.
fn cfg_platforms(attribute: &str) -> Option<BTreeSet<String>> {
    let compact = attribute.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    let predicate = compact.strip_prefix("#[cfg(").or_else(|| compact.strip_prefix("#![cfg("))?.strip_suffix(")]")?;
    predicate_platforms(predicate)
}

fn predicate_platforms(predicate: &str) -> Option<BTreeSet<String>> {
    if let Some(os) = predicate.strip_prefix("target_os=") {
        return Some(BTreeSet::from([os.strip_prefix('"')?.strip_suffix('"')?.to_owned()]));
    }
    let (operator, inner) = predicate.split_once('(')?;
    let inner = inner.strip_suffix(')')?;
    let mut parts = Vec::new();
    let (mut depth, mut quoted, mut start) = (0usize, false, 0);
    for (index, ch) in inner.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth = depth.checked_sub(1)?,
            ',' if !quoted && depth == 0 => {
                parts.push(&inner[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    let parts = parts.into_iter().filter(|part| !part.is_empty()).map(predicate_platforms);
    match operator {
        "any" => parts.collect::<Option<Vec<_>>>().map(|sets| sets.into_iter().flatten().collect()),
        "all" => parts.flatten().reduce(|left, right| left.intersection(&right).cloned().collect()),
        _ => None,
    }
}

fn narrow(platforms: Option<&BTreeSet<String>>, attributes: impl IntoIterator<Item = Option<BTreeSet<String>>>) -> Option<BTreeSet<String>> {
    attributes.into_iter().flatten().fold(platforms.cloned(), |held, gate| Some(match held {
        Some(held) => held.intersection(&gate).cloned().collect(),
        None => gate,
    }))
}

fn join(directory: &str, path: &str) -> Result<String> {
    normalize_repo_path(directory, path).context("STABILITY_SOURCE_PATH: module escapes repository")
}

struct Inventory<'a> {
    sources: BTreeMap<&'a str, &'a str>,
    parser: Parser,
    tests: Vec<Test>,
    errors: Vec<(String, String)>,
    visited: BTreeSet<(String, String, String, Option<String>)>,
    /// The `target_os` gate of the module being walked.
    platforms: Option<BTreeSet<String>>,
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
        let outer = self.platforms.clone();
        let mut cursor = node.walk();
        let inner = node.named_children(&mut cursor).filter(|child| child.kind() == "inner_attribute_item")
            .map(|child| cfg_platforms(text(child, source))).collect::<Vec<_>>();
        self.platforms = narrow(outer.as_ref(), inner);
        let result = self.walk(node, source, path, prefix, directory, package, harness);
        self.platforms = outer;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(&mut self, node: Node<'_>, source: &str, path: &str, prefix: &str,
        directory: &str, package: &str, harness: Option<&str>) -> Result<()> {
        let mut cursor = node.walk();
        for node in node.named_children(&mut cursor) {
            let attrs = attributes(node, source);
            let result = (|| -> Result<()> {
            match node.kind() {
                "function_item" => {
                    static ATTRIBUTE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(||
                        regex::Regex::new(r"^#\[\s*(?:\w+::)*(test|rstest|test_case|test_matrix)\b").expect("test attribute regex"));
                    let attribute = &*ATTRIBUTE;
                    if !attrs.iter().any(|(_, attr)| attribute.is_match(attr)) { return Ok(()); }
                    let name = node.child_by_field_name("name").context("test name missing")?;
                    let name = text(name, source);
                    let name = if prefix.is_empty() { name.into() } else { format!("{prefix}::{name}") };
                    let begin = attrs.iter().map(|(start, _)| *start).min().unwrap_or(node.start_byte());
                    let platforms = narrow(self.platforms.as_ref(), attrs.iter().map(|(_, attr)| cfg_platforms(attr)));
                    self.tests.push(Test {
                        platforms,
                        identity: Identity::exact(path, name), package: package.into(),
                        harness: harness.map(str::to_owned), source: source[begin..node.end_byte()].into(),
                        parameterized: attrs.iter().any(|(_, attr)| attribute.captures(attr)
                            .is_some_and(|kind| &kind[1] != "test")),
                    });
                }
                "mod_item" => {
                    let name = text(node.child_by_field_name("name").context("module name missing")?, source);
                    let child_prefix = if prefix.is_empty() { name.into() } else { format!("{prefix}::{name}") };
                    let outer = self.platforms.clone();
                    self.platforms = narrow(outer.as_ref(), attrs.iter().map(|(_, attr)| cfg_platforms(attr)));
                    let walked = (|| -> Result<()> {
                    if let Some(body) = node.child_by_field_name("body") {
                        self.items(body, source, path, &child_prefix, &join(directory, name)?, package, harness)?;
                    } else {
                        static EXPLICIT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(||
                            regex::Regex::new(r#"^#\[\s*path\s*=\s*("(?:[^"\\]|\\.)*")\s*\]$"#).expect("module path regex"));
                        let explicit = &*EXPLICIT;
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
                    Ok(())
                    })();
                    self.platforms = outer;
                    walked?;
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
            Ok(())
            })();
            if let Err(error) = result {
                // Keep independent module inputs discoverable. The caller
                // widens only an input whose own structure is uncertain.
                self.errors.push((path.to_string(), format!("{error:#}")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static CLASSIFICATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    static INVENTORY_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn inventory(repo: &Path, head: &str, paths: &[String]) -> Result<(Vec<Test>, Vec<(String, String)>)> {
    #[cfg(test)]
    INVENTORY_BUILDS.set(INVENTORY_BUILDS.get() + 1);
    let tree = delivered_tree(repo, head).context("STABILITY_DELIVERY_TREE")?;
    let sources = tree.rust_sources(repo).context("STABILITY_SOURCE_READ")?;
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
    let mut inventory = Inventory { parser, sources: sources.iter().map(|(path, body)| (path.as_str(), body.as_str())).collect(),
        tests: Vec::new(), errors: Vec::new(), visited: BTreeSet::new(), platforms: None };
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
    Ok((inventory.tests, inventory.errors))
}

// One synchronous discovery owns these snapshots. Reuse neither failures nor
// results across requests: a later close must reread its delivery and authority.
type Snapshot = (Vec<Test>, Vec<(String, String)>);

#[derive(Default)]
pub(super) struct Discovery {
    trees: BTreeMap<String, std::sync::Arc<DeliveredTree>>,
    inventories: BTreeMap<(String, BTreeSet<String>), std::sync::Arc<Snapshot>>,
}

impl Discovery {
    fn inventory(&mut self, repo: &Path, head: &str, paths: &[String]) -> Result<std::sync::Arc<Snapshot>> {
        if !self.trees.contains_key(head) {
            let tree = delivered_tree(repo, head).context("STABILITY_DELIVERY_TREE")?;
            self.trees.insert(head.to_owned(), tree);
        }
        let manifests = paths.iter().filter(|path| path.ends_with(".rs"))
            .filter_map(|path| self.trees[head].manifest_dir_for_source(path)).collect();
        let key = (head.to_owned(), manifests);
        if let Some(snapshot) = self.inventories.get(&key) {
            return Ok(std::sync::Arc::clone(snapshot));
        }
        let snapshot = std::sync::Arc::new(inventory(repo, head, paths)?);
        self.inventories.insert(key, std::sync::Arc::clone(&snapshot));
        Ok(snapshot)
    }

    pub(super) fn changed(&mut self, repo: &Path, base: &str, head: &str, paths: &[String]) -> Result<Vec<Test>> {
        let prior = self.inventory(repo, base, paths)?;
        let prior = prior.0.iter()
            .map(|test| ((&test.identity, &test.package, &test.harness), &test.source)).collect::<BTreeMap<_, _>>();
        let snapshot = self.inventory(repo, head, paths)?;
        let (current, errors) = snapshot.as_ref();
        if let Some((path, error)) = errors.iter().find(|(path, _)| paths.contains(path)) {
            return Err(anyhow::anyhow!("{path}: {error}"));
        }
        // A new test in an unreferenced source file must not silently disappear
        // from the gate. It either resolves through a Cargo root or needs repair.
        let tree = &self.trees[head];
        let sources = tree.rust_sources(repo).context("STABILITY_SOURCE_READ")?;
        for path in paths.iter().filter(|path| path.ends_with(".rs")) {
            if tree.manifest_dir_for_source(path).is_none() { continue; }
            let Some((_, source)) = sources.iter().find(|(source, _)| source == path) else { continue; };
            let mut parser = Parser::new();
            parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
            let parsed = parser.parse(source, None).context("STABILITY_RUST_PARSE")?;
            let mut standalone = Inventory { parser, sources: BTreeMap::new(), tests: Vec::new(), errors: Vec::new(), visited: BTreeSet::new(), platforms: None };
            standalone.items(parsed.root_node(), source, path, "", "", "", None)?;
            if let Some((_, error)) = standalone.errors.first() {
                return Err(anyhow::anyhow!("{path}: {error}"));
            }
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
                    if let Some((source, error)) = errors.first() {
                        // An uncertain module graph cannot prove a test missing.
                        return Err(anyhow::anyhow!("{source}: {error}"));
                    }
                    return Err(MissingTest(path.clone(), test.identity.name).into());
                }
            }
        }
        Ok(current.iter().filter(|test| paths.contains(&test.identity.path)
            && prior.get(&(&test.identity, &test.package, &test.harness)).copied() != Some(&test.source)).cloned().collect())
    }
}

/// GH #2320: the operating systems each of these tests builds on at `head`,
/// for the tests a `cfg(target_os)` gate restricts. Ungated tests are absent.
pub(crate) fn platform_gates(repo: &Path, head: &str, tests: &[Identity]) -> Result<BTreeMap<Identity, BTreeSet<String>>> {
    let paths = tests.iter().filter(|test| test.path.ends_with(".rs")).map(|test| test.path.clone())
        .collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
    if paths.is_empty() { return Ok(BTreeMap::new()); }
    let (inventory, _) = inventory(repo, head, &paths)?;
    Ok(inventory.into_iter().filter(|test| tests.contains(&test.identity))
        .filter_map(|test| Some((test.identity, test.platforms?))).collect())
}

/// cas-fcb1: the package and harness that own each of these tests in the
/// delivered Cargo inventory (`None` harness is `--lib`). An explicit harness
/// owns its own file and the modules it imports, whatever their file names.
pub(crate) fn owners(repo: &Path, head: &str, tests: &[Identity]) -> Result<BTreeMap<Identity, (String, Option<String>)>> {
    let paths = tests.iter().filter(|test| test.path.ends_with(".rs")).map(|test| test.path.clone())
        .collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
    if paths.is_empty() { return Ok(BTreeMap::new()); }
    let (inventory, _) = inventory(repo, head, &paths)?;
    let mut owners = BTreeMap::new();
    for test in inventory.into_iter().filter(|test| tests.contains(&test.identity)) {
        owners.entry(test.identity).or_insert((test.package, test.harness));
    }
    Ok(owners)
}

pub(crate) fn changed(repo: &Path, base: &str, head: &str, paths: &[String]) -> Result<Vec<Test>> {
    Discovery::default().changed(repo, base, head, paths)
}

/// The one changed-test model (cas-e751): Rust tests whose source differs
/// between `base` (the merge-base with the target) and `head`. Park's
/// stability requirement and the capped runner's repeat set both read it,
/// so a delivery edit the target later superseded is owed by neither.
pub(crate) fn net_changed(repo: &Path, base: &str, head: &str) -> Result<Vec<Test>> {
    let paths = git_paths(repo, &["diff", "--name-only", "--no-renames", "-z", base, head, "--"])
        .context("STABILITY_DIFF_READ")?;
    changed(repo, base, head, &paths)
}

/// The Cargo targets (package, harness; `None` is the library) whose tests a
/// class filter selects in the crates `paths` touch at `head` (cas-8a5c).
pub(crate) fn class_targets(repo: &Path, head: &str, paths: &[String], class: &str) -> Result<BTreeSet<(String, Option<String>)>> {
    Ok(inventory(repo, head, paths)?.0.into_iter().filter(|test| filter_matches(class, &test.identity.name))
        .map(|test| (test.package, test.harness)).collect())
}

pub(crate) fn filter_matches(filter: &str, name: &str) -> bool {
    filter.split('|').any(|union| union.split('&').all(|intersection| {
        let value = intersection.trim().trim_start_matches("test(").trim_end_matches(')');
        value.strip_prefix('=').map_or_else(|| name.contains(value), |exact| name == exact)
    }))
}

pub(crate) fn selected_inputs(repo: &Path, base: &str, head: &str, test: &TargetedTest) -> Result<(Vec<Test>, bool)> {
    #[cfg(test)]
    CLASSIFICATIONS.set(CLASSIFICATIONS.get() + 1);
    let (package, _) = package_selector(&test.package).context("test package")?;
    let paths = git_paths(repo, &["diff", "--name-only", "--no-renames", "-z", base, head, "--"])
        .context("STABILITY_DIFF_READ")?;
    let mut selected = Vec::new();
    let mut uncertain = false;
    for path in paths.into_iter().filter(|path| path.ends_with(".rs")) {
        if let Some(crate::factory_worker_check::widened::Scope::Crate(owner)) = super::super::widened::scope(repo, &path)
            && owner != package { continue; }
        match changed(repo, base, head, std::slice::from_ref(&path)) {
            Ok(tests) => selected.extend(tests.into_iter().filter(|own| own.package == package
                && own.harness == test.harness && filter_matches(&test.filter, &own.identity.name))),
            Err(error) if error.downcast_ref::<MissingTest>().is_some() => return Err(error),
            Err(error) => {
                eprintln!("Changed-test classification uncertain for {path} ({error:#}); widening crate {package} once");
                uncertain = true;
            }
        }
    }
    Ok((selected, uncertain))
}

pub(crate) fn selected(repo: &Path, base: &str, head: &str, test: &TargetedTest) -> Result<Vec<Test>> {
    Ok(selected_inputs(repo, base, head, test)?.0)
}

/// A broader selection can fail without breaking an owned test's existing
/// streak. Restore only stability, never a PASS for that failed selection.
pub(crate) fn restore_passing(cas_root: &Path, repo: &Path, head: &str,
    tests: &[Test], prior: Vec<Proof>, log: &Path) -> Result<()> {
    if prior.is_empty() || clean_head(repo).ok().as_deref() != Some(head) { return Ok(()); }
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*[A-Za-z]")?;
    let summary = regex::Regex::new(r"(?m)^ *Summary +\[[^\r\n]*\] +[0-9]+ tests? run:")?;
    let outcome = regex::Regex::new(r"^ *(\S+) +\[[^\]]*\] +\( *[0-9]+/ *[0-9]+\) +(\S+) +(\S+)$")?;
    let mut passing = tests.iter().filter(|test| !test.parameterized)
        .map(|test| test.identity.clone()).collect::<BTreeSet<_>>();
    // Every attempted repeat must positively prove the identity passed. An
    // empty/truncated log, launch failure or unrun test cannot restore proof.
    for attempt in 1..=REQUIRED_PASSES {
        let path = if attempt == 1 { log.to_path_buf() }
            else { PathBuf::from(format!("{}.repeat-{attempt}", log.display())) };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if attempt > 1 && error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Ok(()),
        };
        let text = ansi.replace_all(&text, "");
        if summary.find_iter(&text).count() != 1 { return Ok(()); }
        passing.retain(|identity| {
            let test = tests.iter().find(|test| test.identity == *identity).expect("owned identity");
            let binary = test.harness.as_ref().map_or_else(|| test.package.clone(),
                |harness| format!("{}::{harness}", test.package));
            let rows = text.lines().filter_map(|line| outcome.captures(line))
                .filter(|row| row[2] == binary && row[3] == identity.name).collect::<Vec<_>>();
            rows.len() == 1 && &rows[0][1] == "PASS"
        });
    }
    for mut proof in prior {
        proof.tests.retain(|test| passing.contains(test));
        if !proof.tests.is_empty() { super::record(cas_root, repo, head, proof)?; }
    }
    Ok(())
}

#[cfg(test)]
mod close_latency_tests {
    use super::*;

    fn commit(repo: &Path) -> String {
        git(repo, &["add", "."]).unwrap();
        git(repo, &["-c", "user.name=fixture", "-c", "user.email=fixture@example.test", "commit", "-qm", "fixture"]).unwrap();
        git(repo, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn requirements_share_inventory_across_files_cas_bc33() {
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        git(repo, &["init", "-qb", "main"]).unwrap();
        std::fs::create_dir(repo.join("src")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]\nname='fixture'\nversion='0.1.0'\n").unwrap();
        std::fs::write(repo.join("src/lib.rs"), "mod a; mod b; mod c;\n").unwrap();
        let paths: Vec<String> = ["a", "b", "c"].map(|name| format!("src/{name}.rs")).into();
        for path in &paths {
            std::fs::write(repo.join(path), "#[test] fn changed() { assert_eq!(1, 1); }\n").unwrap();
        }
        let base = commit(repo);
        for path in &paths {
            std::fs::write(repo.join(path), "#[test] fn changed() { assert_eq!(2, 2); }\n").unwrap();
        }
        let head = commit(repo);
        let ranges = [(base.clone(), head.clone())];
        INVENTORY_BUILDS.set(0);
        let (tests, widened) = super::super::requirements(repo, &head, &base, &paths, Some(&ranges)).unwrap();
        assert_eq!(tests, ["a", "b", "c"].map(|name| Identity::exact(format!("src/{name}.rs"), format!("{name}::changed"))));
        assert!(widened.is_empty());
        assert!(INVENTORY_BUILDS.get() <= 2, "inventory builds: {}", INVENTORY_BUILDS.get());
    }

}
