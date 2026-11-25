//! One capped pass replaces static selection when an input cannot be classified.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Scope {
    File(String),
    /// The runner's dependency graph, rooted at this source file, rather than a
    /// package pass that may execute thousands of unrelated tests. A Playwright
    /// or `node:test` helper's graph is the tests importing it (GH #2146, #2200).
    Related(String),
    Package(String),
    DeclaredCheck(String),
    Crate(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub runner: String,
    pub scope: Scope,
    #[serde(default)]
    pub reason: String,
    /// Playwright specs this run executed, each with the repository-relative
    /// config it ran under (GH #2146). A helper's `Related` proof names every
    /// importing spec; a spec's own `File` proof names itself.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub configs: BTreeMap<String, String>,
}

pub(super) fn valid(proof: &Proof) -> bool {
    let relative = |path: &str| !Path::new(path).is_absolute()
        && Path::new(path).components().all(|part| matches!(part, std::path::Component::Normal(_)));
    if let Scope::DeclaredCheck(manifest) = &proof.scope {
        return proof.runner == javascript::DECLARED_CHECK && relative(manifest)
            && javascript::package_manifest(manifest) && proof.configs.is_empty()
            && matches!(proof.reason.as_str(), "npm run check" | "npm run test");
    }
    matches!(proof.runner.as_str(), "nextest" | "vitest" | "jest" | "playwright" | "node")
        && (proof.configs.is_empty() || proof.runner == "playwright")
        && proof.configs.iter().all(|(spec, config)| !spec.is_empty() && relative(spec) && !config.is_empty() && relative(config))
        && match &proof.scope {
            Scope::Crate(package) => proof.runner == "nextest" && valid_package(package),
            Scope::File(path) => proof.runner != "nextest" && !path.is_empty() && relative(path),
            Scope::Related(path) => matches!(proof.runner.as_str(), "jest" | "vitest" | "playwright" | "node") && !path.is_empty() && relative(path),
            Scope::Package(path) => proof.runner != "nextest" && relative(path),
            Scope::DeclaredCheck(_) => false,
        }
}

pub(crate) fn scope(repo: &Path, path: &str) -> Scope {
    if javascript::javascript(path) {
        let runner = javascript::runner_hint(repo, path);
        if javascript::test_match(path, runner) { return Scope::File(path.into()); }
        // GH #2352: vitest cannot transform a `.vue` file its config has no Vue
        // plugin for, so `vitest related` would crash; that component proves
        // through its package run, or through the specs that import it.
        let unloadable = path.ends_with(".vue") && runner == Some("vitest") && !javascript::vitest_loads_vue(repo, path);
        if (matches!(runner, Some("jest" | "vitest")) && !unloadable) || javascript::related_tests(repo, path).is_some() {
            return Scope::Related(path.into());
        }
    }
    package(repo, path)
}

/// The crate or package that owns `path`, by its nearest manifest.
fn package(repo: &Path, path: &str) -> Scope {
    let rust = path.ends_with(".rs");
    let manifest = if rust { "Cargo.toml" } else { "package.json" };
    let mut directory = repo.join(path);
    while directory.pop() && directory.starts_with(repo) {
        if directory.join(manifest).is_file() {
            if rust {
                if let Some(name) = std::fs::read_to_string(directory.join(manifest)).ok()
                    .and_then(|body| toml::from_str::<toml::Value>(&body).ok())
                    .and_then(|manifest| manifest.get("package")?.get("name")?.as_str().map(str::to_owned))
                { return Scope::Crate(name); }
            } else { break; }
        }
        if directory == repo { break; }
    }
    Scope::Package(directory.strip_prefix(repo).unwrap_or(Path::new("")).to_string_lossy().into())
}

/// GH #2369: the package whose own run proves `path` under `runner` when a
/// focused run of it selects no test: the directory holding the runner's
/// config for it, else its nearest manifest.
pub(crate) fn owning_package(repo: &Path, path: &str, runner: &str) -> Scope {
    javascript::runner_root(repo, path, runner).ok()
        .and_then(|(directory, _)| directory.strip_prefix(repo).ok().map(|relative| Scope::Package(relative.to_string_lossy().into())))
        .unwrap_or_else(|| package(repo, path))
}

