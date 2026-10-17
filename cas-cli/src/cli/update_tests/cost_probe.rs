//! Temporary real-handler measurement driver; removed after the experiment.
use super::*;
use std::collections::BTreeMap;
use std::time::{Instant, SystemTime};

fn content_stamps(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    fn walk(root: &Path, into: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
        for entry in std::fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            if matches!(entry.file_name().to_str(), Some(".git" | ".cas")) {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), into);
            } else if entry.file_type().unwrap().is_file() {
                into.insert(entry.path(), (
                    std::fs::read(entry.path()).unwrap(),
                    entry.metadata().unwrap().modified().unwrap(),
                ));
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, &mut result);
    result
}

#[test]
fn isolated_local_phase_cost_probe_cas_df15() {
    let workspace = crate::test_paths::workspace_root();
    let source = cas_6718_git(&workspace, &["rev-parse", "HEAD"]);
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let fixture = tempfile::tempdir_in(&workspace).unwrap();
    let fixture = fixture.path().canonicalize().unwrap();
    env.set("CAS_CREDENTIALS_FILE", fixture.join("no-credentials"));
    env.remove("CAS_AGENT_ROLE");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    let args = update_test_args();
    let cli = update_test_cli(true);
    let mut observations = Vec::new();
    for trial in 0..3 {
        let home = fixture.join(format!("trial-{trial}"));
        std::fs::create_dir_all(&home).unwrap();
        env.set("HOME", &home);
        env.set("XDG_CONFIG_HOME", home.join(".config"));
        let projects = home.join("projects");
        let roots: Vec<_> = (0..3).map(|index| {
            let project = cas_6718_drifted_project(&projects.join(format!("project-{index}")));
            std::fs::write(project.join(".cas/config.toml"), format!(
                "[project]\ncanonical_id = 'df15-project-{index}'\n"
            )).unwrap();
            crate::store::known_repos::register_repo(&project);
            project
        }).collect();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&roots[0], home.join("alias")).unwrap();
        env.set("CAS_PROJECT_ROOTS", format!("{},{}", projects.display(), home.join("alias").display()));
        env.set("CAS_ROOT", roots[0].join(".cas"));
        env.set_current_dir(&roots[0]);
        for repeat in 0..4 {
            let sweep = Instant::now();
            let start = Instant::now();
            let discovery = discover_local_projects(Some(&roots[0].join(".cas")));
            let discovery_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(discovery.projects.len(), 3, "canonical alias refreshed twice");
            let start = Instant::now();
            let user = refresh_user_level_store(&args, &cli);
            assert!(!user.failed(), "{}", user.detail());
            let user_ms = start.elapsed().as_secs_f64() * 1000.0;
            let mut phases = Vec::new();
            for project in &discovery.projects {
                let cas_root = project.join(".cas");
                assert!(!cas_root.join("cloud.json").exists());
                let before = content_stamps(project);
                let mut durations = serde_json::Map::new();
                let start = Instant::now();
                run_schema_migrations(&args, &cli, Some(&cas_root)).unwrap();
                durations.insert("migration_ms".into(), (start.elapsed().as_secs_f64()*1000.0).into());
                let start = Instant::now();
                let index = repair_project_search_index(&cas_root, false, &cli);
                durations.insert("index_ms".into(), (start.elapsed().as_secs_f64()*1000.0).into());
                assert!(!index.failed());
                let start = Instant::now();
                let skills = refresh_project_files(&cli, &cas_root, false);
                durations.insert("skills_ms".into(), (start.elapsed().as_secs_f64()*1000.0).into());
                assert!(!skills.failed(), "{}", skills.detail());
                let start = Instant::now();
                let membership = refresh_project_membership(&cas_root, false);
                durations.insert("membership_ms".into(), (start.elapsed().as_secs_f64()*1000.0).into());
                assert!(matches!(membership, ProjectPhase::Skipped(_)));
                let start = Instant::now();
                let (cloud, _) = sync_project_cloud(&cas_root, false, &cli);
                durations.insert("cloud_ms".into(), (start.elapsed().as_secs_f64()*1000.0).into());
                assert!(matches!(cloud, ProjectPhase::Skipped(_)));
                let after = content_stamps(project);
                let rewrites: Vec<_> = before.iter().filter_map(|(path, (bytes, stamp))| {
                    after.get(path).filter(|(new, updated)| new == bytes && updated != stamp)
                        .map(|_| path.strip_prefix(project).unwrap().display().to_string())
                }).collect();
                durations.insert("unchanged_rewrites".into(), serde_json::json!(rewrites));
                phases.push(durations);
            }
            observations.push(serde_json::json!({
                "trial":trial,"repeat":repeat,"discovery_ms":discovery_ms,"user_ms":user_ms,
                "handler_sweep_ms":sweep.elapsed().as_secs_f64()*1000.0,"projects":phases,
            }));
        }
    }
    let output = serde_json::json!({
        "source":source.trim(),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "debug":cfg!(debug_assertions),"workload":"3 unlinked Git/CAS stores + canonical alias; 3 fresh trials, cold + 3 repeats; actual phase handlers; fixture setup excluded; no network",
        "observations":observations,
    });
    std::fs::create_dir_all(workspace.join("target")).unwrap();
    std::fs::write(workspace.join("target/cas-df15-costs.json"), serde_json::to_vec_pretty(&output).unwrap()).unwrap();
}
