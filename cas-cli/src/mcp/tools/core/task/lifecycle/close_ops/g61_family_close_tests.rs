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
        head,
        worker: CasService::new(worker, None),
        supervisor: CasService::new(supervisor, None),
    }
}

/// GH #2380, #2369, #2355, #2362, #2378, a named JavaScript and a named Rust
/// selection that run zero tests, an explicit Rust harness, and a locally
/// served journey, each through `task action=close`.
#[cfg(unix)]
#[tokio::test]
async fn g61_gate_family_advisories_allow_close_cas_fcb1() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        const REGISTRY: &str = "journeys:\n- id: join\n  status: live\n  spec: tests/journeys/join.spec.ts\n- id: delete\n  status: live\n  spec: tests/journeys/O1.spec.ts\n- id: audit\n  status: live\n  spec: tests/journeys/Q1.spec.ts\n- id: pair\n  status: live\n  spec: hub-web/e2e/journeys/pair.journey.ts\n";
        let pw = "import { test } from '@playwright/test';\n";
        // (issue, base files, delivered files, deleted files)
        let rows: [(&str, Vec<(&str, String)>, Vec<(&str, String)>, Vec<&str>); 9] = [
            ("#2380", vec![
                ("package.json", "{}".into()), ("playwright.config.ts", "export default {};\n".into()),
                ("docs/qa/journeys.yaml", REGISTRY.into()),
                ("apps/web/package.json", "{}".into()), ("apps/web/vitest.config.ts", "export default {};\n".into()),
                ("apps/web/locales/en.json", "{\"hi\": \"hi\"}\n".into()),
                ("apps/web/src/chat.history.spec.ts", "import { it } from 'vitest';\nimport { chromium } from '@playwright/test';\nimport en from '../locales/en.json';\nfor (const key of Object.keys(en)) it(`renders ${key}`, () => { chromium; });\n".into()),
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
            let head = f.head.as_str();
            let (closer, close) = if issue == "#2378" {
                git(&f.project, &["merge", "-q", "--ff-only", LANE]);
                git(&f.project, &["push", "-q", "origin", "main"]);
                (&f.supervisor, serde_json::json!({"action": "close", "id": ID, "commit_receipt": head,
                    "supervisor_override": true, "reason": "historical delivery; worker-check ran in its worktree"}))
            } else {
                (&f.worker, serde_json::json!({"action": "close", "id": ID, "reason": "fixture delivery"}))
            };
            let held = call(closer, close).await;
            let status = open_task_store(&f.project.join(".cas")).unwrap().get(ID).unwrap().status;
            let accepted = if issue == "#2378" {
                status == TaskStatus::Closed
            } else {
                status == TaskStatus::AwaitingMerge
            };
            if !accepted { failures.push((issue, held)); }
        }
        assert!(failures.is_empty(), "{failures:#?}");
}
