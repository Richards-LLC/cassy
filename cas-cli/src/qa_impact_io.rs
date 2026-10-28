//! Read-only snapshots and Git/legacy selector boundary for the pure impact engine.
use super::*;
use crate::qa_registry::Registry;
use anyhow::{Context, anyhow, ensure};
use std::process::Command;

const LEGACY_SELECTOR_PATH: &str = "scripts/journeys-for-diff.py";

/// Proven catalog/registry drift, distinct from unavailable snapshot inputs.
#[derive(Debug, thiserror::Error)]
#[error(
    "legacy selector returned journey {journey} missing from {registry}; align the catalog and registry"
)]
pub(crate) struct CatalogDrift {
    journey: String,
    registry: String,
}

pub(crate) fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .context("start git for QA impact")?;
    ensure!(
        out.status.success(),
        "git {} failed: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8(out.stdout).context("Git paths/output must be UTF-8")
}

pub(crate) fn resolve_ref(repo: &Path, reference: &str) -> Result<String> {
    Ok(git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )?
    .trim()
    .to_owned())
}

pub(crate) fn changes_for_diff(repo: &Path, base: &str, head: Option<&str>) -> Result<Vec<Change>> {
    let mut args = vec!["diff", "--name-status", "-z", "-M", base];
    if let Some(head) = head {
        args.push(head);
    }
    args.push("--");
    let text = git(repo, &args)?;
    let mut parts = text.split('\0').filter(|part| !part.is_empty());
    let mut changes = Vec::new();
    while let Some(status) = parts.next() {
        let first = parts
            .next()
            .ok_or_else(|| anyhow!("incomplete git name-status row"))?;
        let (old_file, file) = if status.starts_with(['R', 'C']) {
            (
                Some(first.into()),
                parts
                    .next()
                    .ok_or_else(|| anyhow!("incomplete git rename row"))?,
            )
        } else {
            (None, first)
        };
        changes.push(Change {
            status: status.into(),
            file: file.into(),
            old_file,
        });
    }
    Ok(changes)
}

pub(crate) fn untracked(repo: &Path) -> Result<Vec<Change>> {
    Ok(
        git(repo, &["ls-files", "-z", "--others", "--exclude-standard"])?
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(|path| Change {
                status: "A".into(),
                file: path.into(),
                old_file: None,
            })
            .collect(),
    )
}

