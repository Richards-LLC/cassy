//! Exact-path inspection must not depend on unrelated cache inventory.
use super::*;

/// Derive only an exact worker target in this store's managed namespace.
/// No directory inventory, wildcard expansion or ownership adoption occurs.
pub(super) fn worktree(cas_root: &Path, path: &Path) -> Option<PathBuf> {
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return None;
    }
    let workers = cas_root.join("worktrees");
    if path.file_name().is_some_and(|name| name == "target") {
        let tree = path.parent()?;
        if tree.parent() == Some(workers.as_path()) {
            return Some(tree.to_path_buf());
        }
    }
    let scratch = crate::factory_scratch::configured_scratch_root(cas_root)?;
    let targets =
        cas_types::factory_scratch::project_scratch_dir(&scratch, cas_root).join("targets");
    (path.parent() == Some(targets.as_path()))
        .then(|| workers.join(path.file_name().unwrap_or_default()))
        .filter(|tree| crate::factory_scratch::worker_target_dir(tree) == path)
}

pub(super) fn inspect(
    cas_root: &Path,
    policy: TargetCachePolicy,
    path: &Path,
    live: &[PathBuf],
    dry_run: bool,
) -> io::Result<TargetCacheReport> {
    inspect_scoped_with_probe(
        cas_root,
        policy,
        &[],
        live,
        dry_run,
        false,
        capacity_status,
        Some(Instant::now() + Duration::from_secs(8)),
        Some(path),
    )
}

pub(super) fn discover(
    cas_root: &Path,
    path: &Path,
    deadline: Option<Instant>,
) -> Vec<ScannedCache> {
    let Some(tree) = worktree(cas_root, path) else {
        return Vec::new();
    };
    let scan = scan_cache_until(&tree, path, false, deadline)
        .unwrap_or_else(|error| scan_error_record(tree, path.to_path_buf(), false, error));
    vec![scan]
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path).ok()?;
        (metadata.is_dir() && !metadata.file_type().is_symlink())
            .then(|| (metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// The legacy unknown-target option is exact-path consent, not ownership.
/// Known targets additionally need a fresh inode-bound report and terminal owner.
pub(super) fn validate_opt_in(
    cas_root: &Path,
    record: &TargetCacheRecord,
    consent: &Path,
    policy: TargetCachePolicy,
    deadline: Option<Instant>,
) -> Result<(), (CacheDisposition, String)> {
    if record.ownership == CacheOwnership::Unknown {
        return unknown::validate(cas_root, record, consent, policy);
    }
    let refuse = |reason: &str| (CacheDisposition::UnsafePath, reason.to_string());
    if record.ownership != CacheOwnership::Durable
        || record.interrupted_cleanup
        || !consent.is_absolute()
        || consent.as_os_str() != record.path.as_os_str()
        || worktree(cas_root, consent).as_ref() != Some(&record.worktree)
        || cas_root.canonicalize().ok().as_deref() != Some(cas_root)
        || consent.canonicalize().ok().as_deref() != Some(consent)
        || record.named_identity.is_none()
        || identity(consent) != record.named_identity
    {
        return Err(refuse(
            "named known target requires fresh exact managed-path and inode evidence",
        ));
    }
    if !matches!(
        record.disposition,
        CacheDisposition::Eligible | CacheDisposition::Retained | CacheDisposition::Selected
    ) {
        return Err((
            record.disposition,
            format!("named target is not eligible: {}", record.reason),
        ));
    }
    if policy.min_idle_secs == 0 {
        return Err((
            CacheDisposition::RecentWrite,
            "named reclaim requires a positive quiet window".into(),
        ));
    }
    // No warm-cache override for live, unknown or unfinished ownership. A
    // removed worker follows the existing external provenance/registry gates.
    if !legacy::terminal_dead_owner(cas_root, &record.worktree) {
        return Err((
            CacheDisposition::Indeterminate,
            "named target lacks retired terminal-owner evidence".into(),
        ));
    }
    if !consent.join(owner_marker()).is_file() {
        return Err((
            CacheDisposition::OwnershipUncertain,
            "named known target has no durable generation marker".into(),
        ));
    }
    unknown::validate_evidence(cas_root, consent)?;
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(policy.min_idle_secs))
        .unwrap_or(UNIX_EPOCH);
    for entry in WalkDir::new(consent).follow_links(false) {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err((
                CacheDisposition::Indeterminate,
                "named cache validation budget exhausted; retained".into(),
            ));
        }
        let entry = entry.map_err(|error| refuse(&error.to_string()))?;
        let metadata = entry
            .metadata()
            .map_err(|error| refuse(&error.to_string()))?;
        // Retirement preserves everything outside Cargo's build roots. A
        // narrow reclaim cannot discard those logs/receipts as build output.
        if entry.depth() == 1
            && !matches!(
                entry.file_name().to_str(),
                Some(
                    "debug"
                        | "release"
                        | "doc"
                        | "package"
                        | "tmp"
                        | ".rustc_info.json"
                        | "CACHEDIR.TAG"
                )
            )
            && entry.file_name() != owner_marker()
            && !(entry.file_type().is_dir()
                && (entry.path().join("debug/.cargo-lock").is_file()
                    || entry.path().join("release/.cargo-lock").is_file()))
        {
            return Err(refuse(
                "named cache contains non-Cargo evidence; preserve it through target retirement first",
            ));
        }
        if entry.file_type().is_symlink()
            || !unknown::owned_by_caller(&metadata)
            || matches!(
                entry.file_name().to_str(),
                Some(".git" | "cas.db" | "LEDGER.md" | ".cas-scratch-owner.json")
            )
        {
            return Err(refuse(
                "named cache contains symlinks, foreign files or durable evidence",
            ));
        }
        match metadata.modified() {
            Ok(modified) if modified <= cutoff => {}
            _ => {
                return Err((
                    CacheDisposition::RecentWrite,
                    "named cache quiet window is recent or indeterminate".into(),
                ));
            }
        }
    }
    Ok(())
}

