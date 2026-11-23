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
    matches!(proof.runner.as_str(), "nextest" | "vitest" | "jest" | "playwright" | "node")
        && (proof.configs.is_empty() || proof.runner == "playwright")
        && proof.configs.iter().all(|(spec, config)| !spec.is_empty() && relative(spec) && !config.is_empty() && relative(config))
        && match &proof.scope {
            Scope::Crate(package) => proof.runner == "nextest" && valid_package(package),
            Scope::File(path) => proof.runner != "nextest" && !path.is_empty() && relative(path),
            Scope::Related(path) => matches!(proof.runner.as_str(), "jest" | "vitest" | "playwright" | "node") && !path.is_empty() && relative(path),
            Scope::Package(path) => proof.runner != "nextest" && relative(path),
        }
}

pub(crate) fn scope(repo: &Path, path: &str) -> Scope {
    if javascript::javascript(path) {
        let runner = javascript::runner_hint(repo, path);
        if javascript::test_match(path, runner) { return Scope::File(path.into()); }
        if matches!(runner, Some("jest" | "vitest")) || javascript::related_tests(repo, path).is_some() {
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

/// A widened scope is proven by its own pass or, for a package, by a
/// whole-file pass of every spec that covers its inputs, provided those specs
/// cover every input: a spec reading one input says nothing about another.
pub(crate) fn missing(passing: &PassingTests<'_>, scopes: &[Scope], covering: &BTreeMap<Scope, Covering>) -> Vec<Scope> {
    // Records already carry an ancestor's run only when no proof input changed
    // since it (cas-3080), so a documentation-only descendant keeps it.
    let recorded = |wanted: &Scope| passing.records().iter()
        .filter_map(|(_, _, test)| test.widened.as_ref()).any(|proof| &proof.scope == wanted);
    // GH #2146: a Playwright helper holds no tests. Every spec importing it
    // must have run, each under its recorded config, whether through the
    // helper's own run or through that spec's own run.
    let ran = |spec: &str| passing.records().iter().filter_map(|(_, _, test)| test.widened.as_ref()).any(|proof|
        proof.runner == "playwright" && proof.configs.contains_key(spec)
            && (proof.scope == Scope::File(spec.into()) || matches!(&proof.scope, Scope::Related(_))));
    scopes.iter().filter(|scope| {
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
            return vec![format!("{named}the package needs one capped run of a changed JavaScript file in {scope:?}: `cas factory worker-check --cas-root {} -- <runner> <changed .js/.ts file>`", cas_root.display())];
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
    }
}
