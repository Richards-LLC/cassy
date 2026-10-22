//! The release gate's project seam: `[qa.release]` config, the project QA
//! adapter protocol (v1) and the served-component equivalence preflight.
//!
//! The adapter is one operator-configured, project-relative executable run in
//! the [`crate::qa_command`] sandbox as `<adapter> <verb>`, with a v1 JSON
//! request on stdin and a v1 JSON response on stdout. Callers name a [`Verb`]
//! and get the response `data` or a [`CallError`]; the envelope, version and
//! sandbox stay inside this module. See `docs/qa/release-gate.md`.
use crate::qa_command::{self, CommandError, Sandbox};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub mod blocked;
pub mod fixtures;
pub mod flags;
pub mod independence;

/// Adapter protocol version written to and required from every envelope.
pub const PROTOCOL_VERSION: u32 = 1;

/// `[qa.release]` in `.cas/config.toml`. Operator-owned: deliveries cannot
/// change which command runs or which environment it sees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReleaseConfig {
    /// The branch the gate walks (for example `staging`).
    pub integration_branch: String,
    /// Promotion targets whose PRs need a clean gate (for example `main`).
    pub branches: Vec<String>,
    /// GitHub Actions deploy workflow names watched on the integration branch.
    pub deploy_workflows: Vec<String>,
    /// Project-relative adapter executable; empty leaves the gate off.
    pub adapter: String,
    /// Environment variable names allowed into the adapter.
    pub env: Vec<String>,
    /// Per-call adapter timeout.
    pub timeout_secs: u32,
    /// Served component name to the repository paths it is built from.
    pub components: BTreeMap<String, Vec<String>>,
    /// Pending-migration refusal for merges into `integration_branch`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migrations: Option<MigrationsConfig>,
}

/// `[qa.release.migrations]`: which added files are migrations, and the
/// environment whose applied state a merge into the integration branch needs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MigrationsConfig {
    /// Repository-relative patterns with exactly one `*`, which is the id.
    pub paths: Vec<String>,
    /// Environment name sent to the adapter, for example `staging`.
    pub env: String,
}

impl Default for ReleaseConfig {
    fn default() -> Self {
        Self {
            integration_branch: String::new(),
            branches: Vec::new(),
            deploy_workflows: Vec::new(),
            adapter: String::new(),
            env: Vec::new(),
            timeout_secs: 120,
            components: BTreeMap::new(),
            migrations: None,
        }
    }
}

impl ReleaseConfig {
    /// Parses and validates the `qa.release.components` JSON object.
    pub fn parse_components(value: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
        let components: BTreeMap<String, Vec<String>> = serde_json::from_str(value).map_err(
            |error| format!("qa.release.components must be a JSON object of path lists: {error}"),
        )?;
        for (name, paths) in &components {
            if name.trim().is_empty() || paths.is_empty() {
                return Err("Each qa.release.components entry needs a name and paths".into());
            }
            for path in paths {
                let escapes = Path::new(path)
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir));
                if path.trim().is_empty() || Path::new(path).is_absolute() || escapes {
                    return Err(format!(
                        "qa.release.components.{name} path {path:?} must stay inside the repository"
                    ));
                }
                // `:(...)` and `:!x` are git pathspec magic, not paths.
                if path.starts_with(':') {
                    return Err(format!(
                        "qa.release.components.{name} path {path:?} uses git pathspec magic"
                    ));
                }
            }
        }
        Ok(components)
    }

    /// Parses and validates `qa.release.migrations.paths`.
    pub fn parse_migration_paths(value: &str) -> Result<Vec<String>, String> {
        let paths: Vec<String> = value
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        for path in &paths {
            if Path::new(path).is_absolute()
                || path.matches('*').count() != 1
                || path.contains(['?', '[', ']', '\\'])
            {
                return Err(format!(
                    "qa.release.migrations.paths entry {path} must be repository-relative with exactly one `*`, which is the migration id"
                ));
            }
        }
        Ok(paths)
    }
}

/// Loads `[qa.release]` when an adapter is configured.
pub fn configured(cas_root: &Path) -> Option<ReleaseConfig> {
    crate::config::Config::load(cas_root)
        .ok()?
        .qa?
        .release
        .filter(|c| !c.adapter.trim().is_empty())
}

