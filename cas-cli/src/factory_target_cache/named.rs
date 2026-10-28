//! Exact-path inspection must not depend on unrelated cache inventory.
use super::*;

pub(super) fn inspect(
    cas_root: &Path,
    policy: TargetCachePolicy,
    _path: &Path,
    live: &[PathBuf],
    dry_run: bool,
) -> io::Result<TargetCacheReport> {
    super::inspect(cas_root, policy, &[], live, dry_run)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn exact_retired_target_survives_global_budget_exhaustion() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let repo = env.home().canonicalize().unwrap().join("repo");
        let cas = crate::store::init_cas_dir(&repo).unwrap();
        let scratch = env.home().canonicalize().unwrap().join("scratch");
        fs::write(cas.join("config.toml"), format!("[factory]\nscratch_root = {:?}\n", scratch)).unwrap();
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
        agent.metadata.insert("clone_path".into(), worker.display().to_string());
        crate::store::open_agent_store(&cas).unwrap().register(&agent).unwrap();
        let mut task = cas_types::Task::new("cas-done".into(), "finished work".into());
        task.assignee = Some(agent.id);
        task.status = cas_types::TaskStatus::Closed;
        crate::store::open_task_store(&cas).unwrap().add(&task).unwrap();
        let old = filetime::FileTime::from_system_time(SystemTime::now() - Duration::from_secs(7200));
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
        let global = inspect_until(&cas, policy, &[], &[], Instant::now()).unwrap();
        assert!(!global.inventory_complete);
        assert!(global.caches.iter().any(|cache| cache.path == target
            && cache.disposition != CacheDisposition::Selected));
        let mut report = inspect_named(&cas, policy, &target, &[], false).unwrap();
        cleanup_selected_with_opt_in(&cas, &mut report, policy, &[], Some(&target)).unwrap();
        assert!(!target.exists(), "exact retired target was retained: {:?}", report.caches);
        assert_eq!(report.caches.len(), 1, "named inspection includes no unrelated caches");
        assert!(report.inventory_complete);
        assert!(report.reclaimed_bytes >= b"regenerable".len() as u64);
        assert_eq!(fs::read(worker.join("source.rs")).unwrap(), b"keep source");
        assert_eq!(fs::read(unrelated.join("proof.txt")).unwrap(), b"keep proof");
    }
}