/// The package or crate runs that prove `path` (GH #2369): run more, never
/// an impossible narrower demand. A Rust file's owner is its crate's run.
fn owners(repo: &Path, path: &str) -> Vec<Scope> {
    if path.ends_with(".rs") { return vec![package(repo, path)]; }
    if !javascript::javascript(path) { return Vec::new(); }
    let mut owners = vec![package(repo, path)];
    if let Some(runner) = javascript::runner_hint(repo, path).filter(|runner| *runner != "node --test") {
        owners.push(owning_package(repo, path, runner));
    }
    owners
}

/// Whether a passing run of the package owning `path` is recorded: it proves
/// that file's changed tests and widened scopes alike (GH #2369).
///
/// A Rust changed test keeps its five-pass proof unless its named selection
/// ran zero tests; only that fallback's crate run stands in for it.
pub(crate) fn package_proves(passing: &PassingTests<'_>, path: &str) -> bool {
    let rust = path.ends_with(".rs");
    owners(passing.repo, path).iter().any(|owner| passing.records().iter()
        .filter_map(|(_, _, test)| test.widened.as_ref())
        .any(|proof| &proof.scope == owner && (!rust || proof.reason == ZERO_SELECTION_REASON)))
}

/// The recorded reason of a crate run that replaced a zero-test selection.
pub(crate) const ZERO_SELECTION_REASON: &str = "widened:zero-test selection";

/// The journey specs the registry declares at `head` whose Playwright config
/// targets a deployed host: it starts no local `webServer`, itself or through
/// a config it imports. Their declared lane (batch QA against that host)
/// proves them, never a worker's capped run (GH #2362, #2380). A journey
/// against a locally served app stays owed by the worker.
pub(crate) fn deployed_journey_specs(repo: &Path, registry: &crate::qa_registry::Registry) -> BTreeSet<String> {
    registry.journeys.iter().filter_map(|journey| journey.spec.as_ref())
        .filter(|spec| targets_deployed_host(repo, spec)).cloned().collect()
}

fn targets_deployed_host(repo: &Path, spec: &str) -> bool {
    let Some(config) = javascript::runner_root(repo, spec, "playwright").ok().and_then(|(_, config)| config) else {
        return false;
    };
    let import = regex::Regex::new(r#"(?:\bfrom|\bimport)\s*\(?\s*['"](\.\.?/[^'"\n]+)['"]"#).expect("import pattern");
    let source = |path: &Path| std::fs::read_to_string(path).unwrap_or_default();
    let config_source = source(&config);
    let imported = import.captures_iter(&config_source).filter_map(|capture| {
        let base = config.parent()?.join(&capture[1]);
        std::iter::once(base.clone()).chain(["ts", "mts", "cts", "js", "mjs", "cjs"].iter()
            .map(|ext| PathBuf::from(format!("{}.{ext}", base.display())))).find(|path| path.is_file())
    }).map(|path| source(&path)).collect::<Vec<_>>();
    !std::iter::once(&config_source).chain(&imported).any(|text| text.contains("webServer"))
}

/// What resolved for one widened package's changed inputs (GH #2224, #2266).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Covering {
    /// The test files that read at least one input, in path order.
    pub specs: Vec<String>,
    /// The inputs no spec reads: their proof is the package's own.
    pub unresolved: Vec<String>,
}

/// The test files covering each widened package's changed inputs that no
/// runner takes as a target (a JSON catalogue, a SQL migration, a fixture),
/// with the inputs nothing covers. A package no spec covers at all is absent.
pub(crate) fn covering(repo: &Path, head: &str, paths: &[String], scopes: &[Scope]) -> BTreeMap<Scope, Covering> {
    scopes.iter().filter_map(|scope| {
        let Scope::Package(directory) = scope else { return None };
        let inputs: Vec<&str> = paths.iter().map(String::as_str)
            .filter(|path| &package(repo, path) == scope).collect();
        let resolved = javascript::covering_specs(repo, head, directory, &inputs).ok()?;
        let specs: BTreeSet<String> = resolved.values().flatten().cloned().collect();
        let unresolved = resolved.iter().filter(|(_, specs)| specs.is_empty()).map(|(input, _)| input.clone()).collect();
        (!specs.is_empty()).then(|| (scope.clone(), Covering { specs: specs.into_iter().collect(), unresolved }))
    }).collect()
}