fn owner_marker() -> &'static str {
    #[cfg(unix)]
    {
        owner::MARKER
    }
    #[cfg(not(unix))]
    {
        ".cas-worker-target-owner"
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn fixture() -> (
        crate::test_support::TestEnvGuard,
        PathBuf,
        PathBuf,
        cas_types::Task,
        TargetCachePolicy,
    ) {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let repo = env.home().canonicalize().unwrap().join("repo");
        let cas = crate::store::init_cas_dir(&repo).unwrap();
        let scratch = env.home().canonicalize().unwrap().join("scratch");
        fs::write(
            cas.join("config.toml"),
            format!("[factory]\nscratch_root = {:?}\n", scratch),
        )
        .unwrap();
        let worker = cas.join("worktrees/retired");
        fs::create_dir_all(&worker).unwrap();
        fs::write(worker.join("source.rs"), b"keep source").unwrap();
        drop(owner::acquire(&cas, &worker).unwrap().unwrap());
        let target = crate::factory_scratch::worker_target_dir(&worker);
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(target.join("debug/output"), b"regenerable").unwrap();
        fs::write(target.join("debug/.cargo-lock"), b"").unwrap();
        let mut agent = cas_types::Agent::new("retired-owner".into(), "retired".into());
        agent.pid = Some(i32::MAX as u32);
        agent.pid_starttime = Some(1);
        agent.status = cas_types::AgentStatus::Shutdown;
        agent
            .metadata
            .insert("clone_path".into(), worker.display().to_string());
        crate::store::open_agent_store(&cas)
            .unwrap()
            .register(&agent)
            .unwrap();
        let mut task = cas_types::Task::new("cas-done".into(), "finished work".into());
        task.assignee = Some(agent.id);
        task.status = cas_types::TaskStatus::Closed;
        crate::store::open_task_store(&cas)
            .unwrap()
            .add(&task)
            .unwrap();
        let old =
            filetime::FileTime::from_system_time(SystemTime::now() - Duration::from_secs(7200));
        for entry in WalkDir::new(&target).contents_first(true) {
            filetime::set_file_mtime(entry.unwrap().path(), old).unwrap();
        }
        let unrelated = worker.join("target");
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(unrelated.join("proof.txt"), b"keep proof").unwrap();
        let policy = TargetCachePolicy {
            high_watermark_percent: 100,
            low_watermark_percent: 99,
            min_idle_secs: 60,
            retention_count: 1,
        };
        (env, cas, target, task, policy)
    }

    #[test]
    fn exact_retired_target_survives_global_budget_exhaustion() {
        let (_env, cas, target, _task, policy) = fixture();
        let worker = cas.join("worktrees/retired");
        let unrelated = worker.join("target");
        let global = inspect_until(&cas, policy, &[], &[], Instant::now()).unwrap();
        assert!(!global.inventory_complete);
        assert!(
            global.caches.iter().any(
                |cache| cache.path == target && cache.disposition != CacheDisposition::Selected
            )
        );
        let mut report = inspect_named(&cas, policy, &target, &[], false).unwrap();
        cleanup_selected_with_opt_in(&cas, &mut report, policy, &[], Some(&target)).unwrap();
        assert!(
            !target.exists(),
            "exact retired target was retained: {:?}",
            report.caches
        );
        assert_eq!(
            report.caches.len(),
            1,
            "named inspection includes no unrelated caches"
        );
        assert!(report.inventory_complete);
        assert!(report.reclaimed_bytes >= b"regenerable".len() as u64);
        assert_eq!(fs::read(worker.join("source.rs")).unwrap(), b"keep source");
        assert_eq!(
            fs::read(unrelated.join("proof.txt")).unwrap(),
            b"keep proof"
        );
    }

    #[test]
    fn exact_consent_keeps_liveness_quiet_identity_and_evidence_guards() {
        let (_env, cas, target, mut task, policy) = fixture();
        let worker = cas.join("worktrees/retired");
        let output = target.join("debug/output");
        let original = fs::read(&output).unwrap();
        let preserved = |report: &TargetCacheReport| {
            assert_eq!(report.reclaimed_bytes, 0, "{report:?}");
            assert_eq!(fs::read(&output).unwrap(), original);
            assert!(
                report
                    .caches
                    .iter()
                    .all(|cache| cache.disposition != CacheDisposition::Reclaimed)
            );
        };
        let attempt = |policy, live: &[PathBuf], dry| {
            let mut report = inspect_named(&cas, policy, &target, live, dry).unwrap();
            cleanup_selected_with_opt_in(&cas, &mut report, policy, live, Some(&target)).unwrap();
            report
        };
        preserved(&attempt(policy, &[], true));
        preserved(&attempt(
            TargetCachePolicy {
                min_idle_secs: 0,
                ..policy
            },
            &[],
            false,
        ));
        preserved(&attempt(policy, std::slice::from_ref(&worker), false));
        let lease = owner::acquire(&cas, &worker).unwrap().unwrap();
        preserved(&attempt(policy, &[], false));
        drop(lease);
        let lane = crate::factory_worker_check::try_lock_lane(&cas, &worker)
            .unwrap()
            .unwrap();
        preserved(&attempt(policy, &[], false));
        drop(lane);
        let cargo = OpenOptions::new()
            .read(true)
            .write(true)
            .open(target.join("debug/.cargo-lock"))
            .unwrap();
        cargo.lock_exclusive().unwrap();
        preserved(&attempt(policy, &[], false));
        FileExt::unlock(&cargo).unwrap();
        drop(cargo);
        task.status = cas_types::TaskStatus::InProgress;
        crate::store::open_task_store(&cas)
            .unwrap()
            .update(&task)
            .unwrap();
        preserved(&attempt(policy, &[], false));
        task.status = cas_types::TaskStatus::Closed;
        crate::store::open_task_store(&cas)
            .unwrap()
            .update(&task)
            .unwrap();
        let config = fs::read(cas.join("config.toml")).unwrap();
        let config_with_evidence = format!(
            "{}artifacts_root = {:?}\n",
            String::from_utf8_lossy(&config),
            target.join("evidence")
        );
        fs::write(cas.join("config.toml"), config_with_evidence).unwrap();
        preserved(&attempt(policy, &[], false));
        fs::write(cas.join("config.toml"), config).unwrap();
        let target_stamp =
            filetime::FileTime::from_last_modification_time(&fs::metadata(&target).unwrap());
        for relative in ["worker-check.log", "nextest/junit.xml", "receipts.json"] {
            let evidence = target.join(relative);
            fs::create_dir_all(evidence.parent().unwrap()).unwrap();
            fs::write(&evidence, b"durable proof").unwrap();
            for entry in WalkDir::new(&target).contents_first(true) {
                filetime::set_file_mtime(entry.unwrap().path(), target_stamp).unwrap();
            }
            preserved(&attempt(policy, &[], false));
            assert_eq!(fs::read(&evidence).unwrap(), b"durable proof");
            fs::remove_file(&evidence).unwrap();
            if evidence.parent() != Some(target.as_path()) {
                fs::remove_dir(evidence.parent().unwrap()).unwrap();
            }
            filetime::set_file_mtime(&target, target_stamp).unwrap();
        }
        let mut report = inspect_named(&cas, policy, &target, &[], false).unwrap();
        cleanup_selected_using(
            &cas,
            &mut report,
            policy,
            &[],
            Some(&target),
            &cas.join("missing-proc"),
        )
        .unwrap();
        preserved(&report);
        let mut report = inspect_scoped_with_probe(
            &cas,
            policy,
            &[],
            &[],
            false,
            false,
            capacity_status,
            Some(Instant::now()),
            Some(&target),
        )
        .unwrap();
        cleanup_selected_with_opt_in(&cas, &mut report, policy, &[], Some(&target)).unwrap();
        preserved(&report);
        // Replacing an inode after inspection cannot inherit the selected path's authorization.
        let mut report = inspect_named(&cas, policy, &target, &[], false).unwrap();
        let saved = target.with_file_name("saved-original");
        fs::rename(&target, &saved).unwrap();
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(&output, &original).unwrap();
        fs::copy(saved.join(owner::MARKER), target.join(owner::MARKER)).unwrap();
        cleanup_selected_with_opt_in(&cas, &mut report, policy, &[], Some(&target)).unwrap();
        preserved(&report);
        assert!(saved.join("debug/output").is_file());
        fs::remove_dir_all(&target).unwrap();
        fs::rename(&saved, &target).unwrap();
        let old = fs::metadata(&output).unwrap().modified().unwrap();
        fs::write(&output, &original).unwrap(); // in-place write keeps directory stamps unchanged
        preserved(&attempt(policy, &[], false));
        filetime::set_file_mtime(&output, filetime::FileTime::from_system_time(old)).unwrap();
        let alias = target.with_file_name("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let mut report = inspect_named(&cas, policy, &alias, &[], false).unwrap();
        cleanup_selected_with_opt_in(&cas, &mut report, policy, &[], Some(&alias)).unwrap();
        preserved(&report);
    }

    #[tokio::test]
    async fn named_consent_mcp_route_is_narrow_and_dry_by_default() {
        use rmcp::handler::server::wrapper::Parameters;
        let (_env, cas, target, _task, _policy) = fixture();
        // The real request handler loads policy, rather than the unit test policy.
        let config = fs::read_to_string(cas.join("config.toml")).unwrap();
        fs::write(
            cas.join("config.toml"),
            format!("{config}target_cache_min_idle_secs = 60\n"),
        )
        .unwrap();
        let supervisor = cas_types::Agent::new_with_role(
            "supervisor".into(),
            "supervisor".into(),
            cas_types::AgentRole::Supervisor,
        );
        let agents = crate::store::open_agent_store(&cas).unwrap();
        agents.register(&supervisor).unwrap();
        let core = crate::mcp::CasCore::with_daemon(cas.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id.clone());
        let service = crate::mcp::CasService::new(core, None);
        let request = |force, dry_run| {
            serde_json::json!({"action":"gc_cleanup",
            "reclaim_unknown_target": target, "force":force, "dry_run":dry_run})
        };
        for (force, dry_run) in [(true, None), (false, Some(false))] {
            let result = service
                .factory_request(Parameters(
                    serde_json::from_value(request(force, dry_run)).unwrap(),
                ))
                .await
                .unwrap();
            assert_ne!(result.is_error, Some(true), "{result:?}");
            assert!(
                target.join("debug/output").is_file(),
                "incomplete consent reclaimed cache"
            );
        }
        let mut incompatible = request(true, Some(false));
        incompatible["artifacts"] = true.into();
        assert!(
            service
                .factory_request(Parameters(serde_json::from_value(incompatible).unwrap()))
                .await
                .is_err()
        );
        assert!(target.exists());
        let result = service
            .factory_request(Parameters(
                serde_json::from_value(request(true, Some(false))).unwrap(),
            ))
            .await
            .unwrap();
        assert_ne!(result.is_error, Some(true), "{result:?}");
        assert!(
            !target.exists(),
            "native MCP named reclaim failed: {result:?}"
        );
        assert!(cas.join("worktrees/retired/source.rs").is_file());
        assert!(cas.join("worktrees/retired/target/proof.txt").is_file());
        assert!(
            agents.get("retired-owner").is_ok(),
            "named route entered unrelated stale-agent GC"
        );
    }
}
