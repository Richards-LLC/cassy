//! cas-c24e: the GitHub API budget.
//!
//! Every `gh` run through `bounded_process` is counted: one line per call
//! (caller file:line, endpoint class, milliseconds, exit code) in the
//! machine's `~/.cas/logs/gh-calls-<date>.tsv`, and `cas doctor` prints the
//! last hour's top callers. One user token serves every Cassy process on both
//! machines, so a rate-limit response sets a shared backoff stamp and every
//! Cassy `gh` call on this machine fails fast until it expires, instead of
//! each caller retrying into the limit.
use std::ffi::OsStr;
use std::io::Write;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long every caller stays off GitHub after a rate-limit response.
/// GitHub asks secondary-limit clients to wait at least a minute.
pub(crate) const BACKOFF: Duration = Duration::from_secs(120);
const STAMP: &str = "gh-backoff";
const LOG_PREFIX: &str = "gh-calls-";
const LOG_RETENTION_DAYS: i64 = 7;

/// The machine's `~/.cas`: the token, and so the limit, is per user.
fn cas_home() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".cas"))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// One `gh` invocation being measured.
pub(crate) struct GhCall {
    caller: String,
    class: String,
    started: Instant,
}

impl GhCall {
    /// `None` unless `command` runs the GitHub CLI.
    pub(crate) fn start(command: &Command, caller: &Location<'_>) -> Option<Self> {
        // Unit tests run `gh` stubs; only a test that opts in is measured,
        // so no test writes the real machine's log or obeys its backoff.
        #[cfg(test)]
        if std::env::var_os("CAS_TEST_GH_BUDGET").is_none() {
            return None;
        }
        let program = Path::new(command.get_program());
        let gh = crate::github_repo::gh_binary();
        if program.file_name() != Some(OsStr::new("gh")) && program != gh {
            return None;
        }
        let file = caller.file().rsplit_once("src/").map_or(caller.file(), |(_, file)| file);
        Some(Self {
            caller: format!("{file}:{}", caller.line()),
            class: endpoint_class(&command.get_args().collect::<Vec<_>>()),
            started: Instant::now(),
        })
    }

    /// The failure a caller gets while the shared backoff holds; GitHub is
    /// not called. Reads that cost nothing (`rate_limit`, `auth`) still run.
    pub(crate) fn backoff_output(&self) -> Option<Output> {
        if self.class == "api rate_limit" || self.class.starts_with("auth ") {
            return None;
        }
        let remaining = backoff_remaining()?;
        record_line(&self.caller, &self.class, 0, "backoff");
        Some(failed_output(refusal(remaining)))
    }

    pub(crate) fn finish<E>(self, result: &Result<Output, E>) {
        let ms = self.started.elapsed().as_millis();
        let outcome = match result {
            Ok(output) if output.status.success() => "ok".to_string(),
            Ok(output) => {
                if rate_limited(output) {
                    set_backoff(BACKOFF);
                    "rate-limited".to_string()
                } else {
                    format!("exit {}", output.status.code().unwrap_or(-1))
                }
            }
            Err(_) => "error".to_string(),
        };
        record_line(&self.caller, &self.class, ms, &outcome);
    }
}

/// `Command::output` for a `gh` call site outside `bounded_process`: the
/// same budget log, backoff and process-group bound (cas-c24e).
#[track_caller]
pub(crate) fn output(command: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
    run_command(command, Deadline::after(timeout), timeout).map_err(|error| match error {
        BoundedCommandError::TimedOut => std::io::Error::new(std::io::ErrorKind::TimedOut, "gh timed out"),
        BoundedCommandError::Io => std::io::Error::new(std::io::ErrorKind::NotFound, "gh could not start"),
    })
}

/// Exit code of a call held by the backoff (EX_TEMPFAIL): never sent.
pub(crate) const BACKOFF_EXIT: i32 = 75;