pub(crate) fn declared_checks(repo: &Path, head: &str, scopes: &mut [Scope], covering: &BTreeMap<Scope, Covering>) {
    for scope in scopes {
        let Scope::Package(directory) = &*scope else { continue };
        let pathspec = if directory.is_empty() { "." } else { directory.as_str() };
        let manifest = Path::new(directory).join("package.json").to_string_lossy().into_owned();
        if covering.contains_key(scope) || javascript::declared_script(repo, head, &manifest).is_none() {
            continue;
        }
        if git(repo, &["--literal-pathspecs", "ls-tree", "-r", "--name-only", head, "--", pathspec])
            .is_ok_and(|files| !files.lines().any(|path| javascript::javascript(path)
                && javascript::test_match(path, Some("jest")) && package(repo, path) == *scope))
        {
            *scope = Scope::DeclaredCheck(manifest);
        }
    }
}

/// A widened scope is proven by its own pass or, for a package, by a
/// whole-file pass of every spec that covers its inputs, provided those specs
/// cover every input: a spec reading one input says nothing about another.
pub(crate) fn missing(passing: &PassingTests<'_>, scopes: &[Scope], covering: &BTreeMap<Scope, Covering>) -> Vec<Scope> {
    // Records already carry an ancestor's run only when no proof input changed
    // since it (cas-3080), so a documentation-only descendant keeps it.
    let recorded = |wanted: &Scope| passing.records().iter()
        .any(|(tested_head, _, test)| test.widened.as_ref().is_some_and(|proof| {
            &proof.scope == wanted && (!matches!(wanted, Scope::DeclaredCheck(_))
                || tested_head == passing.head && valid(proof))
        }));
    // GH #2146: a Playwright helper holds no tests. Every spec importing it
    // must have run, each under its recorded config, whether through the
    // helper's own run or through that spec's own run.
    let ran = |spec: &str| passing.records().iter().filter_map(|(_, _, test)| test.widened.as_ref()).any(|proof|
        proof.runner == "playwright" && proof.configs.contains_key(spec)
            && (proof.scope == Scope::File(spec.into()) || matches!(&proof.scope, Scope::Related(_))));
    scopes.iter().filter(|scope| {
        // GH #2369: the owning package's own passing run proves any input in it.
        if let Scope::File(path) | Scope::Related(path) = scope && package_proves(passing, path) { return false; }
        if let Scope::Related(helper) = scope {
            let consumers = javascript::related_tests(passing.repo, helper)
                .filter(|(runner, _)| *runner == "playwright").map(|(_, specs)| specs).unwrap_or_default();
            if !consumers.is_empty() { return !consumers.iter().all(|spec| ran(spec)); }
        }
        // GH #2224, #2266: a package is also proven by whole-file passes of the
        // specs covering its inputs, once they cover every input.
        !recorded(scope) && !covering.get(*scope)
            .is_some_and(|found| found.unresolved.is_empty() && found.specs.iter().all(|spec| recorded(&Scope::File(spec.clone()))))
    }).cloned().collect()
}

