use super::*;
use crate::store::{init_cas_dir, open_agent_store, open_task_store, open_verification_store};
use cas_types::{Task, VerificationProofBoundary};
use serde_json::{Value, json};
use std::process::Command;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    cas: PathBuf,
    supervisor: CasCore,
    spec: CasCore,
    standards: CasCore,
    worker: CasCore,
    base: String,
    dispatch: String,
}
fn git_cmd(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
fn core(cas: &Path, id: &str) -> CasCore {
    let core = CasCore::with_daemon(cas.to_path_buf(), None, None);
    core.set_agent_id_for_testing(id.to_string());
    core
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        git_cmd(&root, &["init", "-b", "delivery"]);
        git_cmd(&root, &["config", "user.email", "review@example.test"]);
        git_cmd(&root, &["config", "user.name", "Shadow fixture"]);
        std::fs::write(root.join(".gitignore"), ".cas/\n").unwrap();
        std::fs::write(root.join("value.txt"), "base\n").unwrap();
        std::fs::write(
            root.join("CODING_STANDARDS.md"),
            "# Standards\nSmells are judgement calls, subject to repository rules.\n",
        )
        .unwrap();
        git_cmd(&root, &["add", "."]);
        git_cmd(&root, &["commit", "-m", "base"]);
        let base = git::head(&root).unwrap();
        std::fs::write(root.join("value.txt"), "delivery\n").unwrap();
        git_cmd(&root, &["commit", "-am", "delivery"]);
        let cas = init_cas_dir(&root).unwrap();
        let agents = open_agent_store(&cas).unwrap();
        let mut supervisor = Agent::new("supervisor".into(), "supervisor".into());
        supervisor.role = AgentRole::Supervisor;
        agents.register(&supervisor).unwrap();
        for axis in ["spec-child", "standards-child", "unbound-child"] {
            agents
                .register(&Agent::new_sub_agent(
                    axis.into(),
                    axis.into(),
                    "supervisor".into(),
                ))
                .unwrap();
        }
        let mut worker = Agent::new("worker".into(), "worker".into());
        worker.role = AgentRole::Worker;
        worker.agent_type = AgentType::Worker;
        agents.register(&worker).unwrap();
        let mut task = Task::new("cas-shadow".into(), "Shadow fixture".into());
        task.description = "Change the delivery value".into();
        task.acceptance_criteria = "- value changes\n- other behavior stays".into();
        open_task_store(&cas).unwrap().add(&task).unwrap();
        let proof = capture_repository_proof_with_anchors(&root, &root, vec![]).unwrap();
        let boundary = VerificationProofBoundary {
            repository: Some(proof),
            ..VerificationProofBoundary::task()
        };
        let dispatch = cas_store::create_verification_dispatch_bound(
            &cas,
            &task.id,
            "worker",
            "worker",
            &boundary,
            chrono::Utc::now() + chrono::Duration::minutes(30),
            false,
        )
        .unwrap();
        Self {
            supervisor: core(&cas, "supervisor"),
            spec: core(&cas, "spec-child"),
            standards: core(&cas, "standards-child"),
            worker: core(&cas, "worker"),
            _temp: temp,
            root,
            cas,
            base,
            dispatch: dispatch.id,
        }
    }
    fn start_request(&self) -> Value {
        json!({"op":"start","task_id":"cas-shadow","dispatch_id":self.dispatch,"base_ref":self.base,"spec_agent_id":"spec-child","standards_agent_id":"standards-child"})
    }
    fn call(core: &CasCore, payload: Value) -> ReviewResult<Value> {
        core.shadow_review_inner(Some(&payload.to_string()))
    }
    fn start(&self) -> Value {
        Self::call(&self.supervisor, self.start_request()).unwrap()
    }
    fn id(&self) -> String {
        format!("shadow-{}", self.dispatch)
    }
    fn context(&self, core: &CasCore) -> Value {
        Self::call(core, json!({"op":"context","round_id":self.id()})).unwrap()
    }
    fn report(&self, axis: Axis, commit: Option<&str>) -> Value {
        let criteria = if axis == Axis::Spec {
            json!([
                {"criterion":"- value changes","status":"approved","evidence":"value.txt contains delivery"},
                {"criterion":"- other behavior stays","status":"approved","evidence":"only the requested file changed"}
            ])
        } else {
            json!([])
        };
        let findings = match commit {
            Some(commit) => {
                json!([{"id":"f1","rank":1,"source":if axis == Axis::Spec { "- value changes" } else { "CODING_STANDARDS.md" },"evidence":"a concrete certain finding","uncertain":false,"judgement":axis == Axis::Standards,"commit":commit}])
            }
            None => json!([]),
        };
        json!({"op":"report","round_id":self.id(),"report":{"axis":axis,"status":"approved","summary":"reviewed fixed diff","criteria":criteria,"scope_creep":[],"findings":findings}})
    }
    fn fix(&self, axis: Axis, file: &str, content: &str) -> String {
        let ctx = self.context(if axis == Axis::Spec {
            &self.spec
        } else {
            &self.standards
        });
        let path = Path::new(ctx["worktree"].as_str().unwrap());
        std::fs::write(path.join(file), content).unwrap();
        git_cmd(path, &["add", file]);
        git_cmd(
            path,
            &[
                "commit",
                "-m",
                &format!("review({}): f1 fix evidence", axis.name()),
            ],
        );
        git::head(path).unwrap()
    }
    fn check(&self, core: &CasCore, commit: &str, decision: &str) -> ReviewResult<Value> {
        Self::call(
            core,
            json!({"op":"cross_check","round_id":self.id(),"commit":commit,"decision":decision,"reason":"independently inspected the fix and its effect"}),
        )
    }
}

