//! Temporary measurement (cas-0a27): time the epic close gate per child on
//! a mirror of this repository and the released v41 epic's children. Reads
//! scratch inputs and returns at once when they are absent.
use super::*;

#[test]
fn cas_0a27_measure_released_epic_close_gate() {
    let scratch = std::path::Path::new("/mnt/rewind/cas/scratch/cas-0a27");
    let repo = scratch.join("mirror.git");
    let Ok(raw) = std::fs::read_to_string(scratch.join("children.json")) else {
        return;
    };
    let rows: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    let children: Vec<Task> = rows
        .iter()
        .map(|row| {
            let mut task = Task::new(
                row["id"].as_str().unwrap().into(),
                row["title"].as_str().unwrap().into(),
            );
            task.status = row["status"].as_str().unwrap().parse().unwrap();
            task.assignee = row["assignee"].as_str().filter(|a| !a.is_empty()).map(str::to_string);
            task.deliverables =
                serde_json::from_str(row["deliverables"].as_str().unwrap()).unwrap();
            task
        })
        .collect();
    let epic = "epic/burn-down-v41-every-open-richards-llc-cassy-issue-cas-a6dc";
    let cache = repo.join("cas").join("epic-close-verdicts.json");
    let mut report = String::new();

    let _ = std::fs::remove_file(&cache);
    let started = std::time::Instant::now();
    let gate = collect_epic_branch_statuses_with_options(
        &children,
        epic,
        &repo,
        EpicStatusOptions { offset: 0, limit: None, summary: false, budget: EPIC_CLOSE_GATE_BUDGET },
    );
    report.push_str(&format!(
        "budgeted gate: {} of {} checked, exhausted={}, blocking={}, {:?}\n",
        gate.statuses.len(),
        children.len(),
        gate.budget_exhausted,
        gate.statuses.iter().filter(|s| s.blocks_epic_close()).count(),
        started.elapsed()
    ));

    let _ = std::fs::remove_file(&cache);
    let mut total = std::time::Duration::ZERO;
    for child in &children {
        let started = std::time::Instant::now();
        let one = collect_epic_branch_statuses_with_options(
            std::slice::from_ref(child),
            epic,
            &repo,
            EpicStatusOptions::full_unbounded(),
        );
        let elapsed = started.elapsed();
        total += elapsed;
        let status = one.statuses.first();
        report.push_str(&format!(
            "{} {:>8.1} ms blocks={} unmerged={} note={}\n",
            child.id,
            elapsed.as_secs_f64() * 1000.0,
            status.is_some_and(|s| s.blocks_epic_close()),
            status.map(|s| s.unmerged_count).unwrap_or_default(),
            status
                .and_then(|s| s.merge_evidence_note.as_deref().or(s.content_evolution_note.as_deref()))
                .map(|n| n.chars().take(90).collect::<String>())
                .unwrap_or_default(),
        ));
    }
    report.push_str(&format!("per-child total {total:?}\n"));

    let _ = std::fs::remove_file(&cache);
    let started = std::time::Instant::now();
    let unbounded = collect_epic_branch_statuses_with_options(
        &children,
        epic,
        &repo,
        EpicStatusOptions::full_unbounded(),
    );
    report.push_str(&format!(
        "unbounded all-41 collection: {} checked in {:?}\n",
        unbounded.statuses.len(),
        started.elapsed()
    ));

    let _ = std::fs::remove_file(&cache);
    let mut total = std::time::Duration::ZERO;
    for child in &children {
        let started = std::time::Instant::now();
        let _one = collect_epic_branch_statuses_with_options(
            std::slice::from_ref(child),
            epic,
            &repo,
            EpicStatusOptions { offset: 0, limit: None, summary: false, budget: EPIC_CLOSE_GATE_BUDGET },
        );
        let elapsed = started.elapsed();
        total += elapsed;
        report.push_str(&format!("bounded {} {:>8.1} ms\n", child.id, elapsed.as_secs_f64() * 1000.0));
    }
    report.push_str(&format!("bounded per-child total {total:?}\n"));
    report.push_str(&fixture_timings());
    std::fs::write(scratch.join("timings-after.txt"), report).unwrap();
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The 40-child fixture of the gate test, released and not, timed under
/// the close gate's budget.
fn fixture_timings() -> String {
    let mut report = String::new();
    for released in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@t"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        let lane = "epic/released-cas-0a27";
        git(p, &["branch", lane, "main"]);
        for worker in 0..8 {
            git(p, &["branch", &format!("factory/w{worker}"), lane]);
        }
        let mut children = Vec::new();
        for index in 0..40 {
            let worker = format!("w{}", index % 8);
            let branch = format!("factory/{worker}");
            git(p, &["checkout", "-q", &branch]);
            let file = format!("child-{index}.rs");
            std::fs::write(p.join(&file), format!("// child {index}\n")).unwrap();
            git(p, &["add", &file]);
            git(p, &["commit", "-q", "-m", "feat"]);
            let anchor = git(p, &["rev-parse", "HEAD"]);
            git(p, &["checkout", "-q", lane]);
            git(p, &["merge", "-q", "--no-ff", "-m", "merge", &branch]);
            let mut task = Task::new(format!("cas-rel-{index}"), "child".into());
            task.status = TaskStatus::Closed;
            task.assignee = Some(worker.clone());
            task.deliverables.parked_branch = Some(branch.clone());
            task.deliverables.factory_branch_anchor = Some(anchor);
            children.push(task);
        }
        for worker in 0..8 {
            git(p, &["checkout", "-q", &format!("factory/w{worker}")]);
            let file = format!("later-{worker}.rs");
            std::fs::write(p.join(&file), "// later\n").unwrap();
            git(p, &["add", &file]);
            git(p, &["commit", "-q", "-m", "later"]);
        }
        git(p, &["checkout", "-q", "main"]);
        if released {
            git(p, &["merge", "-q", "--no-ff", "-m", "release", lane]);
        }
        let started = std::time::Instant::now();
        let gate = collect_epic_branch_statuses_with_options(
            &children,
            lane,
            p,
            EpicStatusOptions { offset: 0, limit: None, summary: false, budget: EPIC_CLOSE_GATE_BUDGET },
        );
        report.push_str(&format!(
            "40-child fixture released={released}: {} of 40 checked, exhausted={}, blocking={}, {:?}\n",
            gate.statuses.len(),
            gate.budget_exhausted,
            gate.statuses.iter().filter(|s| s.blocks_epic_close()).count(),
            started.elapsed()
        ));
    }
    report
}
