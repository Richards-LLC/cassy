//! One capped pass replaces static selection when an input cannot be classified.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Scope {
    File(String),
    /// The runner's dependency graph, rooted at this source file, rather than a
    /// package pass that may execute thousands of unrelated tests.
    Related(String),
    Package(String),
    Crate(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub runner: String,
    pub scope: Scope,
    #[serde(default)]
    pub reason: String,
}

pub(super) fn valid(proof: &Proof) -> bool {
    let relative = |path: &str| !Path::new(path).is_absolute()
        && Path::new(path).components().all(|part| matches!(part, std::path::Component::Normal(_)));
    matches!(proof.runner.as_str(), "nextest" | "vitest" | "jest" | "playwright" | "node")
        && match &proof.scope {
            Scope::Crate(package) => proof.runner == "nextest" && valid_package(package),
            Scope::File(path) => proof.runner != "nextest" && !path.is_empty() && relative(path),
            Scope::Related(path) => matches!(proof.runner.as_str(), "jest" | "vitest") && !path.is_empty() && relative(path),
            Scope::Package(path) => proof.runner != "nextest" && relative(path),
        }
}

pub(crate) fn scope(repo: &Path, path: &str) -> Scope {
    if javascript::javascript(path) {
        let runner = javascript::runner_hint(repo, path);
        if javascript::test_match(path, runner) { return Scope::File(path.into()); }
        if matches!(runner, Some("jest" | "vitest")) { return Scope::Related(path.into()); }
    }
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

pub(crate) fn missing(passing: &PassingTests<'_>, scopes: &[Scope]) -> Vec<Scope> {
    scopes.iter().filter(|scope| !passing.records().iter().any(|(head, _, test)|
        head == passing.head && test.widened.as_ref().is_some_and(|proof| &proof.scope == *scope)))
        .cloned().collect()
}

pub(crate) fn commands(cas_root: &Path, repo: &Path, paths: &[String], scopes: &[Scope]) -> Vec<String> {
    scopes.iter().flat_map(|scope| {
        if let Scope::Crate(package) = scope {
            return vec![format!("cas factory worker-check --cas-root {} -- nextest run -p {package} --lib -E 'test({package})'", cas_root.display())];
        }
        if let Scope::Related(path) = scope {
            let runner = if javascript::runner_hint(repo, path) == Some("vitest") {
                "vitest related --run"
            } else { "jest --findRelatedTests" };
            return vec![format!("cas factory worker-check --cas-root {} -- {runner} {path}", cas_root.display())];
        }
        // Only a JavaScript input reaches the runner; a JSON catalogue or SQL
        // migration in the same package shares the scope but is refused (GH #2224, #2266).
        let path = paths.iter().filter(|path| javascript::javascript(path))
            .find(|path| &self::scope(repo, path) == scope).map(String::as_str)
            .or_else(|| if let Scope::File(path) = scope { Some(path.as_str()) } else { None });
        let Some(path) = path else {
            return vec![format!("one capped run of a changed JavaScript file in {scope:?}: `cas factory worker-check --cas-root {} -- <runner> <changed .js/.ts file>`", cas_root.display())];
        };
        javascript::runner_hint(repo, path).map(|runner| vec![runner]).unwrap_or_else(|| vec!["jest", "vitest", "playwright", "node --test"])
            .into_iter().map(|runner| format!("cas factory worker-check --cas-root {} -- {runner} {path}", cas_root.display())).collect()
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

pub(super) fn invalidate(cas_root: &Path, repo: &Path, head: &str, scope: &Scope) -> Result<()> {
    let directory = receipt_path(cas_root, &repo.canonicalize()?, head);
    let Some(directory) = directory.parent().filter(|directory| directory.is_dir()) else { return Ok(()) };
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let Ok(receipt) = serde_json::from_slice::<CheckReceipt>(&std::fs::read(entry.path())?) else { continue; };
        if receipt.head == head && receipt.repo == repo.canonicalize()?
            && receipt.test.and_then(|test| test.widened).is_some_and(|proof| &proof.scope == scope)
        { std::fs::remove_file(entry.path())?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GH #2266: a migration rename beside a fixture edit widens the package;
    /// every prescribed command must be one the capped runner accepts.
    #[test]
    fn package_proof_never_prescribes_a_non_javascript_input() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("apps/backend/test/fixtures")).unwrap();
        std::fs::write(repo.path().join("apps/backend/package.json"), "{}").unwrap();
        let paths = ["apps/backend/prisma/migrations/1_init/migration.sql", "apps/backend/src/scenarios.json",
            "apps/backend/test/fixtures/patterns.ts"].map(String::from);
        let scopes = [scope(repo.path(), &paths[2])];
        assert_eq!(scopes, [Scope::Package("apps/backend".into())]);
        let commands = commands(Path::new("/cas"), repo.path(), &paths, &scopes);
        assert!(!commands.is_empty());
        for command in &commands {
            let args = command.split_once(" -- ").unwrap().1.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(javascript::invocation(&args).map(|run| run.files), Some(vec![paths[2].clone()]), "{command}");
        }
    }
}