#[test]
fn sealed_axes_hide_other_sources_and_reject_unbound_identity() {
    let f = Fixture::new();
    assert!(Fixture::call(&f.worker, f.start_request()).is_err());
    let round = f.start();
    assert_eq!(round["merge_gate_authority"], false);
    let spec = f.context(&f.spec);
    let standards = f.context(&f.standards);
    assert!(spec["sources"].get("criteria").is_some());
    assert!(spec["sources"].get("promoted_rules").is_none());
    assert!(standards["sources"].get("criteria").is_none());
    assert!(standards["sources"].get("promoted_rules").is_some());
    let unbound = core(&f.cas, "unbound-child");
    assert!(Fixture::call(&unbound, json!({"op":"context","round_id":f.id()})).is_err());
    assert!(Fixture::call(&f.spec, json!({"op":"show","round_id":f.id()})).is_err());
    assert!(
        Fixture::call(
            &f.spec,
            json!({"op":"context","round_id":f.id(),"cross_check":true})
        )
        .is_err()
    );
    let mut same = f.start_request();
    same["standards_agent_id"] = json!("spec-child");
    assert!(Fixture::call(&f.supervisor, same).is_err());
}

#[test]
fn criteria_labels_uncertainty_and_side_ref_changes_are_enforced() {
    let f = Fixture::new();
    f.start();
    let mut missing = f.report(Axis::Spec, None);
    missing["report"]["criteria"] = json!([]);
    assert!(Fixture::call(&f.spec, missing).is_err());
    assert!(Fixture::call(&f.standards, f.report(Axis::Spec, None)).is_err());
    let commit = f.fix(Axis::Spec, "spec.txt", "fix\n");
    let mut uncertain = f.report(Axis::Spec, Some(&commit));
    uncertain["report"]["findings"][0]["uncertain"] = json!(true);
    assert!(Fixture::call(&f.spec, uncertain).is_err());
    assert!(Fixture::call(&f.spec, f.report(Axis::Spec, None)).is_err());
    Fixture::call(&f.spec, f.report(Axis::Spec, Some(&commit))).unwrap();
    Fixture::call(&f.standards, f.report(Axis::Standards, None)).unwrap();
    let ctx = f.context(&f.spec);
    let path = Path::new(ctx["worktree"].as_str().unwrap());
    std::fs::write(path.join("spec.txt"), "unreported\n").unwrap();
    git_cmd(path, &["commit", "-am", "unreported change"]);
    assert!(f.check(&f.standards, &commit, "accept").is_err());
}

