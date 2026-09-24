use super::*;
use std::sync::Mutex;

/// A stubbed Neon: a branch list, and a log of every mutation.
#[derive(Default)]
struct StubNeon {
    branches: Mutex<Vec<NeonBranchInfo>>,
    created: Mutex<Vec<(String, String)>>,
    deleted: Mutex<Vec<String>>,
    fail_connection: bool,
    fail_delete: bool,
}

const CONNECTION: &str = "postgresql://owner:s3cret@ep-x.neon.tech/neondb?sslmode=require";

impl StubNeon {
    fn with(branches: &[(&str, &str, bool)]) -> Self {
        Self {
            branches: Mutex::new(
                branches
                    .iter()
                    .map(|(id, name, is_default)| NeonBranchInfo {
                        id: id.to_string(),
                        name: name.to_string(),
                        is_default: *is_default,
                    })
                    .collect(),
            ),
            ..Self::default()
        }
    }
}

impl NeonBranchApi for StubNeon {
    async fn list_branches(&self, _project_id: &str) -> Result<Vec<NeonBranchInfo>, String> {
        Ok(self.branches.lock().unwrap().clone())
    }
    async fn create_branch(
        &self,
        _project_id: &str,
        name: &str,
        parent_id: &str,
    ) -> Result<NeonBranchInfo, String> {
        let branch = NeonBranchInfo {
            id: format!("br-new-{}", self.created.lock().unwrap().len() + 1),
            name: name.to_string(),
            is_default: false,
        };
        self.created
            .lock()
            .unwrap()
            .push((name.to_string(), parent_id.to_string()));
        self.branches.lock().unwrap().push(branch.clone());
        Ok(branch)
    }
    async fn connection_string(
        &self,
        _project_id: &str,
        _branch_id: &str,
    ) -> Result<String, String> {
        if self.fail_connection {
            Err(format!("upstream said no for {CONNECTION}"))
        } else {
            Ok(CONNECTION.to_string())
        }
    }
    async fn delete_branch(&self, _project_id: &str, branch_id: &str) -> Result<(), String> {
        if self.fail_delete {
            return Err("neon is down".to_string());
        }
        self.deleted.lock().unwrap().push(branch_id.to_string());
        self.branches
            .lock()
            .unwrap()
            .retain(|branch| branch.id != branch_id);
        Ok(())
    }
}

fn binding() -> NeonProjectBinding {
    NeonProjectBinding {
        project_id: "proj-1".to_string(),
        labels: vec![
            ("production".to_string(), "br-prod".to_string()),
            ("staging".to_string(), "br-staging".to_string()),
            ("dev".to_string(), "br-dev".to_string()),
        ],
        source: PathBuf::from(".claude/skills/neon-database/SKILL.md"),
    }
}

fn neon() -> StubNeon {
    StubNeon::with(&[
        ("br-prod", "main", true),
        ("br-staging", "staging", false),
        ("br-dev", "dev", false),
    ])
}

/// A cas root and a real git worktree for the env file.
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let cas_root = tmp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    let worktree = tmp.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&worktree)
        .status()
        .unwrap();
    assert!(status.success());
    (tmp, cas_root, worktree)
}

#[test]
fn names_belong_only_to_their_task() {
    assert_eq!(branch_name("cas-0033", 1), "cas-cas-0033-1");
    assert!(owns_branch_name("cas-0033", "cas-cas-0033-12"));
    assert!(!owns_branch_name("cas-0033", "cas-cas-0033-"));
    assert!(!owns_branch_name("cas-0033", "cas-cas-0033-1x"));
    assert!(!owns_branch_name("cas-0033", "cas-cas-00331-1"));
    assert!(!owns_branch_name("cas-0033", "main"));
    assert_eq!(
        next_index(
            "cas-0033",
            ["cas-cas-0033-1", "cas-cas-0033-4", "cas-cas-9-7"]
        ),
        5
    );
    assert_eq!(next_index("cas-0033", ["dev"]), 1);
}

