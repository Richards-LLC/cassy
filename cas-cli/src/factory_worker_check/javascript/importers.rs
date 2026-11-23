//! GH #2146: a Playwright helper holds no tests; the specs importing it do.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

const EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// The repository-relative Playwright specs that import `path`, directly or
/// through other modules of its package. Empty when no spec imports it or when
/// any importing test is not a Playwright spec: that helper keeps its other
/// proof. A Playwright spec loads `@playwright/test` at runtime, itself or
/// through its imports, and imports no other runner. Only relative specifiers
/// resolve, so an alias-only import finds no importer and the helper keeps its
/// package proof.
pub(crate) fn playwright_importers(repo: &Path, path: &str) -> Vec<String> {
    let target = normalize(&repo.join(path));
    let mut package = target.clone();
    while package.pop() && package.starts_with(repo) && !package.join("package.json").is_file() {}
    if !package.starts_with(repo) { package = repo.to_path_buf(); }
    let graph = match GRAPHS.with(|cache| cache.borrow().as_ref().map(|cache| cache.get(&package).cloned())) {
        Some(Some(graph)) => graph,
        Some(None) => {
            let graph = std::rc::Rc::new(graph(repo, &package));
            GRAPHS.with(|cache| cache.borrow_mut().as_mut().map(|cache| cache.insert(package.clone(), graph.clone())));
            graph
        }
        None => std::rc::Rc::new(graph(repo, &package)),
    };
    let Graph { importers, playwright, unit } = graph.as_ref();
    let reached = |roots: Vec<PathBuf>| {
        let mut seen = BTreeSet::new();
        let mut queue = roots;
        while let Some(file) = queue.pop() {
            for importer in importers.get(&file).into_iter().flatten() {
                if seen.insert(importer.clone()) { queue.push(importer.clone()); }
            }
        }
        seen
    };
    let runs_playwright = reached(playwright.clone()).into_iter().chain(playwright.iter().cloned()).collect::<BTreeSet<_>>();
    let specs: Vec<_> = reached(vec![target]).into_iter().filter_map(|file| {
        let relative = file.strip_prefix(repo).ok()?.to_str()?.to_owned();
        super::test_match(&relative, Some("playwright")).then_some((file, relative))
    }).collect();
    if specs.iter().any(|(file, _)| !runs_playwright.contains(file) || unit.contains(file)) { return Vec::new(); }
    specs.into_iter().map(|(_, relative)| relative).collect()
}

/// One package's import graph: who imports each module, the files that load
/// Playwright and the files that load a unit runner.
struct Graph {
    importers: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    playwright: Vec<PathBuf>,
    unit: BTreeSet<PathBuf>,
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
    let (mut playwright, mut unit) = (Vec::new(), BTreeSet::new());
    for entry in ignore::WalkBuilder::new(package).build().flatten() {
        let file = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file())
            || !super::javascript(&file.to_string_lossy())
            || file.components().any(|part| part.as_os_str() == "node_modules")
        { continue; }
        let Ok(source) = std::fs::read_to_string(file) else { continue };
        let file = normalize(file);
        if loads_playwright(&source) { playwright.push(file.clone()); }
        if ["'vitest'", "\"vitest\"", "@jest/globals", "'node:test'", "\"node:test\""].iter().any(|runner| source.contains(runner)) {
            unit.insert(file.clone());
        }
        for capture in SPECIFIER.captures_iter(&source) {
            if let Some(imported) = resolve(file.parent().unwrap_or(repo), &capture[1]) {
                importers.entry(imported).or_default().insert(file.clone());
            }
        }
    }
    Graph { importers, playwright, unit }
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
