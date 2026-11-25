//! cas-fcb1: the G-61 gate family replayed through the real task close
//! handler. Every row ends in a runnable prescription whose passing receipt
//! parks the delivery, or parks at once, never an impossible demand.
use super::*;
use crate::mcp::CasService;
use crate::store::{open_agent_store, open_store, open_task_store, open_worktree_store};
use crate::test_support::TestEnvGuard;
use cas_types::{Agent, AgentRole, WorkTarget};
use std::path::{Path, PathBuf};

const PROJECT: &str = "cas-fcb1-fixture";
const WORKER: &str = "g61-worker";
const LANE: &str = "factory/g61-worker";
const ID: &str = "cas-g61f";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "CAS Test")
        .env("GIT_AUTHOR_EMAIL", "cas@example.test")
        .env("GIT_COMMITTER_NAME", "CAS Test")
        .env("GIT_COMMITTER_EMAIL", "cas@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

async fn call(service: &CasService, request: serde_json::Value) -> String {
    let request = serde_json::from_value(request).unwrap();
    match service.task(Parameters(request)).await {
        Ok(result) => result
            .content
            .into_iter()
            .filter_map(|content| match content {
                rmcp::model::ContentBlock::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(error) => error.message.to_string(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    _origin: tempfile::TempDir,
    project: PathBuf,
    cas_dir: PathBuf,
    worker_path: PathBuf,
    head: String,
    worker: CasService,
    supervisor: CasService,
}

/// A project whose pushed worker lane delivers `delivered` (and deletes
/// `deleted`) over `base`, with the task assigned to that worker.
fn fixture(env: &mut TestEnvGuard, base: &[(&str, String)], delivered: &[(&str, String)], deleted: &[&str]) -> Fixture {
    let mut task = Task::new(ID.into(), "G-61 gate family fixture".into());
    let origin = tempfile::tempdir().unwrap();
    git(origin.path(), &["init", "-q", "--bare"]);
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_path_buf();
    let write = |files: &[(&str, String)]| for (path, body) in files {
        std::fs::create_dir_all(project.join(path).parent().unwrap()).unwrap();
        std::fs::write(project.join(path), body).unwrap();
    };
    git(&project, &["init", "-q", "-b", "main"]);
    write(base);
    write(&[(".gitignore", ".cas/\nnode_modules/\ntarget/\n".into())]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "seed"]);
    git(&project, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    git(&project, &["push", "-q", "origin", "main"]);
    git(&project, &["checkout", "-q", "-b", LANE]);
    write(delivered);
    for path in deleted { git(&project, &["rm", "-q", path]); }
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", &format!("fix({ID}): deliver the fixture")]);
    git(&project, &["push", "-q", "origin", LANE]);
    let head = git(&project, &["rev-parse", "HEAD"]);
    git(&project, &["checkout", "-q", "main"]);

    let cas_dir = project.join(".cas");
    crate::store::init_cas_dir(&project).unwrap();
    env.set("CAS_ROOT", &cas_dir);
    env.set("CAS_FACTORY_MODE", "1");
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    std::fs::write(
        cas_dir.join("config.toml"),
        format!("[project]\ncanonical_id=\"{PROJECT}\"\n[verification]\nenabled=false\n[factory]\nepic_base_branch='main'\n"),
    ).unwrap();
    open_store(&cas_dir).unwrap().init().unwrap();
    open_worktree_store(&cas_dir).unwrap().init().unwrap();
    open_task_store(&cas_dir).unwrap().init().unwrap();
    let worker_path = cas_dir.join("worktrees").join(WORKER);
    std::fs::create_dir_all(worker_path.parent().unwrap()).unwrap();
    git(&project, &["worktree", "add", "-q", worker_path.to_str().unwrap(), LANE]);
    std::fs::create_dir_all(worker_path.join(".cas")).unwrap();
    std::fs::write(worker_path.join(".cas/config.toml"), format!("[project]\ncanonical_id=\"{PROJECT}\"\n")).unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    agents.init().unwrap();
    agents.register(&Agent::new_with_role("g61-worker-session".into(), WORKER.into(), AgentRole::Worker)).unwrap();
    agents.register(&Agent::new_with_role("g61-supervisor-session".into(), "supervisor".into(), AgentRole::Supervisor)).unwrap();
    let worker = CasCore::with_daemon(cas_dir.clone(), None, None);
    worker.set_agent_id_for_testing("g61-worker-session".into());
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("g61-supervisor-session".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some(WORKER.into());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: format!("project:{PROJECT}"),
        target_branch: "main".into(),
    });
    open_task_store(&cas_dir).unwrap().add(&task).unwrap();
    agents.try_claim(ID, "g61-worker-session", 1800, Some("assigned")).unwrap();
    Fixture {
        _dir: dir,
        _origin: origin,
        project,
        cas_dir,
        worker_path,
        head,
        worker: CasService::new(worker, None),
        supervisor: CasService::new(supervisor, None),
    }
}

fn executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn g61_gabber_declared_check_and_release_only_journeys() {
    use crate::factory_worker_check;
    let mut env = TestEnvGuard::temp_home();
    let manifest = "apps/dm/order's emails/package.json";
    let source = "apps/dm/order's emails/order.vue";
    let package = fixture(&mut env, &[
        ("package.json", "{}".into()),
        ("playwright.config.ts", "export default {};".into()),
        (manifest, r#"{"scripts":{"check":"test -f order.vue","test":"exit 1"}}"#.into()),
        (source, "<template>Draft order</template>".into()),
    ], &[(source, "<template>Order ready</template>".into())], &[]);
    let close = serde_json::json!({"action": "close", "id": ID, "reason": "fixture delivery"});
    let held = call(&package.worker, close.clone()).await;
    let check = ["package-check", manifest].map(str::to_owned);
    let command = |root: &Path| factory_worker_check::widened::commands(root, &package.worker_path, &[],
        &[factory_worker_check::widened::Scope::DeclaredCheck(manifest.into())], &Default::default()).remove(0);
    let quoted_root = package.cas_dir.join("worker's root");
    let words = std::process::Command::new("sh").args(["-c", &format!("set -- {}; printf '%s\\n' \"$@\"", command(&quoted_root))])
        .output().unwrap();
    let expected = format!("cas\nfactory\nworker-check\n--cas-root\n{}\n--\npackage-check\n{manifest}\n", quoted_root.display());
    let prescribed = held.contains(&command(&package.cas_dir)) && words.status.success() && words.stdout == expected.as_bytes();
    let package_failure = if !crate::gates::codes_in(&held).contains(&"G-61") || !prescribed {
        Some(held)
    } else {
        assert!(factory_worker_check::allowed_args(&check));
        factory_worker_check::execute_at(&package.cas_dir, &check, &package.worker_path, Path::new("unused-cargo")).unwrap();
        let parked = call(&package.worker, close.clone()).await;
        let store = open_task_store(&package.cas_dir).unwrap();
        let task = store.get(ID).unwrap();
        assert_eq!(task.status, cas_types::TaskStatus::AwaitingMerge, "{parked}");
        assert!(task.deliverables.test_receipts.iter().any(|receipt| {
            receipt.tested_head == package.head && receipt.widened.as_ref().is_some_and(|proof| {
                proof["runner"] == "package-check" && proof["scope"]["DeclaredCheck"] == manifest
            })
        }));
        None
    };

    let mut failures = Vec::new();
    for (id, spec, marker, guard, fan_out) in [
        ("publish", "tests/journeys/publish.spec.ts", "staging_only", "if (!process.env.STAGING) throw Error();", false),
        ("checkout", "tests/journeys/checkout.spec.ts", "paid", "if (!process.env.PAYMENT_TOKEN) throw Error();", false),
        ("translation", "tests/journeys/translation.spec.ts", "staging_only", "if (!process.env.STAGING) throw Error();", true),
    ] {
        let registry = format!("journeys:\n- id: {id}\n  status: live\n  spec: {spec}\n  worker_check: {marker}\n");
        let data = "tests/journeys/locale.json";
        let imports = if fan_out { "import messages from './locale.json';\n" } else { "" };
        let test = |extra: &str| format!("import {{ test }} from '@playwright/test';\n{imports}test('complete', () => {{ {guard} {extra} }});\n");
        let changed = if fan_out { (data, r#"{"hello":"Hi","bye":"Bye"}"#.into()) } else { (spec, test("void 1;")) };
        let delivery = fixture(&mut env, &[
            ("package.json", "{}".into()),
            ("playwright.config.ts", "export default { webServer: { command: 'npm run preview' } };".into()),
            ("docs/qa/journeys.yaml", registry),
            (data, r#"{"hello":"Hi"}"#.into()),
            (spec, test("")),
        ], &[changed], &[]);
        let parked = call(&delivery.worker, close.clone()).await;
        let store = open_task_store(&delivery.cas_dir).unwrap();
        if store.get(ID).unwrap().status != cas_types::TaskStatus::AwaitingMerge {
            failures.push((id, parked));
            continue;
        }
        assert!(!crate::gates::codes_in(&parked).contains(&"G-61"));
        let impact = store.get_execution_state(ID).unwrap().unwrap();
        assert_eq!(impact["qa_impact"]["g61_deferred_journeys"], serde_json::json!([
            {"id": id, "spec": spec, "reason": marker}
        ]));
    }
    let raw = "tests/checkout.spec.ts";
    let unlisted = "tests/local.spec.ts";
    let locale = "tests/locale.json";
    let config = "playwright.config.ts";
    let registry = "docs/qa/journeys.yaml";
    let spec = "import { test } from '@playwright/test';\nimport messages from './locale.json';\ntest('complete', () => { void messages; });\n";
    let data = fixture(&mut env, &[
        ("package.json", "{}".into()),
        (config, "export default { webServer: { command: 'npm run preview' } };".into()),
        (registry, format!("spec_checks:\n- {{spec: {raw}, worker_check: staging_only}}\n")),
        (locale, r#"{"hello":"Hi"}"#.into()),
        (raw, spec.into()),
        (unlisted, spec.into()),
    ], &[(locale, r#"{"hello":"Hi","bye":"Bye"}"#.into())], &[]);
    let held = call(&data.worker, close.clone()).await;
    assert!(crate::gates::codes_in(&held).contains(&"G-61"));
    assert_eq!(open_task_store(&data.cas_dir).unwrap().get(ID).unwrap().status, TaskStatus::InProgress);
    factory_worker_check::widened::record(&data.cas_dir, &data.worker_path, &data.head, factory_worker_check::widened::Proof {
        runner: "playwright".into(), scope: factory_worker_check::widened::Scope::File(unlisted.into()),
        reason: String::new(), configs: [(unlisted.into(), config.into())].into(),
    }, 1).unwrap();
    let parked = call(&data.worker, close).await;
    let store = open_task_store(&data.cas_dir).unwrap();
    if store.get(ID).unwrap().status != TaskStatus::AwaitingMerge {
        failures.push((raw, parked));
    } else {
        let state = store.get_execution_state(ID).unwrap().unwrap();
        assert_eq!(state["qa_impact"]["g61_deferred_journeys"], serde_json::json!([
            {"id": raw, "spec": raw, "reason": "staging_only"}
        ]));
        assert_eq!(state["qa_impact"]["journeys"], serde_json::json!([]));
        let mut rows = Vec::new();
        let error = crate::cli::qa_ledger::add_release_obligations(store.as_ref(), [(ID, "wrong-anchor")], &mut rows).unwrap_err();
        assert_eq!(error.to_string().split(':').next(), Some("G61_DEFERRED_STATE_MISSING"));
        crate::cli::qa_ledger::add_release_obligations(store.as_ref(), [(ID, data.head.as_str())], &mut rows).unwrap();
        let ledger = crate::qa_ledger::scaffold_with_coverage(&store.get(ID).unwrap(), &rows, &data.head, &[ID.into()]);
        let ledger = crate::qa_ledger::parse(&ledger, 1);
        assert_eq!(ledger.rows.iter().map(|row| (row.id.as_str(), row.verdict)).collect::<Vec<_>>(),
            [(raw, crate::qa_ledger::Verdict::NotExercised)]);

        let qa = Task::new("cas-g61qa".into(), "Batch QA fixture".into());
        store.add(&qa).unwrap();
        env.set_current_dir(&data.worker_path);
        let cli = crate::cli::try_parse_from_with_wordmark(["cas", "qa", "ledger", "init", "--task", &qa.id, "--covers", ID]).unwrap();
        let Some(crate::cli::Commands::Qa(command)) = &cli.command else { unreachable!() };
        let mut incomplete = state["qa_impact"].clone();
        incomplete.as_object_mut().unwrap().remove("g61_deferred_journeys");
        store.patch_execution_state(ID, &serde_json::json!({"qa_impact": incomplete})).unwrap();
        let error = crate::cli::qa::execute(command, &cli, Some(&data.cas_dir)).unwrap_err();
        assert_eq!(error.to_string().split(':').next(), Some("G61_DEFERRED_STATE_MISSING"));
        store.patch_execution_state(ID, &serde_json::json!({"qa_impact": state["qa_impact"]})).unwrap();
        crate::cli::qa::execute(command, &cli, Some(&data.cas_dir)).unwrap();
        let config = crate::config::Config::load(&data.cas_dir).unwrap();
        let artifacts = crate::config::resolved_factory_artifact_paths(&data.cas_dir, config.factory().artifacts_root.as_deref());
        let body = std::fs::read_to_string(artifacts.project_root.join(&qa.id).join("LEDGER.md")).unwrap();
        assert!(crate::qa_ledger::parse(&body, 1).rows.iter().any(|row|
            row.id == raw && row.verdict == crate::qa_ledger::Verdict::NotExercised));
    }
    let missing = fixture(&mut env, &[(locale, r#"{"hello":"Hi"}"#.into())], &[
        (registry, format!("spec_checks:\n- {{spec: {raw}, worker_check: staging_only}}\n")),
    ], &[]);
    let held = call(&missing.worker, serde_json::json!({"action": "close", "id": ID, "reason": "fixture delivery"})).await;
    assert!(crate::gates::codes_in(&held).contains(&"G-61") && held.contains("untracked_spec_check"), "{held}");
    let unit = "tests/vitest.spec.ts";
    let invalid = fixture(&mut env, &[(unit, "import { test } from 'vitest';\ntest('unit', () => {});\n".into())], &[
        (registry, format!("spec_checks:\n- {{spec: {unit}, worker_check: paid}}\n")),
    ], &[]);
    let held = call(&invalid.worker, serde_json::json!({"action": "close", "id": ID, "reason": "fixture delivery"})).await;
    assert!(crate::gates::codes_in(&held).contains(&"G-61") && held.contains("non_playwright_spec_check"), "{held}");
    assert!(package_failure.is_none() && failures.is_empty(), "Maizzle: {package_failure:?}; journeys: {failures:?}");
}

/// GH #2380, #2369, #2355, #2362, #2378, a named JavaScript and a named Rust
/// selection that run zero tests, an explicit Rust harness, and a locally
/// served journey, each through `task action=close`.
#[cfg(unix)]
#[tokio::test]
async fn g61_gate_family_ends_runnable_or_accepted_cas_fcb1() {
    use crate::factory_worker_check::{javascript, stability, widened};
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        // `related` finds no importer; a named run matches no runtime name;
        // the package run passes.
        const VITEST: &str = "#!/usr/bin/env python3\nimport json, sys\nfrom pathlib import Path\nargs = sys.argv[1:]\nrelated = 'related' in args\nnamed = '--testNamePattern' in args\nresults = [] if related else [{'name': str(Path('pages/index.test.ts').resolve()), 'status': 'passed',\n    'assertionResults': [{'fullName': 'renders at runtime' if named else 'renders', 'status': 'skipped' if named else 'passed'}]}]\nPath(args[args.index('--outputFile') + 1]).write_text(json.dumps({'success': not related, 'testResults': results}))\nraise SystemExit(1 if related else 0)\n";
        const REGISTRY: &str = "journeys:\n- id: join\n  status: live\n  spec: tests/journeys/join.spec.ts\n- id: delete\n  status: live\n  spec: tests/journeys/O1.spec.ts\n- id: audit\n  status: live\n  spec: tests/journeys/Q1.spec.ts\n- id: pair\n  status: live\n  spec: hub-web/e2e/journeys/pair.journey.ts\n";
        let pw = "import { test } from '@playwright/test';\n";
        // (issue, base files, delivered files, deleted files)
        let rows: [(&str, Vec<(&str, String)>, Vec<(&str, String)>, Vec<&str>); 9] = [
            ("#2380", vec![
                ("package.json", "{}".into()), ("playwright.config.ts", "export default {};\n".into()),
                ("docs/qa/journeys.yaml", REGISTRY.into()),
                ("apps/web/package.json", "{}".into()), ("apps/web/vitest.config.ts", "export default {};\n".into()),
                ("apps/web/locales/en.json", "{\"hi\": \"hi\"}\n".into()),
                ("apps/web/src/chat.history.spec.ts", "import { it } from 'vitest';\nimport { chromium } from '@playwright/test';\nimport en from '../locales/en.json';\nit('renders history', () => { chromium; en; });\n".into()),
                ("tests/journeys/join.spec.ts", format!("{pw}import copy from '../../apps/web/locales/en.json';\ntest('joins', () => {{ copy; }});\n")),
            ], vec![("apps/web/locales/en.json", "{\"hi\": \"hello\"}\n".into())], vec![]),
            ("#2369", vec![
                ("apps/web/package.json", "{}".into()),
                ("apps/web/vitest.config.ts", "import vue from '@vitejs/plugin-vue';\nexport default { plugins: [vue()] };\n".into()),
                ("apps/web/pages/files.vue", "<template><p /></template>\n".into()),
                ("apps/web/pages/index.test.ts", "import { it } from 'vitest';\nit('renders', () => {});\n".into()),
            ], vec![("apps/web/pages/files.vue", "<template><p>files</p></template>\n".into())], vec![]),
            // A named changed-test selection that runs zero tests.
            ("named", vec![
                ("apps/web/package.json", "{}".into()), ("apps/web/vitest.config.ts", "export default {};\n".into()),
                ("apps/web/pages/index.test.ts", "import { it } from 'vitest';\nit('renders', () => {});\n".into()),
            ], vec![("apps/web/pages/index.test.ts", "import { it } from 'vitest';\nit('renders', () => { 1; });\n".into())], vec![]),
            ("#2355", vec![
                ("package.json", "{}".into()), ("playwright.config.ts", "export default {};\n".into()),
                ("docs/LOCAL_GATES.md", "gates\n".into()),
                ("tests/config/gate.ts", "export const gate = 1;\n".into()),
                ("tests/keep.spec.ts", format!("{pw}import {{ readFileSync }} from 'node:fs';\ntest('reads gates', () => {{ readFileSync(new URL('../docs/LOCAL_GATES.md', import.meta.url)); }});\n")),
                ("tests/old1.spec.ts", format!("{pw}test('old one', () => {{}});\n")),
                ("tests/old2.spec.ts", format!("{pw}test('old two', () => {{}});\n")),
            ], vec![("docs/LOCAL_GATES.md", "gates, fewer\n".into()), ("tests/config/gate.ts", "// kept\nexport const gate = 1;\n".into())],
            vec!["tests/old1.spec.ts", "tests/old2.spec.ts"]),
            ("#2362", vec![
                ("package.json", "{}".into()), ("docs/qa/journeys.yaml", REGISTRY.into()),
                ("tests/journeys/playwright.config.ts", "export default {};\n".into()),
                ("tests/journeys/flows.ts", "export const flow = () => 1;\n".into()),
                ("tests/journeys/O1.spec.ts", format!("{pw}import {{ flow }} from './flows';\ntest('deletes workspace', () => {{ flow(); }});\n")),
                ("tests/journeys/Q1.spec.ts", format!("{pw}import {{ flow }} from './flows';\ntest('audits shapes', () => {{ flow(); }});\n")),
            ], vec![
                ("tests/journeys/flows.ts", "export const flow = () => 2;\n".into()),
                ("tests/journeys/O1.spec.ts", format!("{pw}import {{ flow }} from './flows';\ntest('deletes workspace', () => {{ flow(); flow(); }});\n")),
            ], vec![]),
            // A registered journey against a locally served app stays owed.
            ("local journey", vec![
                ("docs/qa/journeys.yaml", REGISTRY.into()),
                ("hub-web/package.json", "{}".into()),
                ("hub-web/playwright.base.ts", "export default { webServer: { command: 'npm run preview' } };\n".into()),
                ("hub-web/playwright.config.ts", "import base from './playwright.base';\nexport default base;\n".into()),
                ("hub-web/e2e/journeys/pair.journey.ts", format!("{pw}test('pairs a device', () => {{}});\n")),
            ], vec![("hub-web/e2e/journeys/pair.journey.ts", format!("{pw}test('pairs a device', () => {{ 1; }});\n"))], vec![]),
            // A named Rust selection that runs zero tests.
            ("rust", vec![
                ("Cargo.toml", "[package]\nname='fixture'\nversion='0.1.0'\nautotests=false\n".into()),
                ("docs/qa/journeys.yaml", "journeys:\n- {id: rust, status: live, spec: src/lib.rs, worker_check: paid}\n".into()),
                ("src/lib.rs", "#[test]\nfn kept() { let before = 0; }\n".into()),
            ], vec![("src/lib.rs", "#[test]\nfn kept() {}\n".into())], vec![]),
            // A changed test in an explicit harness whose file name differs
            // from the harness name (`autotests = false`).
            ("harness", vec![
                ("Cargo.toml", "[package]\nname='fixture'\nversion='0.1.0'\nautotests=false\n[[test]]\nname='integration_cli'\npath='tests/cli_test.rs'\n".into()),
                ("src/lib.rs", "pub fn run() {}\n".into()),
                ("tests/cli_test.rs", "#[test]\nfn runs() { let before = 0; }\n".into()),
            ], vec![("tests/cli_test.rs", "#[test]\nfn runs() {}\n".into())], vec![]),
            ("#2378", vec![
                ("package.json", "{}".into()),
                ("a.test.mjs", "import { test } from 'node:test';\ntest('fixed', () => {});\n".into()),
            ], vec![("a.test.mjs", "import { test } from 'node:test';\nconst name = ['dyn'].join('');\ntest(name, () => {});\n".into())], vec![]),
        ];
        let mut failures = Vec::new();
        for (issue, base_files, delivered, deleted) in rows {
            let f = fixture(&mut env, &base_files, &delivered, &deleted);
            let (cas, repo, head) = (f.cas_dir.as_path(), f.worker_path.as_path(), f.head.as_str());
            // #2378: the delivery is already on the target and its worker is
            // gone; the supervisor's merged-receipt close reads the project
            // checkout, while the proof ran in the worker's worktree.
            let (closer, close) = if issue == "#2378" {
                git(&f.project, &["merge", "-q", "--ff-only", LANE]);
                git(&f.project, &["push", "-q", "origin", "main"]);
                (&f.supervisor, serde_json::json!({"action": "close", "id": ID, "commit_receipt": head,
                    "supervisor_override": true, "reason": "historical delivery; worker-check ran in its worktree"}))
            } else {
                (&f.worker, serde_json::json!({"action": "close", "id": ID, "reason": "fixture delivery"}))
            };
            let held = call(closer, close.clone()).await;
            let commands: Vec<Vec<String>> = held
                .split("worker-check --cas-root ").skip(1)
                .filter_map(|tail| tail.split_once(" -- ").map(|(_, command)| command.split(['`', ';']).next().unwrap()
                    .split(". ").next().unwrap().trim_end_matches(['.', '\n']).to_string()))
                .map(|command| command.replace('\'', "").split_whitespace().map(str::to_owned).collect())
                .collect();
            let (rust, script): (Vec<_>, Vec<_>) = commands.into_iter().partition(|words| words.first().is_some_and(|word| word == "nextest"));
            for words in &rust {
                assert!(crate::factory_worker_check::allowed_args(words), "{issue}: unrunnable prescription {words:?}");
            }
            let prescribed: Vec<javascript::Invocation> = script.iter().map(|words| javascript::invocation(words)
                .unwrap_or_else(|| panic!("{issue}: unrunnable prescription {words:?}"))).collect();
            for run in &prescribed {
                assert!(run.files.iter().all(|file| repo.join(file).exists()), "{issue}: {held}");
            }
            assert!(deleted.iter().all(|path| !held.contains(path)), "{issue}: {held}");
            match issue {
                "#2380" => {
                    let spec = "apps/web/src/chat.history.spec.ts";
                    if prescribed.iter().map(|run| (run.runner, run.files.clone())).collect::<Vec<_>>() != [("vitest", vec![spec.to_string()])] {
                        failures.push((issue, held));
                        continue;
                    }
                    widened::record(cas, repo, head, widened::Proof { runner: "vitest".into(),
                        scope: widened::Scope::File(spec.into()), reason: String::new(), configs: Default::default() }, 1).unwrap();
                }
                "#2369" | "named" => {
                    executable(&repo.join("apps/web/node_modules/.bin/vitest"), VITEST);
                    let [run] = prescribed.as_slice() else { panic!("{issue}: {held}") };
                    let related = if issue == "#2369" { vec!["related", "--run"] } else { Vec::new() };
                    let args = std::iter::once("vitest").chain(related).map(String::from).chain(run.files.clone()).collect::<Vec<_>>();
                    if let Err(error) = javascript::execute(cas, &args, repo, &[]) {
                        failures.push((issue, format!("{error:#}")));
                        continue;
                    }
                }
                "#2355" => {
                    assert!(!prescribed.is_empty(), "{issue}: {held}");
                    widened::record(cas, repo, head, widened::Proof { runner: "playwright".into(),
                        scope: widened::Scope::Package(String::new()), reason: String::new(), configs: Default::default() }, 1).unwrap();
                }
                "#2362" => {}
                "local journey" => {
                    let journey = "hub-web/e2e/journeys/pair.journey.ts";
                    if prescribed.iter().map(|run| (run.runner, run.files.clone())).collect::<Vec<_>>() != [("playwright", vec![journey.to_string()])] {
                        failures.push((issue, held));
                        continue;
                    }
                    stability::record(cas, repo, head, stability::Proof {
                        runner: "playwright".into(), consecutive_passes: stability::REQUIRED_PASSES, elapsed_ms: 1,
                        tests: vec![stability::Identity::exact(journey, "pairs a device")], inactive: Vec::new(),
                    }).unwrap();
                }
                "rust" | "harness" => {
                    let [words] = rust.as_slice() else { panic!("{issue}: {held}") };
                    // "rust": nextest's own zero-selection status for the named
                    // run, then the crate run passes. "harness": the named run
                    // passes under the harness the inventory owns.
                    let cargo = f.cas_dir.join("fake-cargo");
                    executable(&cargo, if issue == "rust" {
                        "#!/bin/sh\ncase \" $* \" in\n  *' -E '*) echo '    Starting 0 tests across 1 binary (1 skipped)'; echo 'error: no tests to run'; exit 4;;\nesac\necho '        PASS [ 0.01s] (1/1) fixture kept'\necho '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped'\n"
                    } else {
                        "#!/bin/sh\necho '        PASS [ 0.01s] (1/1) fixture::integration_cli runs'\necho '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped'\n"
                    });
                    if let Err(error) = crate::factory_worker_check::execute_at(cas, words, repo, &cargo) {
                        failures.push((issue, format!("{words:?}: {error:#}")));
                        continue;
                    }
                }
                "#2378" => {
                    javascript::execute(cas, &["node".into(), "--test".into(), "a.test.mjs".into()], repo, &[]).unwrap();
                }
                _ => unreachable!(),
            }
            let after = if issue == "#2362" { held } else { call(closer, close).await };
            let accepted = if issue == "#2378" {
                !crate::gates::codes_in(&after).contains(&"G-61")
            } else {
                after.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff)
            };
            if !accepted { failures.push((issue, after)); }
        }
        assert!(failures.is_empty(), "{failures:#?}");
}
