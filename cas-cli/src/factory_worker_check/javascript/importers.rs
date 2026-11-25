//! GH #2146, #2200: a helper holds no tests; the tests importing it do.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

const EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

pub(crate) fn related_specs(repo: &Path, path: &str) -> BTreeMap<&'static str, Vec<String>> {
    related_specs_with_markers(repo, path).specs
}

#[derive(Default)]
pub(in crate::factory_worker_check) struct RelatedSpecs {
    pub specs: BTreeMap<&'static str, Vec<String>>,
    pub marked_runners: BTreeSet<&'static str>,
}

pub(in crate::factory_worker_check) fn related_specs_with_markers(repo: &Path, path: &str) -> RelatedSpecs {
    let target = normalize(&repo.join(path));
    let mut package = target.clone();
    while package.pop() && package.starts_with(repo) && !package.join("package.json").is_file() {}
    if !package.starts_with(repo) { package = repo.to_path_buf(); }
    let graph = cached_graph(repo, &package);
    let Graph { playwright, node, unit, .. } = graph.as_ref();
    let runs_playwright = graph.reached(playwright.clone()).into_iter().chain(playwright.iter().cloned()).collect::<BTreeSet<_>>();
    let mut tests = RelatedSpecs::default();
    for file in graph.reached(vec![target]) {
        let Some(relative) = file.strip_prefix(repo).ok().and_then(Path::to_str) else { continue };
        let marked = if node.contains(&file) { Some(super::NODE_RUNNER) }
            else { unit.get(&file).copied().or_else(|| runs_playwright.contains(&file).then_some("playwright")) };
        let runner = marked.or_else(|| super::runner_hint(repo, relative));
        if let Some(runner) = runner.filter(|runner| super::test_match(relative, Some(runner))) {
            if marked.is_some() { tests.marked_runners.insert(runner); }
            tests.specs.entry(runner).or_default().push(relative.to_owned());
        }
    }
    tests
}

fn cached_graph(repo: &Path, package: &Path) -> std::rc::Rc<Graph> {
    if let Some(graph) = GRAPHS.with(|cache| cache.borrow().as_ref()?.get(package).cloned()) {
        return graph;
    }
    let graph = std::rc::Rc::new(graph(repo, package));
    GRAPHS.with(|cache| cache.borrow_mut().as_mut().map(|cache| cache.insert(package.to_path_buf(), graph.clone())));
    graph
}

pub(super) fn node_specs(repo: &Path, directory: &Path) -> Vec<PathBuf> {
    let graph = cached_graph(repo, directory);
    let mut specs = graph.reached(graph.node.iter().cloned().collect());
    specs.extend(graph.node.iter().cloned());
    specs.into_iter().filter(|path| {
        path.strip_prefix(repo).ok().and_then(Path::to_str).is_some_and(super::node_test_file)
    }).collect()
}

/// One package's import graph: who imports each module, and the files that
/// load Playwright, `node:test` or a unit runner.
struct Graph {
    importers: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    playwright: Vec<PathBuf>,
    node: BTreeSet<PathBuf>,
    unit: BTreeMap<PathBuf, &'static str>,
}

impl Graph {
    fn reached(&self, mut queue: Vec<PathBuf>) -> BTreeSet<PathBuf> {
        let mut seen = BTreeSet::new();
        while let Some(file) = queue.pop() {
            for importer in self.importers.get(&file).into_iter().flatten() {
                if seen.insert(importer.clone()) { queue.push(importer.clone()); }
            }
        }
        seen
    }
}

thread_local! {
    static GRAPHS: std::cell::RefCell<Option<BTreeMap<PathBuf, std::rc::Rc<Graph>>>> = const { std::cell::RefCell::new(None) };
}

/// GH #2331: while `work` runs, each package's import graph is read once. A
/// close asks about every changed file, and each fresh read walks and parses
/// the whole package, built bundles included.
pub(crate) fn with_graph_cache<T>(work: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            if self.0 { GRAPHS.with(|cache| cache.borrow_mut().take()); }
        }
    }
    let _reset = Reset(GRAPHS.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.is_none().then(|| cache.replace(BTreeMap::new())).is_some()
    }));
    work()
}

fn graph(repo: &Path, package: &Path) -> Graph {
    static SPECIFIER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?:\bfrom|\bimport|\brequire\s*\()\s*\(?\s*['"](\.\.?/[^'"\n]+)['"]"#).unwrap()
    });
    let mut importers: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let (mut playwright, mut node, mut unit) = (Vec::new(), BTreeSet::new(), BTreeMap::new());
    let mut walk = ignore::WalkBuilder::new(package);
    walk.filter_entry(|entry| entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir())
        || !entry.path().join("package.json").is_file());
    for entry in walk.build().flatten() {
        let file = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file())
            || !super::javascript(&file.to_string_lossy())
            || file.components().any(|part| part.as_os_str() == "node_modules")
        { continue; }
        let Ok(source) = std::fs::read_to_string(file) else { continue };
        let file = normalize(file);
        if loads_playwright(&source) { playwright.push(file.clone()); }
        if source.contains("'vitest'") || source.contains("\"vitest\"") { unit.insert(file.clone(), "vitest"); }
        else if source.contains("@jest/globals") { unit.insert(file.clone(), "jest"); }
        else if source.contains("'node:test'") || source.contains("\"node:test\"") { node.insert(file.clone()); }
        for capture in SPECIFIER.captures_iter(&source) {
            if let Some(imported) = resolve(file.parent().unwrap_or(repo), &capture[1]) {
                importers.entry(imported).or_default().insert(file.clone());
            }
        }
    }
    Graph { importers, playwright, node, unit }
}

/// Whether `source` loads `@playwright/test` when it runs. `import type`,
/// `export type` and `import("@playwright/test").Page` name only its types.
fn loads_playwright(source: &str) -> bool {
    static KEYWORD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\b(?:import|export|require)\b").unwrap());
    static LOAD: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"^(?:require\s*\(\s*|(?:import|export)\s*(?:([^;'"]*?)\bfrom\s*)?)['"]@playwright/test['"]"#).unwrap()
    });
    static TYPE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\s*type[\s{]").unwrap());
    let starts: Vec<_> = KEYWORD.find_iter(source).map(|keyword| keyword.start()).chain([source.len()]).collect();
    starts.windows(2).any(|pair| LOAD.captures(&source[pair[0]..pair[1]])
        .is_some_and(|load| !load.get(1).is_some_and(|clause| TYPE.is_match(clause.as_str()))))
}

fn resolve(directory: &Path, specifier: &str) -> Option<PathBuf> {
    let base = normalize(&directory.join(specifier));
    let stem = base.with_extension("");
    let renamed = matches!(base.extension().and_then(|ext| ext.to_str()), Some("js" | "mjs" | "cjs" | "jsx"));
    std::iter::once(base.clone())
        .chain(EXTENSIONS.iter().map(|ext| PathBuf::from(format!("{}.{ext}", base.display()))))
        .chain(EXTENSIONS.iter().filter(|_| renamed).map(|ext| stem.with_extension(ext)))
        .chain(EXTENSIONS.iter().map(|ext| base.join(format!("index.{ext}"))))
        .find(|candidate| candidate.is_file())
}

fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => { normal.pop(); }
            part => normal.push(part),
        }
    }
    normal
}