/// One verb per gate seam. C1 implements [`Verb::Served`]; later children add
/// typed wrappers for the others over the same [`call`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Served,
    Flags,
    Balance,
    Grant,
    FixtureSetup,
    FixtureTeardown,
    Run,
    Readback,
    Migrations,
}

impl Verb {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Served => "served",
            Self::Flags => "flags",
            Self::Balance => "balance",
            Self::Grant => "grant",
            Self::FixtureSetup => "fixture-setup",
            Self::FixtureTeardown => "fixture-teardown",
            Self::Run => "run",
            Self::Readback => "readback",
            Self::Migrations => "migrations",
        }
    }
}

/// The classification input for a failed call, reported by the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// A provider or dependency failed (5xx, provider down).
    External,
    /// Throttled (429).
    RateLimited,
    /// A consumable ran out (402).
    Insufficient,
    /// The adapter rejected the request.
    Invalid,
    /// An adapter bug.
    Internal,
}

/// `error` in an `ok: false` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterError {
    pub kind: ErrorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_s: Option<u64>,
    pub message: String,
}

/// Why a call returned no data.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// The adapter answered `ok: false` with a classified error.
    #[error("adapter {0} error ({kind:?}): {message}", kind = .1.kind, message = .1.message)]
    Adapter(&'static str, AdapterError),
    /// The call ran past `qa.release.timeout_secs`.
    #[error("adapter {0} timed out")]
    TimedOut(&'static str),
    /// Missing config, sandbox refusal, nonzero exit, oversize output or a
    /// malformed envelope: an adapter or configuration bug.
    #[error("adapter {0} failed: {1}")]
    Protocol(&'static str, String),
}

/// The fields a verb's request carries besides `v` and `verb`.
#[derive(Debug, Clone, Default)]
pub struct Request<'a> {
    /// Target environment name, for example `staging`.
    pub env: &'a str,
    /// The candidate commit.
    pub sha: &'a str,
    pub journey: Option<&'a str>,
    pub args: Value,
}

/// Calls `<adapter> <verb>` in the sandbox and returns the response `data`.
pub fn call(
    project_root: &Path,
    config: &ReleaseConfig,
    verb: Verb,
    request: Request<'_>,
) -> Result<Value, CallError> {
    let name = verb.as_str();
    let protocol = |message: String| CallError::Protocol(name, message);
    if config.adapter.trim().is_empty() {
        return Err(protocol("no qa.release.adapter configured".into()));
    }
    let mut body = serde_json::json!({
        "v": PROTOCOL_VERSION,
        "verb": name,
        "env": request.env,
        "sha": request.sha,
        "args": if request.args.is_null() { serde_json::json!({}) } else { request.args },
    });
    if let Some(journey) = request.journey {
        body["journey"] = journey.into();
    }
    let sandbox = Sandbox {
        root: project_root,
        env: &config.env,
        timeout: Duration::from_secs(u64::from(config.timeout_secs.max(1))),
    };
    let stdout = qa_command::run(
        sandbox,
        &config.adapter,
        &[name],
        Some(body.to_string().as_bytes()),
    )
    .map_err(|error| match error {
        CommandError::TimedOut(_) => CallError::TimedOut(name),
        CommandError::Failed(error) => protocol(error.to_string()),
    })?;
    #[derive(Deserialize)]
    struct Response {
        v: u32,
        ok: bool,
        #[serde(default)]
        data: Value,
        #[serde(default)]
        error: Option<AdapterError>,
    }
    // Raw stdout is never echoed: only the parse failure's position is kept.
    let response: Response = serde_json::from_slice(&stdout)
        .map_err(|error| protocol(format!("invalid response JSON at line {}", error.line())))?;
    if response.v != PROTOCOL_VERSION {
        return Err(protocol(format!(
            "unsupported protocol version {}",
            response.v
        )));
    }
    match (response.ok, response.error) {
        (true, None) => Ok(response.data),
        (false, Some(mut error)) => {
            error.message = printable(&error.message);
            error.provider = error.provider.as_deref().map(printable);
            Err(CallError::Adapter(name, error))
        }
        (true, Some(_)) => Err(protocol("ok response carries an error".into())),
        (false, None) => Err(protocol("failed response has no error".into())),
    }
}

fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// One deployed component, as the `served` verb reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServedComponent {
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    pub deploy_ref: String,
}

