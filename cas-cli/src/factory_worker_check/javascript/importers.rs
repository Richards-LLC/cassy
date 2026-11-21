//! GH #2146, #2200: a helper holds no tests; the tests importing it do.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

const EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// The repository-relative Playwright specs that import `path` (GH #2146).
pub(crate) fn playwright_importers(repo: &Path, path: &str) -> Vec<String> {
    related_tests(repo, path).filter(|(runner, _)| *runner == "playwright").map(|(_, tests)| tests).unwrap_or_default()
}

/// The runner and repository-relative test files that import `path`, directly
/// or through other modules of its package, when every one is a Playwright
/// spec or a `node:test` file of one runner (a CI guard, GH #2200). None when
/// no test imports it or another runner's test does: that helper keeps its
/// other proof. Only relative specifiers resolve, so an alias-only import
/// finds no importer and the helper keeps its package proof.
pub(crate) fn related_tests(repo: &Path, path: &str) -> Option<(&'static str, Vec<String>)> {
    static SPECIFIER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?:\bfrom|\bimport|\brequire\s*\()\s*\(?\s*['"](\.\.?/[^'"\n]+)['"]"#).unwrap()
    });
    let target = normalize(&repo.join(path));
    let mut package = target.clone();
    while package.pop() && package.starts_with(repo) && !package.join("package.json").is_file() {}
    if !package.starts_with(repo) { package = repo.to_path_buf(); }
    let mut importers: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let (mut playwright, mut node) = (Vec::new(), BTreeSet::new());
    for entry in ignore::WalkBuilder::new(&package).build().flatten() {
        let file = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file())
            || !super::javascript(&file.to_string_lossy())
            || file.components().any(|part| part.as_os_str() == "node_modules")
        { continue; }
        let Ok(source) = std::fs::read_to_string(file) else { continue };
        let file = normalize(file);
        if source.contains("@playwright/test") { playwright.push(file.clone()); }
        if source.contains("'node:test'") || source.contains("\"node:test\"") { node.insert(file.clone()); }
        for capture in SPECIFIER.captures_iter(&source) {
            if let Some(imported) = resolve(file.parent().unwrap_or(repo), &capture[1]) {
                importers.entry(imported).or_default().insert(file.clone());
            }
        }
    }
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
    let runs_playwright = reached(playwright.clone()).into_iter().chain(playwright).collect::<BTreeSet<_>>();
    let tests: Vec<_> = reached(vec![target]).into_iter().filter_map(|file| {
        let relative = file.strip_prefix(repo).ok()?.to_str()?.to_owned();
        super::test_match(&relative, None).then_some((file, relative))
    }).collect();
    let runner = |file: &PathBuf| if runs_playwright.contains(file) { Some("playwright") }
        else if node.contains(file) { Some("node") } else { None };
    let first = runner(&tests.first()?.0)?;
    tests.iter().all(|(file, _)| runner(file) == Some(first))
        .then(|| (first, tests.into_iter().map(|(_, relative)| relative).collect()))
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
