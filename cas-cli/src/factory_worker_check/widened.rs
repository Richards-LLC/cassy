//! One capped pass replaces static selection when an input cannot be classified.
use super::*;
use javascript::{NODE_RUNNER, runner_words};

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
    matches!(proof.runner.as_str(), "nextest" | "vitest" | "jest" | "playwright" | NODE_RUNNER)
        && (proof.configs.is_empty() || proof.runner == "playwright")
        && proof.configs.iter().all(|(spec, config)| !spec.is_empty() && relative(spec) && !config.is_empty() && relative(config))
        && match &proof.scope {
            Scope::Crate(package) => proof.runner == "nextest" && valid_package(package),
            Scope::File(path) => proof.runner != "nextest" && !path.is_empty() && relative(path),
            Scope::Related(path) => matches!(proof.runner.as_str(), "jest" | "vitest" | "playwright" | NODE_RUNNER) && !path.is_empty() && relative(path),
            Scope::Package(path) => proof.runner != "nextest" && relative(path),
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
        if !unloadable && (matches!(runner, Some("jest" | "vitest")) || !javascript::related_specs(repo, path).is_empty()) {
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
    if let Some(runner) = javascript::runner_hint(repo, path).filter(|runner| *runner != NODE_RUNNER) {
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
pub(crate) fn deployed_journey_specs(cas_root: &Path, repo: &Path, head: &str) -> BTreeSet<String> {
    let registry = crate::config::Config::load(cas_root).map(|config| config.qa().registry.path)
        .unwrap_or_else(|_| crate::qa_registry::DEFAULT_PATH.into());
    git(repo, &["show", &format!("{head}:{registry}")]).ok()
        .and_then(|contents| crate::qa_registry::parse(&contents).ok())
        .map(|registry| registry.journeys.into_iter().flat_map(|journey| {
            let additional = journey.extra.get("additional_specs").and_then(|value| value.as_sequence())
                .into_iter().flatten().filter_map(|value| value.as_str().map(str::to_owned)).collect::<Vec<_>>();
            journey.spec.into_iter().chain(additional)
        })
            .filter(|spec| targets_deployed_host(repo, spec)).collect())
        .unwrap_or_default()
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
            .filter(|path| package_input(repo, path, scope)).collect();
        let resolved = javascript::covering_specs(repo, head, directory, &inputs).ok()?;
        let specs: BTreeSet<String> = resolved.values().flatten().cloned().collect();
        let unresolved = resolved.iter().filter(|(_, specs)| specs.is_empty()).map(|(input, _)| input.clone()).collect();
        (!specs.is_empty()).then(|| (scope.clone(), Covering { specs: specs.into_iter().collect(), unresolved }))
    }).collect()
}

fn package_input(repo: &Path, path: &str, wanted: &Scope) -> bool {
    if path.ends_with(".md") { return false; }
    if javascript::javascript(path) { return &scope(repo, path) == wanted; }
    &package(repo, path) == wanted
}

fn file_proves(passing: &PassingTests<'_>, runner: &str, spec: &str) -> bool {
    passing.records().iter().filter_map(|(_, _, test)| test.widened.as_ref()).any(|proof| {
        proof.runner == runner
            && (matches!(&proof.scope, Scope::File(path) if path == spec)
                || (runner == "playwright" && matches!(&proof.scope, Scope::Related(_))))
            && (runner != "playwright" || proof.configs.contains_key(spec))
    })
}

fn outstanding_consumers(passing: &PassingTests<'_>, helper: &str, deployed: &std::cell::OnceCell<BTreeSet<String>>)
    -> Option<Vec<(&'static str, String)>> {
    let consumers = javascript::related_specs(passing.repo, helper);
    if consumers.is_empty() { return None; }
    let packages = owners(passing.repo, helper);
    let records = passing.records();
    Some(consumers.into_iter().flat_map(|(runner, mut specs)| {
        if records.iter().filter_map(|(_, _, test)| test.widened.as_ref()).any(|proof| {
            proof.runner == runner && (packages.contains(&proof.scope)
                || (runner != "playwright" && matches!(&proof.scope, Scope::Related(path) if path == helper)))
        }) { specs.clear(); }
        specs.retain(|spec| {
            (runner != "playwright" || !deployed.get_or_init(|| deployed_journey_specs(passing.cas_root, passing.repo, passing.head)).contains(spec))
                && !file_proves(passing, runner, spec)
        });
        specs.into_iter().map(move |spec| (runner, spec))
    }).collect())
}

/// A widened scope is proven by its own pass or, for a package, by a
/// whole-file pass of every spec that covers its inputs, provided those specs
/// cover every input: a spec reading one input says nothing about another.
pub(crate) fn missing(passing: &PassingTests<'_>, scopes: &[Scope], covering: &BTreeMap<Scope, Covering>) -> Vec<Scope> {
    // Records already carry an ancestor's run only when no proof input changed
    // since it (cas-3080), so a documentation-only descendant keeps it.
    let proofs = || passing.records().iter().filter_map(|(_, _, test)| test.widened.as_ref());
    let recorded = |wanted: &Scope| proofs().any(|proof| &proof.scope == wanted);
    let deployed = std::cell::OnceCell::new();
    scopes.iter().filter(|scope| {
        if let Scope::Related(helper) = scope
            && let Some(consumers) = outstanding_consumers(passing, helper, &deployed)
        {
            return !consumers.is_empty();
        }
        // GH #2369: the owning package's own passing run proves any input in it.
        if let Scope::File(path) | Scope::Related(path) = scope && package_proves(passing, path) { return false; }
        // GH #2224, #2266: a package is also proven by whole-file passes of the
        // specs covering its inputs, once they cover every input.
        !recorded(scope) && !covering.get(*scope)
            .is_some_and(|found| found.unresolved.is_empty() && found.specs.iter().all(|spec| recorded(&Scope::File(spec.clone()))))
    }).cloned().collect()
}

pub(crate) fn coverage_notes(scopes: &[Scope], covering: &BTreeMap<Scope, Covering>) -> String {
    scopes.iter().filter(|scope| matches!(scope, Scope::Package(_))).filter_map(|scope| {
        let unresolved = &covering.get(scope)?.unresolved;
        (!unresolved.is_empty()).then(|| format!(" No covering spec reads {}; use the package's own proof.", unresolved.join(", ")))
    }).collect()
}

pub(crate) fn commands(passing: &PassingTests<'_>, paths: &[String], scopes: &[Scope],
    covering: &BTreeMap<Scope, Covering>) -> Vec<String> {
    let (cas_root, repo) = (passing.cas_root, passing.repo);
    let command = |argv: Vec<String>| worker_check_command_line(cas_root, &argv);
    let deployed = std::cell::OnceCell::new();
    let planned = scopes.iter().filter_map(|scope| {
        match scope {
            Scope::Related(helper) => outstanding_consumers(passing, helper, &deployed).map(|specs| (scope, specs)),
            _ => None,
        }
    }).collect::<BTreeMap<_, _>>();
    let consumers: BTreeSet<_> = planned.values().flatten().cloned().collect();
    let mut commands: Vec<_> = scopes.iter().filter(|scope| !planned.contains_key(*scope)
        && !matches!(scope, Scope::File(path) if consumers.iter().any(|(_, spec)| spec == path))).flat_map(|scope| {
        let found = covering.get(scope);
        if let Some(Covering { specs, unresolved }) = found && unresolved.is_empty() {
            return specs.iter().find_map(|spec| javascript::runner_hint(repo, spec)).map(|runner| vec![runner])
                .unwrap_or_else(|| vec!["jest", "vitest", "playwright", NODE_RUNNER]).into_iter()
                .map(|runner| command(runner_words(runner).map(str::to_owned).chain(specs.iter().cloned()).collect()))
                .collect();
        }
        if let Scope::Crate(package) = scope {
            return vec![command(["nextest", "run", "-p", package, "--lib", "-E", &format!("test({package})")].map(str::to_owned).into())];
        }
        if let Scope::Related(path) = scope {
            let runner = match javascript::runner_hint(repo, path).unwrap_or("jest") {
                "vitest" => "vitest related --run",
                "jest" => "jest --findRelatedTests",
                "playwright" => return Vec::new(),
                runner => runner,
            };
            return vec![command(runner_words(runner).chain([path.as_str()]).map(str::to_owned).collect())];
        }
        // Only a JavaScript input reaches the runner; a JSON catalogue or SQL
        // migration in the same package shares the scope but is refused (GH #2224, #2266).
        let path = paths.iter().filter(|path| javascript::javascript(path))
            .find(|path| &self::scope(repo, path) == scope).map(String::as_str)
            .or_else(|| if let Scope::File(path) = scope { Some(path.as_str()) } else { None });
        let runners = |path| {
            javascript::runner_hint(repo, path)
                .map(|runner| vec![runner]).unwrap_or_else(|| vec!["jest", "vitest", "playwright"])
        };
        let Some(path) = path else {
            // GH #2164: the package's manifest selects its own run.
            let Scope::Package(directory) = scope else { return Vec::new() };
            let manifest = Path::new(directory).join("package.json").to_string_lossy().into_owned();
            return runners(&manifest).into_iter()
                .map(|runner| command(runner_words(runner).chain([manifest.as_str()]).map(str::to_owned).collect())).collect();
        };
        runners(path).into_iter().map(|runner| {
                // GH #2195: a spec only a non-default config lists runs under it.
                let config = (runner == "playwright").then(|| javascript::playwright_owner(repo, path)).flatten();
                command(runner_words(runner).map(str::to_owned)
                    .chain(config.into_iter().flat_map(|config| ["--config".into(), config.to_string_lossy().into_owned()]))
                    .chain([path.into()]).collect())
            }).collect()
    }).collect();
    let mut by_runner: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (runner, spec) in consumers { by_runner.entry(runner).or_default().push(spec); }
    commands.extend(by_runner.into_iter().map(|(runner, specs)| {
        command(runner_words(runner).chain(["--whole-file"]).map(str::to_owned).chain(specs).collect())
    }));
    commands
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
    invalidate_runner(cas_root, repo, head, scope, None)
}

pub(super) fn invalidate_runner(cas_root: &Path, repo: &Path, head: &str, scope: &Scope, runner: Option<&str>) -> Result<()> {
    let directory = receipt_path(cas_root, &repo.canonicalize()?, head);
    let Some(directory) = directory.parent().filter(|directory| directory.is_dir()) else { return Ok(()) };
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let Ok(receipt) = serde_json::from_slice::<CheckReceipt>(&std::fs::read(entry.path())?) else { continue; };
        // A failed rerun also withdraws the ancestor run it would carry.
        if receipt.repo == repo.canonicalize()?
            && (receipt.head == head || receipt_head_is_ancestor(&receipt.repo, &receipt.head, head))
            && receipt.test.and_then(|test| test.widened).is_some_and(|proof| &proof.scope == scope
                && runner.is_none_or(|runner| proof.runner == runner))
        { std::fs::remove_file(entry.path())?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::core::task::lifecycle::close_ops::widened_park_refusals_for_test;

    fn command_args(plan: &str) -> Vec<String> {
        shell_words::split(plan.split_once(" -- ").unwrap().1).unwrap()
    }

    fn has_g61(refusals: &[String]) -> bool {
        refusals.iter().any(|refusal| crate::gates::codes_in(refusal).contains(&"G-61"))
    }

    #[test]
    fn gh2482_visual_qa_delivery_selects_runnable_node_proof() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_BUILD_GUARD", "off");
        env.remove("CAS_SESSION_ID");
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().canonicalize().unwrap().join("worktrees/incident");
        std::fs::write(root.path().join("config.toml"), "[factory]\nepic_base_branch='not-fetched'\n").unwrap();
        let paths = [
            "Cargo.lock",
            "cas-cli/Cargo.toml",
            "cas-cli/src/artifacts/proof.rs",
            "cas-cli/src/builtins/skills/cas-qa-craft/references/independent-pass.md",
            "cas-cli/src/builtins/skills/cas-ui-craft/scripts/visual-qa.mjs",
            "cas-cli/src/builtins/skills/cas-worker/references/details.md",
            "cas-cli/src/config/settings.rs",
            "cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs",
            "cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/blast_radius_park_tests.rs",
            "cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/g61_family_close_tests.rs",
            "cas-cli/src/mcp/tools/core/task/lifecycle/qa_evidence_gate.rs",
            "cas-cli/src/mcp/tools/core/workflow/premerge_verification/automatic.rs",
            "cas-cli/src/qa_evidence.rs",
            "cas-cli/src/qa_evidence_tests.rs",
            "cas-cli/src/qa_ledger.rs",
            "changelog.d/cas-0304.md",
            "docs/product-overview.d/cas-0304.md",
            "docs/qa/batch-qa.md",
            "docs/qa/evidence-close-gate.md",
            "scripts/qa-candidate-build.py",
            "scripts/test-qa-candidate-build.py",
            "scripts/visual-qa-fixtures/batch-park/pass/127-0-0-1-52391-ranking-expanded-light-390x800.png",
            "scripts/visual-qa-fixtures/batch-park/pass/127-0-0-1-52391-ranking-light-390x800.png",
            "scripts/visual-qa-fixtures/batch-park/pass/visual-qa.json",
            "scripts/visual-qa-fixtures/batch-park/scoped-base/127-0-0-1-52519-ranking-expanded-light-390x800.png",
            "scripts/visual-qa-fixtures/batch-park/scoped-base/127-0-0-1-52519-ranking-light-390x800.png",
            "scripts/visual-qa-fixtures/batch-park/scoped-base/visual-qa.json",
            "scripts/visual-qa-fixtures/batch-park/scoped-tip/127-0-0-1-52460-ranking-expanded-light-390x800.png",
            "scripts/visual-qa-fixtures/batch-park/scoped-tip/127-0-0-1-52460-ranking-light-390x800.png",
            "scripts/visual-qa-fixtures/batch-park/scoped-tip/visual-qa.json",
            "scripts/visual-qa-fixtures/batch-park/stale/127-0-0-1-52573-ranking-expanded-light-390x800.png",
            "scripts/visual-qa-fixtures/batch-park/stale/127-0-0-1-52573-ranking-light-390x800.png",
            "scripts/visual-qa-fixtures/batch-park/stale/visual-qa.json",
            "scripts/visual-qa.mjs",
        ].map(String::from);
        let packaged = paths[4].as_str();
        let entry = paths.last().unwrap().as_str();
        let write = |path: &str, text: &str| {
            let file = repo.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        };
        let git = |args: &[&str]| super::git(&repo, &[
            &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"], args,
        ].concat()).unwrap();
        write("package.json", "{}");
        write("hub-web/package.json", "{}");
        let specs = ["scripts/visual-qa-enter-transition.test.mjs", "scripts/visual-qa-readiness.test.mjs", "scripts/visual-qa.test.mjs"];
        for spec in specs {
            let builtin = if spec == specs[0] { "" } else { "import '../cas-cli/src/builtins/skills/cas-ui-craft/scripts/visual-qa.mjs';\n" };
            write(spec, &format!("import test from 'node:test';\nimport './visual-qa.mjs';\n{builtin}test('capture', () => {{}});\n"));
        }
        write("hub-web/preview/visual-qa.test.ts", "import { test } from 'vitest'; import '../../scripts/visual-qa.mjs'; test('preview', () => {});");
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        for path in &paths {
            let body = if path == packaged || path == entry {
                "export const receipt = './visual-qa-fixtures/batch-park/pass/visual-qa.json';"
            } else if path.ends_with(".json") { "{}" } else { "" };
            write(path, body);
        }
        git(&["add", "."]);
        git(&["commit", "-qm", "delivery"]);
        let head = git(&["rev-parse", "HEAD"]);
        assert_eq!(git(&["diff", "--name-only", &base, &head]).lines().collect::<Vec<_>>(), paths);
        let (tests, scopes) = stability::requirements(&repo, &head, &base, &paths, None).unwrap();
        assert!(tests.is_empty());
        assert_eq!(scopes, [Scope::Related(packaged.into()), Scope::Related(entry.into())]);
        let passing = PassingTests::new(root.path(), &repo, &head);
        let plans = commands(&passing, &paths, &scopes, &Default::default());
        assert_eq!(plans.len(), 1, "{plans:?}");
        let args = command_args(&plans[0]);
        assert_eq!(javascript::invocation(&args).unwrap(), javascript::Invocation {
            runner: "node", config: None, files: specs.map(String::from).to_vec(), whole_file: true,
        });
        let refusals = || widened_park_refusals_for_test(root.path(), &repo, "cas-0304", &paths, &head, &base);
        assert!(has_g61(&refusals()));
        javascript::execute(root.path(), &args, &repo, &[]).unwrap();
        let held = refusals();
        assert!(!has_g61(&held), "{held:?}");

        let helper = "scripts/test-helpers.mjs";
        write(helper, "export const label = 'capture';\n");
        git(&["add", helper]);
        git(&["commit", "-qm", "plain helper delivery"]);
        let helper_head = git(&["rev-parse", "HEAD"]);
        assert_eq!(stability::requirements(&repo, &helper_head, &head, &[helper.into()], None).unwrap(),
            (vec![], vec![]));
    }

    #[test]
    fn gh2471_playwright_helper_uses_related_proof_beside_a_vitest_consumer() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_BUILD_GUARD", "off");
        env.remove("CAS_SESSION_ID");
        let state = tempfile::tempdir().unwrap();
        let cas_root = state.path();
        let worktree = cas_root.join("worktrees/web");
        std::fs::create_dir_all(&worktree).unwrap();
        let repo = worktree.as_path();
        let helper = "hub-web/e2e/journeys/hub-double.ts";
        let journey = "hub-web/e2e/journeys/supervisor-commands.journey.ts";
        let preview = "hub-web/preview/browser-page.test.ts";
        let real = "hub-web/e2e/real/real-hub.journey.ts";
        for (path, contents) in [
            (".gitignore", "node_modules/\n"),
            ("hub-web/package.json", r#"{"devDependencies":{"@playwright/test":"1","vitest":"3"}}"#),
            ("hub-web/playwright.config.js", "module.exports = { testDir: 'e2e/journeys', testMatch: '**/*.journey.ts', webServer: { command: 'fixture-local', port: 3000 } };"),
            ("hub-web/playwright.real.config.js", "module.exports = { testDir: 'e2e/real', testMatch: '**/*.journey.ts' };"),
            ("docs/qa/journeys.yaml", "journeys:\n  - id: HUB-J11\n    status: live\n    spec: hub-web/e2e/journeys/supervisor-commands.journey.ts\n    additional_specs:\n      - hub-web/e2e/real/real-hub.journey.ts\n"),
            (helper, "import type { Page } from '@playwright/test'; export class HubDouble {}"),
            (journey, "import { test } from '@playwright/test'; import './hub-double';\ntest('journey', () => {});"),
            (real, "import { test } from '@playwright/test'; import '../journeys/hub-double';\ntest('real', () => {});"),
            (preview, "import { test } from 'vitest'; import '../e2e/journeys/hub-double'; test('preview', () => {});"),
        ] {
            let file = repo.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, contents).unwrap();
        }
        let playwright = repo.join("hub-web/node_modules/.bin/playwright");
        std::fs::create_dir_all(playwright.parent().unwrap()).unwrap();
        std::fs::write(&playwright, javascript::tests::STAND_IN_PLAYWRIGHT).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&playwright, std::fs::Permissions::from_mode(0o755)).unwrap();
        let scopes = [scope(repo, helper)];
        assert_eq!(scopes, [Scope::Related(helper.into())]);
        let git = |args: &[&str]| super::git(repo, &[
            &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"], args,
        ].concat()).unwrap();
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "delivery"]);
        let head = git(&["rev-parse", "HEAD"]);
        assert_eq!(deployed_journey_specs(cas_root, repo, &head), BTreeSet::from([real.into()]));
        assert_eq!(javascript::related_specs(repo, helper), BTreeMap::from([
            ("playwright", vec![journey.into(), real.into()]), ("vitest", vec![preview.into()]),
        ]));
        let paths = [helper.into()];
        let plans = || commands(&PassingTests::new(cas_root, repo, &head), &paths, &scopes, &Default::default())
            .iter().map(|plan| {
                let args = shell_words::split(plan.split_once(" -- ").unwrap().1).unwrap();
                let run = javascript::invocation(&args).unwrap();
                (run.runner, run.files)
            }).collect::<Vec<_>>();
        assert_eq!(plans(), [("playwright", vec![journey.to_owned()]), ("vitest", vec![preview.to_owned()])]);
        javascript::execute(cas_root, &["playwright".into(), helper.into()], repo, &[]).unwrap();
        let proof_missing = || missing(&PassingTests::new(cas_root, repo, &head), &scopes, &Default::default());
        assert_eq!(proof_missing(), scopes);
        assert_eq!(plans(), [("vitest", vec![preview.to_owned()])]);
        record(cas_root, repo, &head, Proof {
            runner: "vitest".into(), scope: Scope::File(preview.into()), reason: String::new(), configs: Default::default(),
        }, 1).unwrap();
        assert!(proof_missing().is_empty());
        assert!(plans().is_empty());
    }

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
        let passing = PassingTests::new(Path::new("/cas"), repo.path(), "HEAD");
        let commands = commands(&passing, &[plain.to_string()], &scopes, &Default::default());
        assert!(!commands.is_empty());
        for command in &commands {
            let args = shell_words::split(command.split_once(" -- ").unwrap().1).unwrap();
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
        let passing = PassingTests::new(Path::new("/cas"), repo.path(), "HEAD");
        let commands = commands(&passing, &paths, &scopes, &Default::default());
        assert!(!commands.is_empty());
        for command in &commands {
            let args = shell_words::split(command.split_once(" -- ").unwrap().1).unwrap();
            assert_eq!(javascript::invocation(&args).map(|run| run.files), Some(vec![paths[2].clone()]), "{command}");
        }
        // GH #2164: with no JavaScript input left, the package's manifest
        // selects its own run, and that selection widens to this package.
        let commands = super::commands(&passing, &paths[..2], &scopes, &Default::default());
        assert!(!commands.is_empty());
        for command in &commands {
            let args = shell_words::split(command.split_once(" -- ").unwrap().1).unwrap();
            let files = javascript::invocation(&args).map(|run| run.files).unwrap_or_default();
            assert_eq!(files.iter().map(|file| scope(repo.path(), file)).collect::<Vec<_>>(), scopes, "{command}");
        }
    }
}