/// Why the served environment is not the candidate.
#[derive(Debug, thiserror::Error)]
pub enum ServedRefusal {
    /// The `served` call failed; its kind feeds row classification.
    #[error("SERVED UNKNOWN: {0}")]
    Call(#[from] CallError),
    /// One line per component that is not provably equivalent.
    #[error("SERVED MISMATCH: {}", .0.join("; "))]
    Mismatch(Vec<String>),
}

/// Asks the adapter what `env` serves and proves that every declared
/// component's served commit has the same tree as `candidate` under that
/// component's paths. Anything unprovable refuses.
pub fn preflight_served(
    project_root: &Path,
    config: &ReleaseConfig,
    env: &str,
    candidate: &str,
) -> Result<Vec<ServedComponent>, ServedRefusal> {
    let data = call(
        project_root,
        config,
        Verb::Served,
        Request {
            env,
            sha: candidate,
            ..Request::default()
        },
    )?;
    // The parse error is dropped: it could quote adapter data.
    let served: Vec<ServedComponent> = serde_json::from_value(data).map_err(|_| {
        CallError::Protocol(
            Verb::Served.as_str(),
            "served data is not a component list".into(),
        )
    })?;
    let mut problems = Vec::new();
    if config.components.is_empty() {
        problems.push("no qa.release.components are declared".into());
    }
    let mut seen = BTreeMap::new();
    for entry in &served {
        if seen.insert(entry.component.as_str(), entry).is_some() {
            problems.push(format!(
                "{} is reported twice",
                printable(&entry.component)
            ));
        } else if !config.components.contains_key(&entry.component) {
            problems.push(format!(
                "{} has no declared paths",
                printable(&entry.component)
            ));
        }
    }
    let Some(candidate_commit) = resolve_commit(project_root, candidate) else {
        problems.push(format!("candidate {} is not a local commit", printable(candidate)));
        return Err(ServedRefusal::Mismatch(problems));
    };
    for (name, paths) in &config.components {
        let Some(entry) = seen.get(name.as_str()) else {
            problems.push(format!("{name} is not reported served"));
            continue;
        };
        let Some(sha) = entry.sha.as_deref() else {
            problems.push(format!("{name} reports no served sha"));
            continue;
        };
        let Some(served_commit) = resolve_commit(project_root, sha) else {
            problems.push(format!(
                "{name} serves {}, which is not a local commit; fetch it",
                printable(sha)
            ));
            continue;
        };
        match trees_equal(project_root, &served_commit, &candidate_commit, paths) {
            Some(true) => {}
            Some(false) => problems.push(format!(
                "{name} serves {} but candidate {} differs under {}",
                short(&served_commit),
                short(&candidate_commit),
                paths.join(", ")
            )),
            None => problems.push(format!("{name}: git diff could not compare the trees")),
        }
    }
    if problems.is_empty() {
        Ok(served)
    } else {
        Err(ServedRefusal::Mismatch(problems))
    }
}

/// Reads back `journey`'s declared invariants for the given subject ids
/// through the adapter. Undeclared subjects, non-opaque ids and malformed
/// invariant IDs are refused before any call.
pub fn readback(
    project_root: &Path,
    config: &ReleaseConfig,
    env: &str,
    sha: &str,
    journey: &crate::qa_registry::Journey,
    subjects: &BTreeMap<String, String>,
) -> Result<crate::qa_readback::Outcome, String> {
    let (declaration, args) =
        crate::qa_readback::request_args(&journey.id, journey.readback.as_ref(), subjects)?;
    let answer = call(
        project_root,
        config,
        Verb::Readback,
        Request {
            env,
            sha,
            journey: Some(&journey.id),
            args,
        },
    );
    Ok(crate::qa_readback::classify(
        &journey.id,
        declaration,
        subjects,
        answer,
    ))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

const GIT_CAP: Duration = Duration::from_secs(10);

fn git(root: &Path, args: &[&str]) -> Option<std::process::Output> {
    let mut command = std::process::Command::new("git");
    command.current_dir(root).args(args);
    crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(GIT_CAP),
        GIT_CAP,
    )
    .ok()
}

fn resolve_commit(root: &Path, sha: &str) -> Option<String> {
    if !(7..=64).contains(&sha.len()) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let output = git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{sha}^{{commit}}"),
        ],
    )?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `git diff --quiet <served> <candidate> -- <paths>`: exit 0 is equal, 1 differs.