struct Snapshot<'a> {
    root: &'a Path,
    head: Option<String>,
    paths: Vec<String>,
}
impl<'a> Snapshot<'a> {
    fn new(root: &'a Path, head: Option<&str>) -> Result<Self> {
        let head = head.map(|head| resolve_ref(root, head)).transpose()?;
        let text = match &head {
            Some(head) => git(root, &["ls-tree", "-rz", "--name-only", head])?,
            None => git(root, &["ls-files", "-z", "--cached", "--full-name"])?,
        };
        Ok(Self {
            root,
            head,
            paths: text
                .split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
    fn read(&self, path: &str) -> Result<Option<String>> {
        if let Some(head) = &self.head {
            if !self.paths.iter().any(|p| p == path) {
                return Ok(None);
            }
            return git(self.root, &["show", &format!("{head}:{path}")]).map(Some);
        }
        match std::fs::read_to_string(self.root.join(path)) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("read {path}")),
        }
    }
}

pub(super) fn analyze(
    repo: &Path,
    qa: &QaConfig,
    changes: &[Change],
    base: Option<&str>,
    head: Option<&str>,
) -> Result<Impact> {
    let tree = Snapshot::new(repo, head)?;
    let before = base
        .map(|base| Snapshot::new(repo, Some(base)))
        .transpose()?;
    let registry = tree
        .read(&qa.registry.path)?
        .map(|text| crate::qa_registry::parse(&text))
        .transpose()?;
    let previous = before
        .as_ref()
        .map(|tree| tree.read(&qa.registry.path))
        .transpose()?
        .flatten()
        .and_then(|text| crate::qa_registry::parse(&text).ok());
    let codemap = tree.read(".claude/CODEMAP.md")?;
    let overview_present = tree.read("docs/PRODUCT_OVERVIEW.md")?.is_some();
    let needs_legacy_selector = registry.is_none()
        || registry
            .as_ref()
            .is_some_and(|registry| registry.selection == crate::qa_registry::Selection::LegacyCatalog);
    let selector_present = needs_legacy_selector
        && tree.paths.iter().any(|path| path == LEGACY_SELECTOR_PATH)
        && tree.read(LEGACY_SELECTOR_PATH)?.is_some();
    let selected = if let Some(registry) = registry
        .as_ref()
        .filter(|r| r.selection == crate::qa_registry::Selection::LegacyCatalog)
    {
        ensure!(
            tree.read("docs/qa/journeys.md")?.is_some(),
            "legacy_catalog selection requires docs/qa/journeys.md"
        );
        ensure!(
            selector_present,
            "legacy_catalog selection requires committed {LEGACY_SELECTOR_PATH}; commit that selector or set selection: globs in {}",
            qa.registry.path
        );
        let paths = changes
            .iter()
            .flat_map(|c| std::iter::once(c.file.clone()).chain(c.old_file.clone()))
            .filter(|path| {
                !crate::qa_registry::matches_path(registry.ignore.iter().map(String::as_str), path)
            })
            .collect::<Vec<_>>();
        let ids = catalog_ids(&tree, &paths, base)?;
        for id in &ids {
            if !registry.journeys.iter().any(|j| j.id == *id) {
                return Err(CatalogDrift {
                    journey: id.clone(),
                    registry: qa.registry.path.clone(),
                }
                .into());
            }
        }
        Some(ids.into_iter().collect::<std::collections::BTreeSet<_>>())
    } else {
        None
    };
    let mut report = impact_for_selected_changes(
        registry.as_ref(),
        qa,
        changes,
        codemap.as_deref().unwrap_or(""),
        selected.as_ref(),
    );
    if let Some(registry) = &registry {
        let validation = registry.validate(&tree.paths);
        report.registry.valid = validation.problems.is_empty();
        report.registry.debt_count = validation.debt.len();
        if let (Some(previous), Some(before)) = (&previous, &before) {
            let old = previous.validate(&before.paths);
            report.registry.errors_from_diff = validation
                .problems
                .iter()
                .filter(|p| !old.problems.contains(p))
                .cloned()
                .collect();
        } else if changes.iter().any(|c| c.file == qa.registry.path) {
            report.registry.errors_from_diff = validation.problems.clone();
        }
        if !validation.problems.is_empty() {
            report.warnings.push(format!(
                "Journey registry has {} validation problem(s). Run cas qa journeys --check.",
                validation.problems.len()
            ));
        }
    } else if let Some(catalog) = tree.read("docs/qa/journeys.md")? {
        report
            .warnings
            .retain(|w| !w.starts_with("No journey registry:"));
        if selector_present {
            let paths: Vec<String> = changes
                .iter()
                .flat_map(|c| std::iter::once(c.file.clone()).chain(c.old_file.clone()))
                .collect();
            let ids = legacy_ids(&tree, &paths)?;
            report.source = "legacy_catalog".into();
            report.areas.clear();
            report.warnings.push("No journey registry: using the legacy catalog; invariants, traffic and per-file ownership are unavailable. Run cas qa journeys --seed.".into());
            let metadata = legacy_metadata(&catalog);
            for id in ids {
                let journey = metadata
                    .iter()
                    .find(|j| j.id == id)
                    .ok_or_else(|| anyhow!("legacy selector returned unknown journey {id}"))?;
                report.journeys.push(JourneyImpact {
                    id,
                    name: journey.name.clone().unwrap_or_else(|| journey.id.clone()),
                    status: "live".into(),
                    kind: None,
                    share_pct: None,
                    basis: None,
                    entry: journey.entry.clone(),
                    env_status: BTreeMap::new(),
                    invariants: vec![],
                    fixtures: vec![],
                    spec: journey.spec.clone(),
                    known_failure: None,
                    matches: vec![],
                    evidence_required: evidence_required(journey, None),
                });
            }
            sort_journeys(&mut report.journeys);
        } else {
            report.warnings.push(format!(
                "Legacy catalog has no committed {LEGACY_SELECTOR_PATH}: impact is by CODEMAP area only. Commit that selector or run cas qa journeys --seed."
            ));
        }
    }
    let mut warnings = Vec::new();
    let watch = crate::hooks::handlers::handlers_events::project_overview::watch_targets(repo);
    if !overview_present {
        warnings.push("PRODUCT_OVERVIEW.md is missing: journeys have no product context. Run the project-overview skill.".into());
    } else if let Some(warning) = &watch.warning {
        warnings.push(format!("PRODUCT_OVERVIEW watch targets: {warning}."));
    }
    if codemap.is_none() {
        warnings.push(
            "CODEMAP.md is missing: unowned files have no area label. Run the codemap skill."
                .into(),
        );
    } else {
        // GH #1340: the freshness source `cas codemap status` and
        // `cas qa journeys --status` use: committed history after the CODEMAP
        // commit on the measured ref, so neither that commit's own files nor
        // untracked checkout debris count as drift.
        use crate::hooks::handlers::handlers_events::{codemap, docs_freshness::Measurement};
        let measurement = match &tree.head {
            Some(head) => Measurement {
                root: repo.to_path_buf(),
                store: repo.join(".cas"),
                reference: head.clone(),
            },
            // A linked worktree's shared store names its task's integration ref.
            None => Measurement::at(
                repo,
                &crate::store::detect::find_cas_root_ignoring_env(repo)
                    .unwrap_or_else(|_| repo.join(".cas")),
            ),
        };
        let count = codemap::evaluate_codemap_at(&measurement)
            .and_then(|evaluation| evaluation.structural_change_count());
        if let (Some(count @ 1..), Some(last)) =
            (count, measurement.last_commit(".claude/CODEMAP.md"))
        {
            warnings.push(format!("CODEMAP is stale ({count} structural changes since {last}): unowned files may be mislabelled."));
        }
    }
    warnings.append(&mut report.warnings);
    report.warnings = warnings;
    report.upkeep = upkeep(
        &watch,
        changes,
        &report,
        codemap.as_deref(),
        overview_present,
        registry.as_ref(),
        previous.as_ref(),
        before
            .as_ref()
            .map(|s| s.paths.as_slice())
            .unwrap_or(&tree.paths),
        &tree.paths,
    );
    Ok(report)
}

// The registry engine launches no processes. Legacy compatibility deliberately
// reuses the project's existing Python selector at this one adapter boundary.
fn legacy_ids(tree: &Snapshot<'_>, paths: &[String]) -> Result<Vec<String>> {
    let surface: Vec<String> = paths
        .iter()
        .filter(|p| !crate::qa_pass::is_non_surface_path(p))
        .cloned()
        .collect();
    catalog_ids(tree, &surface, None)
}

fn catalog_ids(tree: &Snapshot<'_>, paths: &[String], base: Option<&str>) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(head) = &tree.head {
        return crate::qa_evidence::journeys::journeys_for_paths_at_with_base(
            tree.root, head, paths, base,
        )
        .map_err(|e| anyhow!(e));
    }
    let mut command = Command::new("python3");
    command
        .arg(tree.root.join("scripts/journeys-for-diff.py"))
        .arg("--paths")
        .args(paths)
        .env("CAS_JOURNEYS_ROOT", tree.root)
        .env_remove("CAS_JOURNEYS_HEAD")
        .env_remove("CAS_JOURNEYS_BASE")
        .current_dir(tree.root);
    if let Some(base) = base {
        command.env("CAS_JOURNEYS_BASE", base);
    }
    let output = command.output().context("start legacy journey selector")?;
    ensure!(
        output.status.success(),
        "legacy journey selector failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("legacy selector must emit JSON")?;
    let rows = value["journeys"]
        .as_array()
        .ok_or_else(|| anyhow!("legacy selector JSON has no journeys array"))?;
    rows.iter()
        .map(|row| {
            row["id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("legacy selector journey has no id"))
        })
        .collect()
}

fn legacy_metadata(text: &str) -> Vec<crate::qa_registry::Journey> {
    let mut journeys = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("### ") {
            if let Some((id, name)) = heading.split_once(" · ") {
                journeys.push(crate::qa_registry::Journey {
                    id: id.trim().into(),
                    name: Some(name.trim().into()),
                    status: "live".into(),
                    ..Default::default()
                });
            }
        } else if let Some(journey) = journeys.last_mut() {
            let quoted = || line.split('`').nth(1).map(str::to_owned);
            if let Some(entry) = line.strip_prefix("- **Entry:** ") {
                journey.entry = Some(Entry {
                    url: quoted().unwrap_or_else(|| entry.into()),
                    trigger: "legacy catalog entry".into(),
                });
            } else if line.starts_with("- **Suite:** ") {
                journey.spec = quoted();
            }
        }
    }
    journeys
}

fn upkeep(
    watch: &crate::hooks::handlers::handlers_events::project_overview::WatchTargets,
    changes: &[Change],
    report: &Impact,
    codemap: Option<&str>,
    overview: bool,
    registry: Option<&Registry>,
    previous: Option<&Registry>,
    base_paths: &[String],
    head_paths: &[String],
) -> Upkeep {
    let mut upkeep = docs_upkeep(
        changes,
        &report.registry.path,
        codemap,
        overview,
        registry,
        previous,
        watch,
        base_paths,
        head_paths,
    );
    if registry.is_some() {
        upkeep.registry.reasons.extend(
            report
                .registry
                .errors_from_diff
                .iter()
                .map(|p| format!("{}: {}", p.subject, p.message)),
        );
        for path in &report.unowned {
            if path.surface
                && changes
                    .iter()
                    .any(|c| c.file == path.file && c.status.starts_with(['A', 'R']))
            {
                upkeep
                    .registry
                    .reasons
                    .push(format!("unowned new surface {}", path.file));
            }
        }
    }
    for item in [
        &mut upkeep.codemap,
        &mut upkeep.product_overview,
        &mut upkeep.registry,
    ] {
        item.reasons.sort();
        item.reasons.dedup();
        item.required = !item.reasons.is_empty();
    }
    upkeep
}

/// Shared G5/G6 facts for impact reports and park-time enforcement. Callers
/// select snapshots and watch configuration; this function performs no I/O.
#[allow(clippy::too_many_arguments)]
pub(crate) fn docs_upkeep(
    changes: &[Change],
    registry_path: &str,
    codemap: Option<&str>,
    overview: bool,
    registry: Option<&Registry>,
    previous: Option<&Registry>,
    watch: &crate::hooks::handlers::handlers_events::project_overview::WatchTargets,
    base_paths: &[String],
    head_paths: &[String],
) -> Upkeep {
    use std::collections::BTreeSet;
    let mut upkeep = Upkeep::default();
    let touched = |path: &str| {
        changes
            .iter()
            .any(|c| c.file == path || c.old_file.as_deref() == Some(path))
    };
    if let Some(codemap) = codemap.filter(|_| !touched(".claude/CODEMAP.md")) {
        let labels = codemap_labels(codemap);
        // CODEMAP lists direct children of these parents. Git has no directory
        // records, so existence is established from tracked snapshot paths.
        let parent = |path: &str| path.rsplit_once('/').map_or("", |(p, _)| p).to_owned();
        let enumerated: BTreeSet<String> =
            labels.iter().map(|label| parent(&label.prefix)).collect();
        for change in changes {
            let removed = if change.status.starts_with('D') {
                Some(change.file.as_str())
            } else if change.status.starts_with('R') {
                change.old_file.as_deref()
            } else {
                None
            };
            if let Some(removed) = removed {
                for label in &labels {
                    if contains_path(&label.prefix, removed)
                        && base_paths
                            .iter()
                            .any(|old| contains_path(&label.prefix, old))
                        && !head_paths
                            .iter()
                            .any(|now| contains_path(&label.prefix, now))
                    {
                        upkeep
                            .codemap
                            .reasons
                            .push(format!("deleted or renamed mapped path {}", label.prefix));
                    }
                }
            }
            if change.status.starts_with(['A', 'R']) {
                let mut dir = change.file.rsplit_once('/').map(|(p, _)| p);
                while let Some(path) = dir {
                    let parent = parent(path);
                    if enumerated.contains(&parent)
                        && !base_paths.iter().any(|old| contains_path(path, old))
                    {
                        upkeep.codemap.reasons.push(format!(
                            "new directory {path} under enumerated {}",
                            if parent.is_empty() {
                                "repository root"
                            } else {
                                &parent
                            }
                        ));
                    }
                    dir = path.rsplit_once('/').map(|(p, _)| p);
                }
            }
        }
    }
    if overview && !touched("docs/PRODUCT_OVERVIEW.md") {
        for change in changes {
            for path in std::iter::once(change.file.as_str()).chain(change.old_file.as_deref()) {
                if watch.is_domain_path(path)
                    || (change.status.starts_with(['A', 'D', 'R']) && watch.is_in_watch_dir(path))
                {
                    upkeep
                        .product_overview
                        .reasons
                        .push(format!("watched product path {path}"));
                }
            }
        }
        if let Some(registry) = registry {
            for journey in &registry.journeys {
                let old = previous.and_then(|r| r.journeys.iter().find(|j| j.id == journey.id));
                if (journey.status == "live" && old.is_none() && touched(registry_path))
                    || (touched(registry_path)
                        && old.is_some_and(|old| {
                            matches!(
                                (old.status.as_str(), journey.status.as_str()),
                                ("live", "dead") | ("dead", "live")
                            )
                        }))
                {
                    upkeep.product_overview.reasons.push(format!(
                        "journey {} added live or changed live/dead status",
                        journey.id
                    ));
                }
            }
        }
    }
    for item in [&mut upkeep.codemap, &mut upkeep.product_overview] {
        item.reasons.sort();
        item.reasons.dedup();
        item.required = !item.reasons.is_empty();
    }
    upkeep
}