#[test]
fn accepted_fixes_apply_only_with_opt_in_and_legacy_gates_are_unchanged() {
    let f = Fixture::new();
    f.start();
    let store = open_verification_store(&f.cas).unwrap();
    let mut legacy = Verification::new("ver-legacy".into(), "cas-shadow".into());
    legacy.provenance = cas_types::VerificationProvenance::SupervisorDirect;
    legacy.agent_id = Some("supervisor".into());
    legacy.issuer_agent_id = Some("supervisor".into());
    legacy.dispatch_id = Some(f.dispatch.clone());
    legacy.status = VerificationStatus::Rejected;
    legacy.summary = "legacy verdict stays authoritative".into();
    store.add(&legacy).unwrap();
    let spec = f.fix(Axis::Spec, "spec.txt", "spec fix\n");
    let standards = f.fix(Axis::Standards, "standards.txt", "standards fix\n");
    Fixture::call(&f.spec, f.report(Axis::Spec, Some(&spec))).unwrap();
    Fixture::call(&f.standards, f.report(Axis::Standards, Some(&standards))).unwrap();
    assert!(f.check(&f.spec, &spec, "accept").is_err());
    let apply = json!({"op":"apply","round_id":f.id(),"opt_in":true});
    assert!(Fixture::call(&f.supervisor, apply.clone()).is_err());
    f.check(&f.standards, &spec, "accept").unwrap();
    f.check(&f.spec, &standards, "accept").unwrap();
    assert!(!f.root.join("spec.txt").exists());
    assert!(Fixture::call(&f.worker, apply.clone()).is_err());
    assert!(
        Fixture::call(
            &f.supervisor,
            json!({"op":"apply","round_id":f.id(),"opt_in":false})
        )
        .is_err()
    );
    let result = Fixture::call(&f.supervisor, apply.clone()).unwrap();
    assert_eq!(result["legacy_verdict"]["status"], "rejected");
    assert_eq!(result["round"]["spec"]["report"]["status"], "approved");
    assert!(f.root.join("spec.txt").exists());
    assert!(f.root.join("standards.txt").exists());
    assert_eq!(
        store.get_latest_for_task("cas-shadow").unwrap().unwrap().id,
        "ver-legacy"
    );
    assert_eq!(store.get_for_task("cas-shadow").unwrap().len(), 1);
    let head = git::head(&f.root).unwrap();
    Fixture::call(&f.supervisor, apply).unwrap();
    assert_eq!(
        git::head(&f.root).unwrap(),
        head,
        "replayed application is a no-op"
    );
    let proof: RepositoryProofBoundary =
        serde_json::from_value(result["round"]["application"]["repository"].clone()).unwrap();
    verify_repository_proof(&proof).unwrap();
}

#[test]
fn opposite_axis_revert_has_commit_reason_and_excludes_fix_from_apply() {
    let f = Fixture::new();
    f.start();
    let commit = f.fix(Axis::Spec, "spec.txt", "bad fix\n");
    Fixture::call(&f.spec, f.report(Axis::Spec, Some(&commit))).unwrap();
    Fixture::call(&f.standards, f.report(Axis::Standards, None)).unwrap();
    let receipts = f.check(&f.standards, &commit, "revert").unwrap();
    assert_eq!(receipts[0]["axis"], "standards");
    assert_eq!(receipts[0]["state"], "complete");
    let ctx = f.context(&f.spec);
    let path = Path::new(ctx["worktree"].as_str().unwrap());
    assert!(!path.join("spec.txt").exists());
    let subject = git_cmd(
        path,
        &[
            "show",
            "-s",
            "--format=%B",
            receipts[0]["revert_commit"].as_str().unwrap(),
        ],
    );
    assert!(subject.starts_with("review(standards): f1 revert"));
    assert!(subject.contains("Shadow-Reviewer: standards-child"));
    Fixture::call(
        &f.supervisor,
        json!({"op":"apply","round_id":f.id(),"opt_in":true}),
    )
    .unwrap();
    assert!(!f.root.join("spec.txt").exists());
}

#[test]
fn conflicting_fixes_abort_application_and_keep_a_durable_failure() {
    let f = Fixture::new();
    f.start();
    let spec = f.fix(Axis::Spec, "value.txt", "spec replacement\n");
    let standards = f.fix(Axis::Standards, "value.txt", "standards replacement\n");
    Fixture::call(&f.spec, f.report(Axis::Spec, Some(&spec))).unwrap();
    Fixture::call(&f.standards, f.report(Axis::Standards, Some(&standards))).unwrap();
    f.check(&f.standards, &spec, "accept").unwrap();
    f.check(&f.spec, &standards, "accept").unwrap();
    let before = git::head(&f.root).unwrap();
    assert!(
        Fixture::call(
            &f.supervisor,
            json!({"op":"apply","round_id":f.id(),"opt_in":true})
        )
        .is_err()
    );
    assert_eq!(git::head(&f.root).unwrap(), before);
    git::clean(&f.root).unwrap();
    let result = Fixture::call(&f.supervisor, json!({"op":"show","round_id":f.id()})).unwrap();
    assert_eq!(result["round"]["application"]["state"], "failed");
}