fn trees_equal(root: &Path, served: &str, candidate: &str, paths: &[String]) -> Option<bool> {
    let mut args = vec![
        "diff",
        "--quiet",
        "--no-ext-diff",
        "--no-textconv",
        served,
        candidate,
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    match git(root, &args)?.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// One migration's state in the target environment, as `migrations` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MigrationStatus {
    Applied,
    Pending,
    Unknown,
}

/// Why a merge into the integration branch must not proceed. Every variant
/// fails closed.
#[derive(Debug, thiserror::Error)]
pub enum MigrationRefusal {
    /// At least one added migration is not known to be applied.
    #[error("MIGRATION PENDING: {} on {env}", .ids.join(", "))]
    Pending { ids: Vec<String>, env: String },
    /// A transient failure: the status could not be read.
    #[error("MIGRATION STATUS UNKNOWN: {0}")]
    StatusUnknown(String),
    /// The adapter or its configuration is broken.
    #[error("MIGRATION ADAPTER BUG: {0}")]
    AdapterBug(String),
    /// Local git could not list the delivery's added migrations.
    #[error("MIGRATION CHECK FAILED: local git: {0}")]
    CheckFailed(String),
}

impl MigrationRefusal {
    /// The refusal class, which an override reason must name.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending { .. } => "MIGRATION PENDING",
            Self::StatusUnknown(_) => "MIGRATION STATUS UNKNOWN",
            Self::AdapterBug(_) => "MIGRATION ADAPTER BUG",
            Self::CheckFailed(_) => "MIGRATION CHECK FAILED",
        }
    }

    /// What clears this refusal without an override.
    pub fn next_step(&self) -> String {
        match self {
            Self::Pending { env, .. } => format!("Apply the migration on {env}, then retry."),
            Self::StatusUnknown(_) => {
                "Retry when the adapter can report migration status.".into()
            }
            Self::AdapterBug(_) => {
                "Fix the project adapter or [qa.release] configuration, then retry.".into()
            }
            Self::CheckFailed(_) => {
                "Make the target and delivery tip local commits (fetch them), then retry.".into()
            }
        }
    }
}

/// The migration id `pattern`'s single `*` captures in `path`, if it matches.
fn migration_id<'p>(pattern: &str, path: &'p str) -> Option<&'p str> {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&'p str> = path.split('/').collect();
    if pattern.len() != path.len() {
        return None;
    }
    let mut id = None;
    for (want, got) in pattern.iter().zip(&path) {
        match want.split_once('*') {
            None if want == got => {}
            None => return None,
            Some((prefix, suffix)) => {
                let rest = got.strip_prefix(prefix)?.strip_suffix(suffix)?;
                if rest.is_empty() {
                    return None;
                }
                id = Some(rest);
            }
        }
    }
    id
}

/// The resolved `tip` and the migration ids it adds since its merge base with
/// `target`: `git diff --name-only --diff-filter=A <merge-base> <tip> -- <paths>`.
/// Renames count as additions.
fn added_migrations(
    root: &Path,
    paths: &[String],
    target: &str,
    tip: &str,
) -> Result<(String, Vec<String>), String> {
    let rev = |name: &str| {
        git(
            root,
            &["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{name}^{{commit}}")],
        )
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .ok_or_else(|| format!("{} is not a local commit", printable(name)))
    };
    let (target, tip) = (rev(target)?, rev(tip)?);
    let base = git(root, &["merge-base", &target, &tip])
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .ok_or("git merge-base failed")?;
    let pathspecs: Vec<String> = paths.iter().map(|p| format!(":(glob){p}")).collect();
    let mut args = vec![
        "diff", "--name-only", "-z", "--no-renames", "--diff-filter=A", &base, &tip, "--",
    ];
    args.extend(pathspecs.iter().map(String::as_str));
    let output = git(root, &args)
        .filter(|output| output.status.success())
        .ok_or("git diff of the delivery range failed")?;
    let mut ids: Vec<String> = output
        .stdout
        .split(|b| *b == 0)
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter_map(|path| paths.iter().find_map(|p| migration_id(p, path)))
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids.dedup();
    Ok((tip, ids))
}

