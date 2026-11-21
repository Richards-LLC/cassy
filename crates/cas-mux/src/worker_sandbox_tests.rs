use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} in {}", dir.display());
}

/// A throwaway primary checkout plus one linked worktree on `factory/w`.
fn linked_worktree(root: &Path) -> (PathBuf, PathBuf) {
    let main = root.join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let worktree = root.join("wt");
    git(&main, &["worktree", "add", "-q", "-b", "factory/w", worktree.to_str().unwrap()]);
    (main.join(".git").canonicalize().unwrap(), worktree.canonicalize().unwrap())
}

fn allow_list(sandbox: &Value) -> Vec<PathBuf> {
    sandbox["filesystem"]["allowWrite"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| PathBuf::from(path.as_str().unwrap()))
        .collect()
}

/// cas-ac77 incident (9:57 AM ET Nov 17): an allowlisted common `.git`
/// re-opened `.git/config` and `.git/hooks` inside the sandbox. For a linked
/// worktree no generated write path contains the shared `.git` paths; for a
/// primary checkout, whose directory holds `.git`, they are explicitly denied
/// (a deny outranks an allow).
#[test]
fn generated_profile_never_allowlists_the_shared_git_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let (common, worktree) = linked_worktree(tmp.path());
    let primary = common.parent().unwrap().to_path_buf();
    for checkout in [&worktree, &primary] {
        let git = git_paths(checkout).expect("git checkout resolves");
        let mut sandbox = claude_sandbox_block(&ProfileRoots {
            write_roots: &[],
            cas_root: &tmp.path().join("cas"),
        });
        apply_worktree(&mut sandbox, checkout, Some(&git));
        let allow = allow_list(&sandbox);
        assert!(!allow.contains(&common), "the shared .git is allowlisted for {}", checkout.display());
        let deny: Vec<PathBuf> = sandbox["filesystem"]["denyWrite"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| PathBuf::from(path.as_str().unwrap()))
            .collect();
        let linked = checkout == &worktree;
        for (protected, always) in [
            (common.join("config"), true),
            (common.join("hooks/pre-push"), true),
            (common.join("refs/heads/main"), linked),
        ] {
            if !always {
                continue;
            }
            let writable = allow.iter().any(|path| protected.starts_with(path))
                && !deny.iter().any(|path| protected.starts_with(path));
            assert!(!writable, "{} is writable for {}", protected.display(), checkout.display());
        }
    }
}

fn which(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool}")]).output().is_ok_and(|out| out.status.success())
}