fn refusal(remaining: Duration) -> String {
    format!(
        "GitHub rate-limit backoff: not sent; Cassy retries GitHub in {}s (after a rate-limit response on this machine)",
        remaining.as_secs().max(1)
    )
}

/// The backoff refusal for a failed `gh` output, so callers that hide
/// GitHub's stderr can still say why nothing was sent.
pub(crate) fn backoff_refusal(output: &Output) -> Option<String> {
    (output.status.code() == Some(BACKOFF_EXIT))
        .then(|| String::from_utf8_lossy(&output.stderr).trim().to_string())
        .filter(|text| text.starts_with("GitHub rate-limit backoff"))
}

#[cfg(unix)]
fn failed_output(stderr: String) -> Output {
    use std::os::unix::process::ExitStatusExt;
    let status = std::process::ExitStatus::from_raw(BACKOFF_EXIT << 8);
    Output { status, stdout: Vec::new(), stderr: stderr.into_bytes() }
}

#[cfg(not(unix))]
fn failed_output(stderr: String) -> Output {
    use std::os::windows::process::ExitStatusExt;
    let status = std::process::ExitStatus::from_raw(BACKOFF_EXIT as u32);
    Output { status, stdout: Vec::new(), stderr: stderr.into_bytes() }
}

/// GitHub's own refusal phrases on `gh`'s stderr: primary ("API rate limit
/// exceeded" / "already exceeded"), secondary ("secondary rate limit") and
/// GraphQL (`RATE_LIMITED`). Response bodies on stdout are never read.
fn rate_limited(output: &Output) -> bool {
    let text = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    ["api rate limit exceeded", "api rate limit already exceeded", "secondary rate limit", "rate_limited"]
        .iter()
        .any(|phrase| text.contains(phrase))
}

/// cas-c24e: one answer per `key` per `ttl` for every Cassy process on this
/// machine (the merge-queue listing). The first caller past the TTL fetches
/// under a file lock; the others read its answer. Failures are not cached.
pub(crate) fn shared_read<E>(
    key: &str,
    ttl: Duration,
    fetch: impl FnOnce() -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, E> {
    #[cfg(test)]
    if std::env::var_os("CAS_TEST_GH_BUDGET").is_none() {
        return fetch();
    }
    let Some(dir) = cas_home().map(|home| home.join("gh-cache")) else { return fetch() };
    let name: String = key.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let path = dir.join(format!("{name}.json"));
    let fresh = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age < ttl)
    };
    if fresh(&path) && let Ok(bytes) = std::fs::read(&path) {
        return Ok(bytes);
    }
    let _ = std::fs::create_dir_all(&dir);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("{name}.lock")));
    let _held = lock.ok().filter(|file| fs2::FileExt::lock_exclusive(file).is_ok());
    if fresh(&path) && let Ok(bytes) = std::fs::read(&path) {
        return Ok(bytes);
    }
    let bytes = fetch()?;
    let staging = dir.join(format!("{name}.{}", std::process::id()));
    if std::fs::write(&staging, &bytes).is_ok() {
        let _ = std::fs::rename(&staging, &path);
    }
    Ok(bytes)
}

/// A stable, id-free name for what a call asks GitHub for.
pub(crate) fn endpoint_class(args: &[&OsStr]) -> String {
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
    match words.as_slice() {
        ["api", "graphql", ..] => {
            let query = args.iter().find_map(|a| a.strip_prefix("query=")).unwrap_or("");
            format!("api graphql {}", graphql_fields(query))
        }
        ["api", rest @ ..] => {
            // The path, not an HTTP method or a `-f key=value` field.
            let path = rest
                .iter()
                .find(|a| !a.chars().all(|c| c.is_ascii_uppercase()) && (a.contains('/') || !a.contains('=')))
                .copied()
                .unwrap_or("?");
            format!("api {}", mask_path(path.split('?').next().unwrap_or(path)))
        }
        [group, verb, ..] => format!("{group} {verb}"),
        [group] => (*group).to_string(),
        [] => "gh".to_string(),
    }
}