#[test]
fn parses_branches_and_connection_strings_from_tool_text() {
    let listed = r#"Here are the branches: {"branches":[{"id":"br-prod","name":"main","default":true},{"id":"br-dev","name":"dev","primary":false}]}"#;
    let branches = parse_branches(listed);
    assert_eq!(branches.len(), 2);
    assert!(branches[0].is_default);
    assert!(!branches[1].is_default);
    // An MCP envelope nests the payload as a JSON string.
    let enveloped = serde_json::json!({"content":[{"type":"text","text":"{\"branch\":{\"id\":\"br-new\",\"name\":\"cas-cas-1-1\"}}"}]}).to_string();
    assert_eq!(parse_branches(&enveloped)[0].id, "br-new");

    let text = format!("Connection string: \"{CONNECTION}\" (keep it secret)");
    assert_eq!(
        extract_connection_string(&text).as_deref(),
        Some(CONNECTION)
    );
    let redacted = redact_connection_strings(&text);
    assert!(!redacted.contains("s3cret"), "{redacted}");
    assert!(
        redacted.contains("<connection string redacted>"),
        "{redacted}"
    );
}

#[test]
fn a_production_parent_is_refused() {
    let branches = neon().branches.into_inner().unwrap();
    for requested in ["production", "br-prod", "main"] {
        let error = resolve_parent(&binding(), Some(requested), &branches).unwrap_err();
        assert!(
            error.contains("refused") && error.contains("production"),
            "{requested}: {error}"
        );
    }
    // Neon's default branch is production even when nothing labels it.
    let unlabelled = NeonProjectBinding {
        labels: vec![],
        ..binding()
    };
    let error = resolve_parent(&unlabelled, Some("br-prod"), &branches).unwrap_err();
    assert!(error.contains("refused"), "{error}");
    // No request: dev first, then staging; never production.
    assert_eq!(
        resolve_parent(&binding(), None, &branches).unwrap().id,
        "br-dev"
    );
    let staging_only = NeonProjectBinding {
        labels: vec![
            ("production".to_string(), "br-prod".to_string()),
            ("staging".to_string(), "br-staging".to_string()),
        ],
        ..binding()
    };
    assert_eq!(
        resolve_parent(&staging_only, None, &branches).unwrap().id,
        "br-staging"
    );
    let prod_only = NeonProjectBinding {
        labels: vec![("production".to_string(), "br-prod".to_string())],
        ..binding()
    };
    assert!(
        resolve_parent(&prod_only, None, &branches)
            .unwrap_err()
            .contains("no dev or staging")
    );
    assert!(
        resolve_parent(&binding(), Some("br-missing"), &branches)
            .unwrap_err()
            .contains("not found")
    );
}

#[test]
fn project_binding_reads_the_generated_skill_file() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        project_binding(tmp.path())
            .unwrap_err()
            .contains("cas integrate neon init")
    );
    let dir = tmp.path().join(".claude/skills/neon-database");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "# Neon\n\n<!-- keep neon-ids -->\n| | Value |\n|--|--|\n| **org_id** | `org-1` |\n| **projectId** | `proj-1` |\n| **databaseName** | `neondb` |\n| **production branchId** | `br-prod` (name: `main`) |\n| **staging branchId** | `br-staging` (name: `staging`) |\n<!-- /keep neon-ids -->\n",
    )
    .unwrap();
    let binding = project_binding(tmp.path()).unwrap();
    assert_eq!(binding.project_id, "proj-1");
    assert!(
        binding
            .labels
            .contains(&("staging".to_string(), "br-staging".to_string()))
    );
    assert!(
        binding
            .labels
            .contains(&("production".to_string(), "br-prod".to_string()))
    );
}