fn bash_version() -> (u32, u32) {
    let out = Command::new("bash").args(["-c", "echo ${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split_whitespace().map(|part| part.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

struct Box_ {
    srt: PathBuf,
    settings: PathBuf,
    worktree: PathBuf,
    outside: PathBuf,
}

impl Box_ {
    fn run(&self, script: &str, target: &Path, sandboxed: bool) -> (bool, String) {
        std::fs::write(self.outside.join("victim"), "original\n").unwrap();
        let rel = |from: &Path| pathdiff(target, from);
        let mut command = if sandboxed {
            let mut command = Command::new(&self.srt);
            let quoted = format!("bash -c {}", shell_quote(script));
            command.args(["--settings", self.settings.to_str().unwrap(), "-c", &quoted]);
            command
        } else {
            let mut command = Command::new("bash");
            command.args(["-c", script]);
            command
        };
        let out = command
            .current_dir(&self.worktree)
            .env("O", target)
            .env("OUT", &self.outside)
            .env("REL_IN", rel(&self.worktree.join("in")))
            .env("REL_W", rel(&self.worktree))
            .output()
            .expect("probe runs");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let victim = std::fs::read_to_string(self.outside.join("victim")).unwrap_or_default();
        let hit = target.symlink_metadata().is_ok() || victim != "original\n" || out.stdout.windows(7).any(|w| w == b"PLANTED");
        (hit, text)
    }

    fn reset(&self, target: &Path) {
        let _ = std::fs::remove_file(target);
        let _ = std::fs::remove_dir_all(target);
        for name in ["in", "gr", "a\u{a0}", ".bashrc", "hl", "s.sh", "step.sh", "c7.py", "mv.tmp", "cp.tmp", "x"] {
            let path = self.worktree.join(name);
            let _ = std::fs::remove_dir_all(&path);
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn pathdiff(target: &Path, from: &Path) -> String {
    let target: Vec<_> = target.components().collect();
    let from: Vec<_> = from.components().collect();
    let common = target.iter().zip(&from).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = from[common..].iter().map(|_| "..".to_string()).collect();
    parts.extend(target[common..].iter().map(|part| part.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

/// The sandbox-runtime binary Claude Code's sandbox is built on. Tests that
/// need it skip with a reason when it is absent; set `CAS_SRT_BIN`.
fn srt() -> Option<PathBuf> {
    std::env::var_os("CAS_SRT_BIN").map(PathBuf::from).filter(|path| path.is_file())
}

/// The cas-ac77 bypass corpus, run through the generated worker profile: every
/// probe writes outside the roots unsandboxed (so the probe is live) and is
/// refused by the OS inside the sandbox. The git rows check the exact path
/// rules: a worker commits, branches `factory/*` and fetches, and cannot write
/// the shared git config, hooks, `main`, or a protected secret file.
#[test]
fn bypass_corpus_is_blocked_through_the_generated_profile() {
    if !cfg!(target_os = "macos") {
        eprintln!("SKIP: sandbox-runtime does not start on the Linux factory host yet (cas-ac77)");
        return;
    }
    let Some(srt) = srt() else {
        eprintln!("SKIP: set CAS_SRT_BIN to a sandbox-runtime `srt` binary to run the corpus");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (common, worktree) = linked_worktree(&root);
    let remote = root.join("remote.git");
    git(&root, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(&worktree, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&worktree, &["push", "-q", "origin", "HEAD:main"]);
    let (outside, artifacts, scratch) = (root.join("outside"), root.join("artifacts"), root.join("scratch"));
    for dir in [&outside, &artifacts, &scratch] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let git_layout = git_paths(&worktree).unwrap();
    for dir in &git_layout.create {
        std::fs::create_dir_all(dir).unwrap();
    }
    let mut sandbox = claude_sandbox_block(&ProfileRoots {
        write_roots: &[artifacts.clone(), scratch.clone()],
        cas_root: &root.join("cas"),
    });
    apply_worktree(&mut sandbox, &worktree, Some(&git_layout));
    let settings = root.join("srt-settings.json");
    std::fs::write(&settings, srt_settings(&sandbox).to_string()).unwrap();
    let sandbox_box = Box_ { srt, settings, worktree: worktree.clone(), outside: outside.clone() };

    let corpus: Value = serde_json::from_str(include_str!("worker_sandbox_corpus.json")).unwrap();
    let bash = bash_version();
    let mut escaped = Vec::new();
    let mut ran = 0;
    for probe in corpus["probes"].as_array().unwrap() {
        let id = probe["id"].as_str().unwrap();
        if probe["needs"].as_str().is_some_and(|tool| !which(tool)) {
            eprintln!("skip {id}: needs {}", probe["needs"]);
            continue;
        }
        if let Some(min) = probe["min_bash"].as_array() {
            let min = (min[0].as_u64().unwrap() as u32, min[1].as_u64().unwrap() as u32);
            if bash < min {
                eprintln!("skip {id}: needs bash {min:?}");
                continue;
            }
        }
        let script = probe["script"].as_str().unwrap();
        let target = outside.join(id);
        sandbox_box.reset(&target);
        let (control, text) = sandbox_box.run(script, &target, false);
        assert!(control, "{id}: probe is not live unsandboxed: {text}");
        sandbox_box.reset(&target);
        let (hit, text) = sandbox_box.run(script, &target, true);
        if hit {
            escaped.push(format!("{id}: {text}"));
        }
        sandbox_box.reset(&target);
        ran += 1;
    }
    assert!(ran >= 40, "only {ran} corpus probes ran");
    assert!(escaped.is_empty(), "escaped the sandbox: {escaped:#?}");

    let git_rows = format!(
        "git -c user.name=t -c user.email=t@example.invalid commit -q --allow-empty -m s && echo commit=ok
git branch -q factory/extra && echo factory_branch=ok
git fetch -q origin && echo fetch=ok
git update-ref refs/heads/main HEAD 2>/dev/null && echo main=WRITTEN
echo '[core]' >> {config} 2>/dev/null && echo config=WRITTEN
echo x > {hooks}/pre-push 2>/dev/null && echo hooks=WRITTEN
echo S=1 > .env 2>/dev/null && echo env=WRITTEN
echo k > id_ed25519 2>/dev/null && echo key=WRITTEN
true",
        config = common.join("config").display(),
        hooks = common.join("hooks").display(),
    );
    let (_, text) = sandbox_box.run(&git_rows, &outside.join("git-rows"), true);
    for expected in ["commit=ok", "factory_branch=ok", "fetch=ok"] {
        assert!(text.contains(expected), "{expected} missing: {text}");
    }
    assert!(!text.contains("WRITTEN"), "a protected git or secret path was written: {text}");
}