/// Merge admission for `[qa.release.migrations]`: every migration `tip` adds
/// relative to `target` must be applied in the configured environment. A
/// range that adds none returns empty after one local diff, with no adapter
/// call. Returns the applied ids.
pub fn check_migrations(
    root: &Path,
    config: &ReleaseConfig,
    target: &str,
    tip: &str,
) -> Result<Vec<String>, MigrationRefusal> {
    let Some(migrations) = config.migrations.as_ref().filter(|m| !m.paths.is_empty()) else {
        return Ok(Vec::new());
    };
    let (tip_sha, ids) = added_migrations(root, &migrations.paths, target, tip)
        .map_err(MigrationRefusal::CheckFailed)?;
    if ids.is_empty() {
        return Ok(ids);
    }
    let name = Verb::Migrations.as_str();
    let data = call(
        root,
        config,
        Verb::Migrations,
        Request {
            env: &migrations.env,
            sha: &tip_sha,
            args: serde_json::json!({ "ids": ids }),
            ..Request::default()
        },
    )
    .map_err(|error| match &error {
        CallError::TimedOut(_) => MigrationRefusal::StatusUnknown(error.to_string()),
        CallError::Adapter(_, adapter) => match adapter.kind {
            ErrorKind::External | ErrorKind::RateLimited | ErrorKind::Insufficient => {
                MigrationRefusal::StatusUnknown(error.to_string())
            }
            ErrorKind::Invalid | ErrorKind::Internal => {
                MigrationRefusal::AdapterBug(error.to_string())
            }
        },
        CallError::Protocol(..) => MigrationRefusal::AdapterBug(error.to_string()),
    })?;
    // The parse error is dropped: it could quote adapter data.
    let statuses: BTreeMap<String, MigrationStatus> = serde_json::from_value(data).map_err(|_| {
        MigrationRefusal::AdapterBug(format!(
            "adapter {name} failed: data is not an id to applied|pending|unknown map"
        ))
    })?;
    let unapplied: Vec<String> = ids
        .iter()
        .filter(|id| statuses.get(id.as_str()) != Some(&MigrationStatus::Applied))
        .cloned()
        .collect();
    if unapplied.is_empty() {
        Ok(ids)
    } else {
        Err(MigrationRefusal::Pending {
            ids: unapplied,
            env: printable(&migrations.env),
        })
    }
}

/// Test projects with the tracked fake adapter at `./scripts/qa/fake-adapter`.
#[cfg(test)]
pub(crate) mod test_project {
    use super::ReleaseConfig;
    use std::path::Path;
    use std::process::Command;

    pub(crate) struct Project {
        dir: tempfile::TempDir,
        /// The adapter configuration the fixture installed.
        pub(crate) installed: ReleaseConfig,
    }

    impl Project {
        /// A git repository on `main` with the fake adapter installed, uncommitted.
        pub(crate) fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let installed = fake_adapter(dir.path(), "");
            let project = Self { dir, installed };
            project.git(&["init", "-q", "-b", "main"]);
            project
        }

        pub(crate) fn root(&self) -> &Path {
            self.dir.path()
        }