pub(crate) fn commands(cas_root: &Path, repo: &Path, paths: &[String], scopes: &[Scope],
    covering: &BTreeMap<Scope, Covering>) -> Vec<String> {
    scopes.iter().flat_map(|scope| {
        if let Scope::DeclaredCheck(manifest) = scope {
            return vec![format!("cas factory worker-check --cas-root {} -- {} {}",
                shell_word(&cas_root.to_string_lossy()), javascript::DECLARED_CHECK, shell_word(manifest))];
        }
        let found = covering.get(scope);
        if let Some(Covering { specs, unresolved }) = found && unresolved.is_empty() {
            return specs.iter().find_map(|spec| javascript::runner_hint(repo, spec)).map(|runner| vec![runner])
                .unwrap_or_else(|| vec!["jest", "vitest", "playwright", "node --test"]).into_iter()
                .map(|runner| format!("cas factory worker-check --cas-root {} -- {runner} {}", cas_root.display(), specs.join(" ")))
                .collect();
        }
        if let Scope::Crate(package) = scope {
            return vec![format!("cas factory worker-check --cas-root {} -- nextest run -p {package} --lib -E 'test({package})'", cas_root.display())];
        }
        if let Scope::Related(path) = scope {
            let runner = if let Some((runner, _)) = javascript::related_tests(repo, path) {
                if runner == "node" { "node --test" } else { runner }
            } else if javascript::runner_hint(repo, path) == Some("vitest") {
                "vitest related --run"
            } else { "jest --findRelatedTests" };
            return vec![format!("cas factory worker-check --cas-root {} -- {runner} {path}", cas_root.display())];
        }
        // Only a JavaScript input reaches the runner; a JSON catalogue or SQL
        // migration in the same package shares the scope but is refused (GH #2224, #2266).
        let path = paths.iter().filter(|path| javascript::javascript(path))
            .find(|path| &self::scope(repo, path) == scope).map(String::as_str)
            .or_else(|| if let Scope::File(path) = scope { Some(path.as_str()) } else { None });
        // The inputs nothing covers: some specs read the other inputs, but
        // their passes do not stand in for the whole package.
        let unresolved: Vec<&str> = match found {
            Some(found) => found.unresolved.iter().map(String::as_str).collect(),
            None => paths.iter().map(String::as_str).filter(|input| &self::package(repo, input) == scope).collect(),
        };
        let named = if unresolved.is_empty() { String::new() } else { format!("no spec reads {}; ", unresolved.join(", ")) };
        let Some(path) = path else {
            // GH #2164: the package's manifest selects its own run.
            let Scope::Package(directory) = scope else { return Vec::new() };
            let manifest = Path::new(directory).join("package.json").to_string_lossy().into_owned();
            return javascript::runner_hint(repo, &manifest).map(|runner| vec![runner]).unwrap_or_else(|| vec!["jest", "vitest", "playwright"])
                .into_iter().map(|runner| format!("{named}cas factory worker-check --cas-root {} -- {runner} {manifest}", cas_root.display())).collect();
        };
        let note = (!named.is_empty() && found.is_some()).then(|| format!("{}the covering specs' passes do not prove the package; run the whole package instead", named));
        note.into_iter().chain(javascript::runner_hint(repo, path).map(|runner| vec![runner]).unwrap_or_else(|| vec!["jest", "vitest", "playwright", "node --test"])
            .into_iter().map(|runner| {
                // GH #2195: a spec only a non-default config lists runs under it.
                let config = (runner == "playwright").then(|| javascript::playwright_owner(repo, path)).flatten()
                    .map(|config| format!("--config {} ", config.display())).unwrap_or_default();
                format!("cas factory worker-check --cas-root {} -- {runner} {config}{path}", cas_root.display())
            })).collect()
    }).collect()
}

/// Same receipt envelope and immutable-input checks as focused proof; one kind,
/// with an explicit scope instead of invented source-to-runtime identities.
pub(crate) fn record(cas_root: &Path, repo: &Path, head: &str, proof: Proof, count: u64) -> Result<()> {
    if clean_head(repo)? != head || count == 0 { bail!(crate::gates::tag("G-61", "widened run did not prove a nonempty clean delivery")); }
    let key = hex::encode(Sha256::digest(serde_json::to_vec(&proof)?));
    let selection = TargetedTest { package: proof.runner.clone(), harness: None, filter: format!("test(widened_{key})") };
    let path = test_receipt_path(cas_root, &repo.canonicalize()?, head, &selection);
    let receipt = CheckReceipt { head: head.into(), repo: repo.canonicalize()?, packages: Vec::new(),
        no_default_features: false,
        test: Some(TestReceipt { filter: selection.filter, harness: None, count, stability: None,
            widened: Some(proof), selected_log: None, host_path_opt_out: None, supervisor_recorded: None }) };
    std::fs::create_dir_all(path.parent().context("widened receipt directory")?)?;
    std::fs::write(path, serde_json::to_vec(&receipt)?)?;
    Ok(())
}

