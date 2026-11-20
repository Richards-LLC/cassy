//! One capped pass replaces static selection when an input cannot be classified.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Scope {
    File(String),
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
            Scope::Package(path) => proof.runner != "nextest" && relative(path),
        }
}

pub(crate) fn scope(repo: &Path, path: &str) -> Scope {
    if javascript::javascript(path) && javascript::test_match(path) { return Scope::File(path.into()); }
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
        let path = paths.iter().find(|path| &self::scope(repo, path) == scope).map(String::as_str)
            .or_else(|| if let Scope::File(path) = scope { Some(path.as_str()) } else { None }).unwrap_or(".");
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