#[test]
fn changed_delivery_and_malformed_operations_do_not_mutate_round() {
    let f = Fixture::new();
    f.start();
    assert!(
        Fixture::call(
            &f.spec,
            json!({"op":"context","round_id":f.id(),"caller_id":"standards-child"})
        )
        .is_err()
    );
    assert!(Fixture::call(&f.spec, json!({"op":"context","round_id":"../escape"})).is_err());
    std::fs::write(f.root.join("value.txt"), "dirty\n").unwrap();
    assert!(Fixture::call(&f.spec, f.report(Axis::Spec, None)).is_err());
    assert!(
        Fixture::call(
            &f.supervisor,
            json!({"op":"apply","round_id":f.id(),"opt_in":true})
        )
        .is_err()
    );
    let result = Fixture::call(&f.supervisor, json!({"op":"show","round_id":f.id()})).unwrap();
    assert!(result["round"]["spec"]["report"].is_null());
    assert!(result["round"]["application"].is_null());
}

#[test]
fn fix_labels_and_standards_sources_are_not_self_asserted() {
    let f = Fixture::new();
    f.start();
    let fix = f.fix(Axis::Standards, "standards.txt", "standards\n");
    let mut bad = f.report(Axis::Standards, Some(&fix));
    bad["report"]["findings"][0]["source"] = json!("invented-rule");
    assert!(Fixture::call(&f.standards, bad).is_err());
    let ctx = f.context(&f.standards);
    let path = Path::new(ctx["worktree"].as_str().unwrap());
    git_cmd(path, &["commit", "--amend", "-m", "unlabelled finding"]);
    let bad_sha = git::head(path).unwrap();
    assert!(Fixture::call(&f.standards, f.report(Axis::Standards, Some(&bad_sha))).is_err());
    let comparison = Fixture::call(&f.supervisor, json!({"op":"show","round_id":f.id()})).unwrap();
    assert!(comparison["round"]["standards"]["report"].is_null());
}

#[test]
fn report_and_cross_check_replays_are_immutable_and_revert_intent_is_durable() {
    let f = Fixture::new();
    f.start();
    let fix = f.fix(Axis::Spec, "spec.txt", "fix\n");
    let report = f.report(Axis::Spec, Some(&fix));
    Fixture::call(&f.spec, report.clone()).unwrap();
    Fixture::call(&f.standards, f.report(Axis::Standards, None)).unwrap();
    f.check(&f.standards, &fix, "revert").unwrap();
    // The original report remains replayable after a legitimate cross-axis revert.
    Fixture::call(&f.spec, report).unwrap();
    f.check(&f.standards, &fix, "revert").unwrap();
    assert!(f.check(&f.standards, &fix, "accept").is_err());
    let store = persistence::Store::lock(&f.cas).unwrap();
    let mut round = store.load(&f.id()).unwrap();
    round.cross_checks[0].state = CrossCheckState::Intent;
    store.save(&round).unwrap();
    drop(store);
    assert!(f.check(&f.standards, &fix, "revert").is_err());
    assert!(
        Fixture::call(
            &f.supervisor,
            json!({"op":"apply","round_id":f.id(),"opt_in":true})
        )
        .is_err()
    );
}

#[test]
fn switched_delivery_branch_cannot_receive_opted_in_fixes() {
    let f = Fixture::new();
    f.start();
    let fix = f.fix(Axis::Spec, "spec.txt", "fix\n");
    Fixture::call(&f.spec, f.report(Axis::Spec, Some(&fix))).unwrap();
    Fixture::call(&f.standards, f.report(Axis::Standards, None)).unwrap();
    f.check(&f.standards, &fix, "accept").unwrap();
    git_cmd(&f.root, &["switch", "-c", "other-delivery"]);
    assert!(
        Fixture::call(
            &f.supervisor,
            json!({"op":"apply","round_id":f.id(),"opt_in":true})
        )
        .is_err()
    );
    assert!(!f.root.join("spec.txt").exists());
    let comparison = Fixture::call(&f.supervisor, json!({"op":"show","round_id":f.id()})).unwrap();
    assert!(comparison["round"]["application"].is_null());
}