#[tokio::test]
async fn provision_writes_a_private_git_excluded_env_file_and_records_the_branch() {
    let (_tmp, cas_root, worktree) = setup();
    let api = neon();
    let now = Utc::now();
    let record = provision(
        &api,
        &cas_root,
        &binding(),
        "cas-0033",
        &worktree,
        None,
        now,
    )
    .await
    .unwrap();
    assert_eq!(record.name, "cas-cas-0033-1");
    assert_eq!(record.parent_id, "br-dev");
    assert_eq!(record.state, DbBranchState::Live);
    assert_eq!(record.expires_at, now + Duration::hours(TTL_HOURS));
    assert_eq!(
        api.created.lock().unwrap()[0],
        ("cas-cas-0033-1".to_string(), "br-dev".to_string())
    );

    let env = worktree.join(ENV_FILE);
    assert_eq!(record.env_file, env);
    let body = std::fs::read_to_string(&env).unwrap();
    assert!(
        body.contains(&format!("DATABASE_URL={CONNECTION}\n")),
        "{body}"
    );
    assert!(
        body.contains(&format!("CAS_DB_BRANCH_ID={}\n", record.branch_id)),
        "{body}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&env).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let ignored = std::process::Command::new("git")
        .args(["check-ignore", "-q", ENV_FILE])
        .current_dir(&worktree)
        .status()
        .unwrap();
    assert!(ignored.success(), "{ENV_FILE} must be git-ignored");

    // The ledger holds the record, and never the connection string.
    assert_eq!(load(&cas_root, "cas-0033"), vec![record.clone()]);
    let ledger = std::fs::read_to_string(ledger_path(&cas_root, "cas-0033").unwrap()).unwrap();
    assert!(!ledger.contains("s3cret"), "{ledger}");
    assert!(!render_record(&record).contains("s3cret"));

    // The next one gets the next index, and the cap holds.
    let second = provision(
        &api,
        &cas_root,
        &binding(),
        "cas-0033",
        &worktree,
        Some("staging"),
        now,
    )
    .await
    .unwrap();
    assert_eq!(second.name, "cas-cas-0033-2");
    assert_eq!(second.parent_id, "br-staging");
    provision(
        &api,
        &cas_root,
        &binding(),
        "cas-0033",
        &worktree,
        None,
        now,
    )
    .await
    .unwrap();
    let capped = provision(
        &api,
        &cas_root,
        &binding(),
        "cas-0033",
        &worktree,
        None,
        now,
    )
    .await
    .unwrap_err();
    assert!(capped.contains("cap"), "{capped}");
}

#[tokio::test]
async fn provision_refuses_production_and_creates_nothing() {
    let (_tmp, cas_root, worktree) = setup();
    let api = neon();
    let error = provision(
        &api,
        &cas_root,
        &binding(),
        "cas-0033",
        &worktree,
        Some("main"),
        Utc::now(),
    )
    .await
    .unwrap_err();
    assert!(error.contains("refused"), "{error}");
    assert!(api.created.lock().unwrap().is_empty());
    assert!(!worktree.join(ENV_FILE).exists());
    assert!(load(&cas_root, "cas-0033").is_empty());
}

#[tokio::test]
async fn a_failed_env_write_deletes_the_new_branch_and_redacts_the_error() {
    let (_tmp, cas_root, worktree) = setup();
    let api = StubNeon {
        fail_connection: true,
        ..neon()
    };
    let error = provision(
        &api,
        &cas_root,
        &binding(),
        "cas-0033",
        &worktree,
        None,
        Utc::now(),
    )
    .await
    .unwrap_err();
    assert!(!error.contains("s3cret"), "{error}");
    assert!(error.contains("deleted again"), "{error}");
    assert_eq!(api.deleted.lock().unwrap().as_slice(), ["br-new-1"]);
    assert!(load(&cas_root, "cas-0033").is_empty());
}

#[tokio::test]
async fn teardown_deletes_only_the_tasks_own_branches_and_removes_the_env_file() {
    let (_tmp, cas_root, worktree) = setup();
    let api = neon();
    let now = Utc::now();
    let mine = provision(
        &api,
        &cas_root,
        &binding(),
        "cas-0033",
        &worktree,
        None,
        now,
    )
    .await
    .unwrap();
    // A record that does not carry this task's name prefix is never deleted.
    let mut records = load(&cas_root, "cas-0033");
    let mut foreign = mine.clone();
    foreign.name = "dev".to_string();
    foreign.branch_id = "br-dev".to_string();
    records.push(foreign);
    save(&cas_root, "cas-0033", &records).unwrap();

    let report = teardown(&api, &cas_root, "cas-0033", |_| true, now)
        .await
        .unwrap();
    assert_eq!(report.deleted.len(), 1);
    assert_eq!(
        api.deleted.lock().unwrap().as_slice(),
        [mine.branch_id.as_str()]
    );
    assert!(!worktree.join(ENV_FILE).exists());
    let after = load(&cas_root, "cas-0033");
    assert_eq!(after[0].state, DbBranchState::Deleted);
    assert_eq!(
        after[1].state,
        DbBranchState::Live,
        "the foreign record is untouched"
    );
    // Nothing is left to delete for this task.
    let again = teardown(
        &api,
        &cas_root,
        "cas-0033",
        |record| record.name != "dev",
        now,
    )
    .await
    .unwrap();
    assert!(again.deleted.is_empty() && again.failed.is_empty());
}

#[tokio::test]
async fn a_failed_deletion_stays_pending_and_is_retried_after_a_rest() {
    let (_tmp, cas_root, worktree) = setup();
    let now = Utc::now();
    provision(
        &neon(),
        &cas_root,
        &binding(),
        "cas-0033",
        &worktree,
        None,
        now,
    )
    .await
    .unwrap();
    let down = StubNeon {
        fail_delete: true,
        ..neon()
    };
    let report = teardown(&down, &cas_root, "cas-0033", |_| true, now)
        .await
        .unwrap();
    assert_eq!(report.failed.len(), 1);
    let record = &load(&cas_root, "cas-0033")[0];
    assert_eq!(record.state, DbBranchState::PendingDelete);
    assert!(!due_for_sweep(record, true, now + Duration::seconds(10)));
    assert!(due_for_sweep(
        record,
        true,
        now + Duration::seconds(RETRY_AFTER_SECS + 1)
    ));
    let flags = gc_flags(&load_all(&cas_root), |_| true, now);
    assert!(flags[0].contains("deletion pending"), "{flags:?}");
}

#[test]
fn a_worker_close_queues_the_deletion_for_the_supervisor() {
    let (_tmp, cas_root, worktree) = setup();
    let now = Utc::now();
    let record = DbBranchRecord {
        task_id: "cas-0033".to_string(),
        name: "cas-cas-0033-1".to_string(),
        branch_id: "br-new-1".to_string(),
        project_id: "proj-1".to_string(),
        parent_id: "br-dev".to_string(),
        parent_name: "dev".to_string(),
        worktree: worktree.clone(),
        env_file: worktree.join(ENV_FILE),
        created_at: now,
        expires_at: now + Duration::hours(TTL_HOURS),
        state: DbBranchState::Live,
        deleted_at: None,
        last_attempt_at: None,
        last_error: None,
    };
    save(&cas_root, "cas-0033", &[record.clone()]).unwrap();
    assert!(
        !due_for_sweep(&record, false, now),
        "a live branch of a live task stays"
    );
    assert!(due_for_sweep(&record, true, now), "its task ended");
    let marked = mark_pending_delete(&cas_root, "cas-0033").unwrap();
    assert_eq!(marked.len(), 1);
    let queued = &load(&cas_root, "cas-0033")[0];
    assert!(due_for_sweep(queued, false, now));
    // TTL and a vanished worktree are flagged by gc_report.
    let mut expired = record;
    expired.worktree = worktree.join("gone");
    expired.expires_at = now - Duration::hours(1);
    let flags = gc_flags(&[("cas-0033".to_string(), vec![expired])], |_| false, now);
    assert!(
        flags[0].contains("TTL") && flags[0].contains("is gone"),
        "{flags:?}"
    );
}

#[test]
fn a_worker_caller_is_refused() {
    let error = role_gate(false, "db_branch_create").unwrap_err();
    assert!(error.contains("only the supervisor"), "{error}");
    assert!(error.contains("blocker=true"), "{error}");
    assert!(role_gate(true, "db_branch_create").is_ok());
}

#[test]
fn ledger_paths_reject_path_like_task_ids() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(ledger_path(tmp.path(), "../escape").is_err());
    assert!(ledger_path(tmp.path(), "").is_err());
    assert!(ledger_path(tmp.path(), "cas-0033").is_ok());
}