pub(crate) fn invalidate(cas_root: &Path, repo: &Path, head: &str, scope: &Scope) -> Result<()> {
    let directory = receipt_path(cas_root, &repo.canonicalize()?, head);
    let Some(directory) = directory.parent().filter(|directory| directory.is_dir()) else { return Ok(()) };
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let Ok(receipt) = serde_json::from_slice::<CheckReceipt>(&std::fs::read(entry.path())?) else { continue; };
        // A failed rerun also withdraws the ancestor run it would carry.
        if receipt.repo == repo.canonicalize()?
            && (receipt.head == head || receipt_head_is_ancestor(&receipt.repo, &receipt.head, head))
            && receipt.test.and_then(|test| test.widened).is_some_and(|proof| &proof.scope == scope)
        { std::fs::remove_file(entry.path())?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GH #2352: vitest cannot transform a `.vue` file without a Vue plugin, so
    /// `vitest related --run <file>.vue` crashes and no receipt can satisfy it.
    /// Such a component proves through its package run, and every command
    /// prescribed for it resolves back to that scope; a config with the plugin
    /// keeps the related-spec proof.
    #[test]
    fn vue_source_without_a_vue_plugin_is_proven_by_its_package_gh2352() {
        let repo = tempfile::tempdir().unwrap();
        for (project, config) in [
            ("plain", "export default {};\n"),
            ("vue", "import vue from '@vitejs/plugin-vue';\nexport default { plugins: [vue()] };\n"),
        ] {
            std::fs::create_dir_all(repo.path().join(project).join("components")).unwrap();
            std::fs::write(repo.path().join(project).join("package.json"), "{}").unwrap();
            std::fs::write(repo.path().join(project).join("vitest.config.ts"), config).unwrap();
            std::fs::write(repo.path().join(project).join("components/Foo.vue"), "<template><p /></template>\n").unwrap();
        }
        let (plain, vue) = ("plain/components/Foo.vue", "vue/components/Foo.vue");
        assert_eq!(scope(repo.path(), vue), Scope::Related(vue.into()));
        let scopes = [scope(repo.path(), plain)];
        assert_eq!(scopes, [Scope::Package("plain".into())]);
        let commands = commands(Path::new("/cas"), repo.path(), &[plain.to_string()], &scopes, &Default::default());
        assert!(!commands.is_empty());
        for command in &commands {
            let args = command.split_once(" -- ").unwrap().1.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
            let files = javascript::invocation(&args).map(|run| run.files).unwrap_or_default();
            assert_eq!(files, [plain], "{command}");
            assert!(files.iter().all(|file| scope(repo.path(), file) == scopes[0]), "{command}");
        }
    }

    /// GH #2266: a migration rename beside a fixture edit widens the package;
    /// with no covering spec resolved, every prescribed command is one the
    /// capped runner accepts, never the SQL or JSON input itself.
    #[test]
    fn package_proof_never_prescribes_a_non_javascript_input() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("apps/backend/test/fixtures")).unwrap();
        std::fs::write(repo.path().join("apps/backend/package.json"), "{}").unwrap();
        let paths = ["apps/backend/prisma/migrations/1_init/migration.sql", "apps/backend/src/scenarios.json",
            "apps/backend/test/fixtures/patterns.ts"].map(String::from);
        let scopes = [scope(repo.path(), &paths[2])];
        assert_eq!(scopes, [Scope::Package("apps/backend".into())]);
        let commands = commands(Path::new("/cas"), repo.path(), &paths, &scopes, &Default::default());
        assert!(!commands.is_empty());
        for command in &commands {
            let args = command.split_once(" -- ").unwrap().1.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(javascript::invocation(&args).map(|run| run.files), Some(vec![paths[2].clone()]), "{command}");
        }
        // GH #2164: with no JavaScript input left, the package's manifest
        // selects its own run, and that selection widens to this package.
        let commands = super::commands(Path::new("/cas"), repo.path(), &paths[..2], &scopes, &Default::default());
        assert!(!commands.is_empty());
        for command in &commands {
            let args = command.split_once(" -- ").unwrap().1.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
            let files = javascript::invocation(&args).map(|run| run.files).unwrap_or_default();
            assert_eq!(files.iter().map(|file| scope(repo.path(), file)).collect::<Vec<_>>(), scopes, "{command}");
        }
    }
}