/// The first two top-level field names of a query, e.g. `repository.pullRequests`.
fn graphql_fields(query: &str) -> String {
    let Some(body) = query.split_once('{').map(|(_, body)| body) else { return "?".into() };
    let (mut depth, mut word, mut fields) = (0, String::new(), Vec::new());
    for ch in body.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if depth == 0 && (c.is_ascii_alphanumeric() || c == '_') => {
                word.push(c);
                continue;
            }
            _ => {}
        }
        if !word.is_empty() {
            fields.push(std::mem::take(&mut word));
            if fields.len() == 2 {
                break;
            }
        }
    }
    if fields.is_empty() { "?".into() } else { fields.join(".") }
}

/// `repos/o/r/commits/<sha>/check-runs` -> `repos/:repo/commits/:id/check-runs`.
fn mask_path(path: &str) -> String {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        if parts[i] == "repos" && parts.len() > i + 2 {
            out.push("repos/:repo".to_string());
            i += 3;
            continue;
        }
        let part = parts[i];
        let id = part.chars().all(|c| c.is_ascii_digit())
            || (part.len() >= 7 && part.chars().all(|c| c.is_ascii_hexdigit()));
        out.push(if id { ":id".into() } else { part.to_string() });
        i += 1;
    }
    out.join("/")
}

pub(crate) fn backoff_remaining() -> Option<Duration> {
    let until: u64 = std::fs::read_to_string(cas_home()?.join(STAMP)).ok()?.trim().parse().ok()?;
    let now = now_secs();
    (until > now).then(|| Duration::from_secs(until - now))
}

pub(crate) fn set_backoff(duration: Duration) {
    let Some(home) = cas_home() else { return };
    let _ = std::fs::create_dir_all(&home);
    let staging = home.join(format!("{STAMP}.{}", std::process::id()));
    if std::fs::write(&staging, (now_secs() + duration.as_secs()).to_string()).is_ok() {
        let _ = std::fs::rename(&staging, home.join(STAMP));
    }
}

fn log_path(date: chrono::NaiveDate) -> Option<PathBuf> {
    Some(cas_home()?.join("logs").join(format!("{LOG_PREFIX}{date}.tsv")))
}

fn record_line(caller: &str, class: &str, ms: u128, outcome: &str) {
    let now = chrono::Utc::now();
    let Some(path) = log_path(now.date_naive()) else { return };
    let fresh = !path.exists();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        if fresh {
            sweep_old_logs(dir, now.date_naive());
        }
    }
    // One short O_APPEND write per call keeps concurrent writers' lines whole.
    let line = format!("{}\t{caller}\t{class}\t{ms}\t{outcome}\n", now.timestamp());
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn sweep_old_logs(dir: &Path, today: chrono::NaiveDate) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(date) = name.strip_prefix(LOG_PREFIX).and_then(|rest| rest.strip_suffix(".tsv")) else {
            continue;
        };
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .is_ok_and(|date| (today - date).num_days() > LOG_RETENTION_DAYS)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Calls in the last `window`, grouped by caller and class, most first.
pub(crate) struct Usage {
    pub total: usize,
    pub graphql: usize,
    pub refused: usize,
    pub top: Vec<(String, String, usize)>,
}

pub(crate) fn usage(window: Duration, top: usize) -> Usage {
    let now = chrono::Utc::now();
    let since = now.timestamp() - window.as_secs() as i64;
    let mut counts = std::collections::BTreeMap::<(String, String), usize>::new();
    let (mut total, mut graphql, mut refused) = (0, 0, 0);
    for date in [now.date_naive() - chrono::Duration::days(1), now.date_naive()] {
        let Some(text) = log_path(date).and_then(|path| std::fs::read_to_string(path).ok()) else {
            continue;
        };
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            let [at, caller, class, _, outcome] = fields.as_slice() else { continue };
            if at.parse::<i64>().map_or(true, |at| at < since) {
                continue;
            }
            if *outcome == "backoff" {
                refused += 1;
                continue;
            }
            total += 1;
            // `gh pr`, `gh repo`, `gh issue` and `gh release` read through GraphQL too.
            if class.starts_with("api graphql")
                || ["pr ", "repo ", "issue ", "release "].iter().any(|g| class.starts_with(g))
            {
                graphql += 1;
            }
            *counts.entry((caller.to_string(), class.to_string())).or_default() += 1;
        }
    }
    let mut ranked: Vec<_> = counts.into_iter().map(|((caller, class), n)| (caller, class, n)).collect();
    ranked.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(top);
    Usage { total, graphql, refused, top: ranked }
}