        pub(crate) fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .args(["-c", "user.name=Cassy Test", "-c", "user.email=test@cas.test"])
                .args(args)
                .current_dir(self.root())
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }

        /// Writes `path` and commits everything; returns the new HEAD.
        pub(crate) fn commit(&self, path: &str, body: &str) -> String {
            let file = self.root().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, body).unwrap();
            self.git(&["add", "."]);
            self.git(&["commit", "-qm", path]);
            self.git(&["rev-parse", "HEAD"])
        }
    }

    /// Copies the tracked fake adapter into `root` and configures it.
    pub(crate) fn fake_adapter(root: &Path, env: &str) -> ReleaseConfig {
        use std::os::unix::fs::PermissionsExt;
        let adapter = root.join("scripts/qa/fake-adapter");
        std::fs::create_dir_all(adapter.parent().unwrap()).unwrap();
        std::fs::write(
            &adapter,
            include_str!("../tests/fixtures/qa-release/fake-adapter"),
        )
        .unwrap();
        std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o700)).unwrap();
        ReleaseConfig {
            adapter: "./scripts/qa/fake-adapter".into(),
            env: env.split(',').filter(|name| !name.is_empty()).map(str::to_owned).collect(),
            timeout_secs: 5,
            ..ReleaseConfig::default()
        }
    }

    /// Builds `[qa.release]` through `cas config set`, checking each key lists back.
    pub(crate) fn release_config(settings: &[(&str, &str)]) -> ReleaseConfig {
        let mut config = crate::config::Config::default();
        for (key, value) in settings {
            let key = format!("qa.release.{key}");
            config.set(&key, value).unwrap();
            assert!(config.list().contains(&(key, (*value).into())));
        }
        config.qa.unwrap().release.unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_project::{Project, fake_adapter, release_config};
    use super::*;
    use std::process::Command;

    include!("qa_release/fixture_test.rs");

    /// The tracked fake adapter runs through the real sandbox: `served` echoes
    /// the allow-listed `FAKE_SERVED_SHA`, and `FAKE_ERROR_KIND` answers with a
    /// classified error instead.
    #[test]
    fn served_equivalence() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = Project::new();
        let root = project.root();
        let served = project.commit("apps/backend/api.ts", "v1");
        let web_only = project.commit("apps/web/page.ts", "v2");
        let backend_changed = project.commit("apps/backend/api.ts", "v3");

        let config = release_config(&[
            ("adapter", "./scripts/qa/fake-adapter"),
            ("env", "FAKE_SERVED_SHA,FAKE_ERROR_KIND"),
            ("components", r#"{"backend":["apps/backend"]}"#),
            ("integration_branch", "staging"),
            ("branches", "main"),
            ("timeout_secs", "5"),
        ]);
        assert_eq!(config.adapter, project.installed.adapter);
        env.set("FAKE_SERVED_SHA", &served);
        env.set("FAKE_PRIVATE", "must-not-inherit");

        let components = preflight_served(root, &config, "staging", &web_only).unwrap();
        assert_eq!(components[0].sha.as_deref(), Some(served.as_str()));
        let refusal = preflight_served(root, &config, "staging", &backend_changed)
            .unwrap_err()
            .to_string();
        assert!(
            refusal.starts_with("SERVED MISMATCH: backend serves")
                && refusal.contains("apps/backend"),
            "{refusal}"
        );

        env.set("FAKE_ERROR_KIND", "rate_limited");
        match preflight_served(root, &config, "staging", &web_only) {
            Err(ServedRefusal::Call(CallError::Adapter("served", error))) => {
                assert_eq!(error.kind, ErrorKind::RateLimited);
                assert_eq!(error.retry_after_s, Some(30));
            }
            other => panic!("expected a classified adapter error, got {other:?}"),
        }
    }

    /// Only declared subject kinds with opaque ids and declared invariant IDs
    /// reach the adapter; its answer maps to pass, fail, blocked or missing.
    #[test]
    fn readback_allowlist() {
        use crate::qa_readback::{Declaration, Outcome};
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let config = fake_adapter(project.path(), "FAKE_READBACK_DATA,FAKE_ERROR_KIND");
        let mut journey = crate::qa_registry::Journey {
            id: "signup".into(),
            readback: Some(Declaration {
                subjects: vec!["account_uid".into(), "workspace_id".into()],
                invariants: vec!["INV-ACCOUNT".into(), "INV-WORKSPACE".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ids = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        let subjects = ids(&[("account_uid", "uid-7f3a"), ("workspace_id", "ws_42")]);
        let run = |journey: &crate::qa_registry::Journey, subjects: &BTreeMap<String, String>| {
            readback(project.path(), &config, "staging", &"a".repeat(40), journey, subjects)
        };

        // Refused before any adapter call (FAKE_READBACK_DATA is still unset).
        for refused in [
            ids(&[("email", "a@b.c")]),
            ids(&[("account_uid", "1; DROP TABLE users")]),
        ] {
            assert!(run(&journey, &refused).is_err(), "{refused:?}");
        }
        let declared = journey.readback.clone();
        journey.readback.as_mut().unwrap().invariants =
            vec!["SELECT * FROM accounts".into()];
        assert!(run(&journey, &subjects).is_err(), "a query is not an invariant ID");
        journey.readback = declared;

        env.set(
            "FAKE_READBACK_DATA",
            r#"{"INV-ACCOUNT":{"ok":true,"observed":1},"INV-WORKSPACE":{"ok":true,"observed":"secret"}}"#,
        );
        match run(&journey, &subjects).unwrap() {
            Outcome::Pass(readback) => {
                assert!(readback.proves());
                assert_eq!(readback.subject, "account_uid=uid-7f3a workspace_id=ws_42");
            }
            other => panic!("expected PASS, got {other:?}"),
        }
        env.set(
            "FAKE_READBACK_DATA",
            r#"{"INV-ACCOUNT":{"ok":true},"INV-WORKSPACE":{"ok":false}}"#,
        );
        assert!(matches!(
            run(&journey, &subjects).unwrap(),
            Outcome::Fail { failed, .. } if failed == ["INV-WORKSPACE"]
        ));
        env.set("FAKE_READBACK_DATA", r#"{"INV-ACCOUNT":{"ok":true}}"#);
        assert!(matches!(run(&journey, &subjects).unwrap(), Outcome::Missing(_)));
        env.set("FAKE_ERROR_KIND", "external");
        assert!(matches!(
            run(&journey, &subjects).unwrap(),
            Outcome::BlockedExternal(blocked) if blocked.provider == "fake-health"
        ));
    }

    /// Journeys' `depends_on_flags` are read through the `flags` verb, stored
    /// in the LEDGER.md `Flags:` header and diffed once against the target env
    /// as CONFIG_MISMATCH naming each flag and both values.
    #[test]
    fn flag_drift() {
        use flags::{FlagSnapshot, compare, read};
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let calls = tempfile::tempdir().unwrap();
        let log = calls.path().join("calls.log");
        let config = fake_adapter(project.path(), "FAKE_FLAGS_STAGING,FAKE_FLAGS_PROD,FAKE_CALL_LOG");
        env.set("FAKE_CALL_LOG", &log);
        let registry: crate::qa_registry::Registry = serde_yaml::from_str(
            "journeys:\n  - id: signup\n  - id: audit\n    depends_on_flags: [post_audit.audio_evidence.enabled, audit.mode]\n",
        )
        .unwrap();
        let journeys = &registry.journeys;
        let sha = "a".repeat(40);
        let read = |env: &str| read(project.path(), &config, env, &sha, journeys);

        let undeclared = flags::read(project.path(), &config, "staging", &sha, &journeys[..1]);
        assert!(undeclared.unwrap().values.is_empty() && !log.exists(), "no declared flag, no call");

        env.set(
            "FAKE_FLAGS_STAGING",
            r#"{"post_audit.audio_evidence.enabled":false,"audit.mode":"a|b","undeclared":1}"#,
        );
        env.set("FAKE_FLAGS_PROD", r#"{"post_audit.audio_evidence.enabled":true,"audit.mode":"a|b"}"#);
        let snapshot = read("staging").unwrap();
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains(r#""args":{"keys":["audit.mode","post_audit.audio_evidence.enabled"]}"#)
        );
        assert!(!snapshot.values.contains_key("undeclared"));
        let line = snapshot.ledger_line();
        assert!(!line.contains('|'), "a value must not form a ledger cell: {line}");
        let ledger = format!("# Ledger\n{line}\n| id | cell | expected | observed | verdict |\n");
        assert_eq!(FlagSnapshot::from_ledger(&ledger).as_ref(), Some(&snapshot));

        let target = read("prod").unwrap();
        let mismatches: Vec<String> =
            compare(&snapshot, &target, journeys).iter().map(ToString::to_string).collect();
        assert_eq!(
            mismatches,
            ["CONFIG_MISMATCH post_audit.audio_evidence.enabled: test=false target=true (audit)"]
        );
        assert!(compare(&snapshot, &snapshot, journeys).is_empty());

        env.set("FAKE_FLAGS_PROD", r#"{"audit.mode":"a|b"}"#);
        let refusal = read("prod").unwrap_err().to_string();
        assert!(refusal.contains("omits post_audit.audio_evidence.enabled"), "{refusal}");
    }

    /// Only added files under `migrations.paths` reach the `migrations` verb;
    /// each answer class maps to its refusal, all failing closed.
    #[test]
    fn migrations_verb() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = Project::new();
        let calls = tempfile::tempdir().unwrap();
        let log = calls.path().join("calls.log");
        let config = release_config(&[
            ("adapter", "./scripts/qa/fake-adapter"),
            ("env", "FAKE_MIGRATIONS,FAKE_ERROR_KIND,FAKE_CALL_LOG,FAKE_SLEEP"),
            ("integration_branch", "staging"),
            ("migrations.paths", "db/migrations/*/migration.sql"),
            ("migrations.env", "staging"),
            ("timeout_secs", "5"),
        ]);
        env.set("FAKE_CALL_LOG", &log);
        let root = project.root();
        project.commit("db/migrations/20260101_init/migration.sql", "init");
        project.git(&["checkout", "-qb", "plain"]);
        project.commit("db/migrations/20260101_init/migration.sql", "edited");
        project.commit("db/migrations/20261021_add/notes.md", "not a migration");
        assert!(check_migrations(root, &config, "main", "plain").unwrap().is_empty());
        assert!(!log.exists(), "a merge adding no migration must not call the adapter");

        project.git(&["checkout", "-q", "main"]);
        project.git(&["checkout", "-qb", "migration"]);
        let tip = project.commit("db/migrations/20261021_add/migration.sql", "add");
        // The target moves past the merge base: it adds a migration and drops
        // one the tip inherited. Diffing target..tip in either direction,
        // instead of from the merge base, would report one of them as added.
        project.git(&["checkout", "-q", "main"]);
        project.git(&["rm", "-q", "db/migrations/20260101_init/migration.sql"]);
        project.commit("db/migrations/20261022_main/migration.sql", "main");
        env.set("FAKE_MIGRATIONS", r#"{"20261021_add":"applied"}"#);
        assert_eq!(check_migrations(root, &config, "main", "migration").unwrap(), ["20261021_add"]);
        let request = std::fs::read_to_string(&log).unwrap();
        assert!(
            request.contains(&format!(r#""sha":"{tip}""#))
                && request.contains(r#""args":{"ids":["20261021_add"]}"#),
            "{request}"
        );

        for data in [r#"{"20261021_add":"pending"}"#, r#"{"20261021_add":"unknown"}"#, "{}"] {
            env.set("FAKE_MIGRATIONS", data);
            let refusal = check_migrations(root, &config, "main", "migration").unwrap_err();
            assert_eq!(refusal.to_string(), "MIGRATION PENDING: 20261021_add on staging");
        }
        for (kind, prefix) in [
            ("rate_limited", "MIGRATION STATUS UNKNOWN: "),
            ("external", "MIGRATION STATUS UNKNOWN: "),
            ("insufficient", "MIGRATION STATUS UNKNOWN: "),
            ("internal", "MIGRATION ADAPTER BUG: "),
            ("invalid", "MIGRATION ADAPTER BUG: "),
        ] {
            env.set("FAKE_ERROR_KIND", kind);
            let refusal = check_migrations(root, &config, "main", "migration").unwrap_err().to_string();
            assert!(refusal.starts_with(prefix), "{kind}: {refusal}");
        }
        env.remove("FAKE_ERROR_KIND");
        env.set("FAKE_SLEEP", "3");
        let impatient = ReleaseConfig { timeout_secs: 1, ..config.clone() };
        let refusal = check_migrations(root, &impatient, "main", "migration").unwrap_err();
        assert!(matches!(refusal, MigrationRefusal::StatusUnknown(_)), "{refusal}");
        let refusal = check_migrations(root, &config, "main", "no-such-branch").unwrap_err();
        assert!(
            matches!(refusal, MigrationRefusal::CheckFailed(_))
                && refusal.next_step().contains("fetch them"),
            "{refusal}"
        );
    }
}