/// `cas doctor`'s line: last hour's calls, GraphQL share and top callers.
pub(crate) fn doctor_summary() -> (bool, String) {
    let usage = usage(Duration::from_secs(3600), 5);
    let backoff = backoff_remaining();
    let mut message = format!(
        "last hour on this machine: {} gh calls ({} GraphQL), {} held by backoff",
        usage.total, usage.graphql, usage.refused
    );
    if let Some(remaining) = backoff {
        message.push_str(&format!("; rate-limit backoff active for {}s", remaining.as_secs()));
    }
    for (caller, class, n) in &usage.top {
        message.push_str(&format!("\n  {n:>5}  {class}  ({caller})"));
    }
    (backoff.is_some(), message)
}

/// `cas factory ci-watch-dry-run`: the daemon poll's GitHub reads at the
/// daemon's cadence for `minutes`, persisting nothing; returns the calls spent.
pub(crate) fn ci_watch_dry_run(cas_dir: &Path, session: &str, minutes: u64) -> anyhow::Result<String> {
    use crate::ui::factory::daemon::runtime::ci_watch;
    let project = cas_dir.parent().ok_or_else(|| anyhow::anyhow!("project repository missing"))?;
    // The daemon watches main plus its workers' branches.
    let worktrees = Command::new("git").args(["worktree", "list", "--porcelain"]).current_dir(project).output()?;
    let mut watched: std::collections::BTreeSet<String> = String::from_utf8_lossy(&worktrees.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("branch refs/heads/"))
        .filter(|branch| branch.starts_with("factory/"))
        .map(str::to_string)
        .collect();
    watched.insert("main".into());
    let started = Instant::now();
    let window = Duration::from_secs(minutes * 60);
    let (mut queued, mut armed, mut held) = Default::default();
    let mut polls = 0;
    while started.elapsed() < window {
        let poll = ci_watch::read_github(cas_dir, project, session, &watched, &queued, &armed, &held);
        polls += 1;
        let pending = match poll {
            Ok(poll) => {
                queued = poll.queue_poll.queued_prs;
                armed = poll.queue_poll.auto_merge_prs;
                held = poll.queue_poll.held_exits;
                poll.ci_wakes.1
            }
            Err(error) => {
                eprintln!("poll {polls}: {error}");
                false
            }
        };
        std::thread::sleep(ci_watch::ci_watch_interval(pending, !queued.is_empty()));
    }
    let usage = usage(started.elapsed() + Duration::from_secs(1), 10);
    let hours = started.elapsed().as_secs_f64() / 3600.0;
    let mut report = format!(
        "{polls} polls over {:.1} min, {} branches watched: {} gh calls ({:.0}/h), {} GraphQL ({:.0}/h), {} held by backoff",
        started.elapsed().as_secs_f64() / 60.0,
        watched.len(),
        usage.total,
        usage.total as f64 / hours,
        usage.graphql,
        usage.graphql as f64 / hours,
        usage.refused
    );
    for (caller, class, n) in &usage.top {
        report.push_str(&format!("\n  {n:>5}  {:>6.0}/h  {class}  ({caller})", *n as f64 / hours));
    }
    Ok(report)
}

#[cfg(test)]
#[path = "gh_budget_tests.rs"]
mod tests;
