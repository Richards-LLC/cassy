//! Bootstrap journey registries from tracked source and project context.

use crate::{
    config::QaConfig,
    qa_registry::{self, Entry, Journey, OwnerGroup, Owners, Problem, Registry},
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{collections::BTreeSet, fs, path::Path, process::Command};

#[derive(Debug, Serialize)]
pub struct Status {
    pub path: String,
    pub present: bool,
    pub valid: bool,
    pub journeys: usize,
    pub seeded: usize,
    pub debt_count: usize,
    pub unowned_surfaces: Vec<String>,
    /// GH #1344: `<journey>: <path>` for each planned spec not yet tracked.
    pub planned_specs: Vec<String>,
    pub problems: Vec<Problem>,
    pub added: usize,
}

impl Status {
    /// The `--status` line naming specs still owed, if any.
    pub fn planned_specs_line(&self) -> Option<String> {
        (!self.planned_specs.is_empty()).then(|| {
            format!(
                "  Planned specs (not written): {}",
                self.planned_specs.join(", ")
            )
        })
    }
}

fn tracked(root: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .args(["ls-files", "-z", "--cached", "--full-name"])
        .current_dir(root)
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

pub fn status(root: &Path, qa: &QaConfig) -> Result<Status> {
    let path = root.join(&qa.registry.path);
    let registry = path.try_exists()?.then(|| qa_registry::load(&path));
    let files = if registry.as_ref().is_some_and(|registry| registry.is_ok()) {
        tracked(root)?
    } else {
        vec![]
    };
    Ok(status_with_registry(qa, registry, &files))
}

/// Shared coverage calculation for checkout and immutable integration snapshots.
pub(crate) fn status_with_registry(
    qa: &QaConfig,
    registry: Option<Result<Registry>>,
    files: &[String],
) -> Status {
    let mut status = Status {
        path: qa.registry.path.clone(),
        present: registry.is_some(),
        valid: false,
        journeys: 0,
        seeded: 0,
        debt_count: 0,
        unowned_surfaces: vec![],
        planned_specs: vec![],
        problems: vec![],
        added: 0,
    };
    let registry = match registry {
        None => return status,
        Some(Ok(registry)) => registry,
        Some(Err(error)) => {
            status.problems.push(Problem {
                code: "registry_read".into(),
                subject: qa.registry.path.clone(),
                message: format!("{error:#}"),
            });
            return status;
        }
    };
    let report = registry.validate(files);
    status.valid = report.problems.is_empty();
    status.journeys = registry.journeys.len();
    status.seeded = registry.journeys.iter().filter(|j| j.seeded).count();
    status.debt_count = report.debt.len();
    status.problems = report.problems;
    status.planned_specs = registry
        .journeys
        .iter()
        .filter(|j| j.spec_status.as_deref() == Some("planned"))
        .filter_map(|j| j.spec.as_ref().map(|spec| (j, spec)))
        .filter(|(_, spec)| !files.contains(spec))
        .map(|(j, spec)| format!("{}: {spec}", j.id))
        .collect();
    status.unowned_surfaces = files
        .iter()
        .filter(|file| {
            qa_registry::matches_path(registry.effective_surfaces(qa), file)
                && !qa_registry::matches_path(registry.ignore.iter().map(String::as_str), file)
                && !registry
                    .journeys
                    .iter()
                    .any(|j| qa_registry::matches_path(registry.owner_globs(j), file))
        })
        .cloned()
        .collect();
    status
}

fn slug(text: &str) -> String {
    let value = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase();
    if value.is_empty() {
        "home".into()
    } else {
        value
    }
}

fn candidate(file: &str, url: &str, trigger: &str) -> Journey {
    Journey {
        id: format!("{}-{}", slug(trigger), slug(url)),
        status: "live".into(),
        entry: Some(Entry {
            url: url.into(),
            trigger: trigger.into(),
        }),
        owners: Owners::List(vec![file.into()]),
        seeded: true,
        ..Default::default()
    }
}

fn file_route(file: &str) -> Option<(String, String)> {
    let parts: Vec<_> = file.split('/').collect();
    let index = parts
        .iter()
        .position(|p| matches!(*p, "pages" | "routes" | "app"))?;
    let filename = parts.last()?;
    let (stem, ext) = filename.rsplit_once('.')?;
    if !["vue", "svelte", "tsx", "jsx", "ts", "js"].contains(&ext)
        || stem.contains('.')
        || stem.starts_with('_')
        || stem.starts_with('+') && stem != "+page"
    {
        return None;
    }
    if parts[index] == "app" && stem != "page" {
        return None;
    }
    let mut route: Vec<_> = parts[index + 1..parts.len() - 1]
        .iter()
        .copied()
        .filter(|p| !p.starts_with('('))
        .collect();
    if !["index", "page", "+page"].contains(&stem) {
        route.push(stem);
    }
    Some((
        format!("{}/**", parts[..=index].join("/")),
        format!("/{}", route.join("/")),
    ))
}

fn discover(root: &Path, qa: &QaConfig, files: &[String]) -> Result<Registry> {
    let mut registry = Registry {
        version: Some(1),
        updated: Some(chrono::Utc::now().format("%Y-%m-%d").to_string()),
        seeded_from: vec!["tracked source routes".into()],
        ..Default::default()
    };
    let configured_surfaces: Vec<String> = qa
        .user_facing_paths
        .iter()
        .chain(&qa.api_paths)
        .filter(|glob| {
            files.iter().any(|file| {
                qa_registry::matches_path([glob.strip_prefix('!').unwrap_or(glob.as_str())], file)
            })
        })
        .cloned()
        .collect();
    let mut surfaces = BTreeSet::new();
    let calls = regex::Regex::new(
        r#"(?:\b(?:router|app)\.(get|post|put|patch|delete|head|options)|\.(route))\s*\(\s*["']([^"']+)["']"#,
    )?;
    let controller = regex::Regex::new(r#"@Controller\s*\(\s*(?:["']([^"']*)["'])?"#)?;
    let decorator = regex::Regex::new(
        r#"@(Get|Post|Put|Patch|Delete|Head|Options)\s*\(\s*(?:["']([^"']*)["'])?"#,
    )?;
    for file in files {
        if let Some((surface, url)) = file_route(file) {
            surfaces.insert(surface);
            registry.journeys.push(candidate(file, &url, "Visit"));
        }
        if !["ts", "js", "tsx", "jsx", "rs"].contains(&file.rsplit('.').next().unwrap_or("")) {
            continue;
        }
        let Ok(source) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        let mut found = false;
        for captures in calls.captures_iter(&source) {
            let method = captures
                .get(1)
                .or_else(|| captures.get(2))
                .unwrap()
                .as_str()
                .to_ascii_uppercase();
            registry
                .journeys
                .push(candidate(file, &captures[3], &method));
            found = true;
        }
        for captures in decorator.captures_iter(&source) {
            let start = captures.get(0).unwrap().start();
            let Some(prefix) = controller.captures_iter(&source[..start]).last() else {
                continue;
            };
            let prefix = prefix.get(1).map_or("", |m| m.as_str());
            let suffix = captures.get(2).map_or("", |m| m.as_str());
            let segments = [prefix.trim_matches('/'), suffix.trim_matches('/')];
            let url = format!(
                "/{}",
                segments
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("/")
            );
            registry
                .journeys
                .push(candidate(file, &url, &captures[1].to_ascii_uppercase()));
            found = true;
        }
        if found {
            surfaces.insert(
                file.rsplit_once('/')
                    .map_or_else(|| file.clone(), |(dir, _)| format!("{dir}/**")),
            );
        }
    }
    // Config filters are ordered: sorting them would discard negative exclusions.
    let mut combined = configured_surfaces;
    for surface in surfaces {
        if !combined.contains(&surface) {
            combined.push(surface);
        }
    }
    registry.surfaces = Some(combined);
    // Own helper and configured surface files even when no route syntax can be inferred.
    for file in files {
        if qa_registry::matches_path(registry.effective_surfaces(qa), file)
            && !registry
                .journeys
                .iter()
                .any(|j| qa_registry::matches_path(j.owners.globs(), file))
        {
            let mut journey = candidate(
                file,
                &format!("/{}", file.trim_end_matches('/')),
                "Review surface",
            );
            journey.name = Some(format!("Review {file}"));
            journey.status = "dark".into();
            journey.status_reason = Some("surface file; no route found".into());
            journey.entry = None;
            registry.journeys.push(journey);
        }
    }
    if let Ok(overview) = fs::read_to_string(root.join("PRODUCT_OVERVIEW.md")) {
        registry.seeded_from.push("PRODUCT_OVERVIEW.md".into());
        apply_flows(&mut registry, &overview);
    }
    for path in [".claude/CODEMAP.md", "CODEMAP.md"] {
        if let Ok(map) = fs::read_to_string(root.join(path)) {
            registry.seeded_from.push(path.into());
            for line in map.lines() {
                let Some((paths, description)) =
                    line.split_once(" — ").or_else(|| line.split_once(" - "))
                else {
                    continue;
                };
                for (_, prefix) in paths
                    .split('`')
                    .enumerate()
                    .filter(|(index, _)| index % 2 == 1)
                {
                    if !prefix.ends_with('/') {
                        continue;
                    }
                    let glob = format!("{}**", prefix.trim_start_matches("./"));
                    if !files.iter().any(|file| {
                        qa_registry::matches_path([glob.as_str()], file)
                            && qa_registry::matches_path(registry.effective_surfaces(qa), file)
                    }) {
                        continue;
                    }
                    let mut id = slug(prefix);
                    while registry.owner_groups.contains_key(&id) {
                        id.push_str("-group");
                    }
                    registry.owner_groups.insert(
                        id.clone(),
                        OwnerGroup {
                            description: description.trim().into(),
                            globs: vec![glob.clone()],
                        },
                    );
                    for journey in &mut registry.journeys {
                        if journey
                            .owners
                            .globs()
                            .iter()
                            .any(|file| qa_registry::matches_path([glob.as_str()], file))
                        {
                            journey.owner_groups.push(id.clone());
                        }
                    }
                }
            }
            break;
        }
    }
    unique_ids(&mut registry.journeys, &mut BTreeSet::new());
    anyhow::ensure!(
        !registry.journeys.is_empty(),
        "no routes or flows found; configure qa.user_facing_paths or add PRODUCT_OVERVIEW flow headings, then run cas qa journeys --seed"
    );
    Ok(registry)
}

fn apply_flows(registry: &mut Registry, overview: &str) {
    let mut sections = Vec::new();
    let mut flow_level = None;
    let mut current: Option<(String, String)> = None;
    for line in overview.lines().chain(std::iter::once("# End")) {
        let level = line.bytes().take_while(|b| *b == b'#').count();
        if level > 0 && line.as_bytes().get(level) == Some(&b' ') {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            let heading = line[level..].trim();
            let lower = heading.to_ascii_lowercase();
            let container =
                ["user flows", "user journeys", "flows", "journeys"].contains(&lower.as_str());
            if flow_level.is_some_and(|parent| level <= parent) {
                flow_level = None;
            }
            if container {
                flow_level = Some(level);
            } else if flow_level.is_some() || lower.contains("flow") || lower.contains("journey") {
                current = Some((heading.into(), String::new()));
            }
        } else if let Some((_, body)) = &mut current {
            body.push_str(line);
            body.push('\n');
        }
    }
    for (heading, body) in sections {
        let matches: Vec<_> = registry
            .journeys
            .iter()
            .enumerate()
            .filter(|(_, j)| {
                j.entry.as_ref().is_some_and(|entry| {
                    // Require token boundaries so / does not accidentally match every URL.
                    body.split(|c: char| c.is_whitespace() || "`\"'(),<>".contains(c))
                        .any(|token| token == entry.url)
                        || slug(&heading) == slug(&entry.url)
                })
            })
            .map(|(i, _)| i)
            .collect();
        if let Some(&first) = matches.first() {
            let owners: Vec<_> = matches
                .iter()
                .flat_map(|&i| {
                    registry.journeys[i]
                        .owners
                        .globs()
                        .into_iter()
                        .map(str::to_owned)
                })
                .collect();
            registry.journeys[first].id = slug(&heading);
            registry.journeys[first].name = Some(heading);
            registry.journeys[first].owners = Owners::List(owners);
            for &index in matches.iter().skip(1).rev() {
                registry.journeys.remove(index);
            }
        } else {
            registry.journeys.push(Journey {
                id: slug(&heading),
                name: Some(heading),
                status: "dark".into(),
                status_reason: Some("named in PRODUCT_OVERVIEW; no route found".into()),
                seeded: true,
                ..Default::default()
            });
        }
    }
}

fn unique_ids(journeys: &mut [Journey], used: &mut BTreeSet<String>) {
    for journey in journeys {
        let original = journey.id.clone();
        let mut count = 2;
        while !used.insert(journey.id.clone()) {
            journey.id = format!("{original}-{count}");
            count += 1;
        }
    }
}

// Insert into a block sequence, preserving every existing byte (including comments).
fn append_journeys(contents: &str, journeys: &[Journey]) -> Result<String> {
    let header = regex::Regex::new(r"(?m)^journeys:[ \t]*(?:#.*)?\r?$")?
        .find(contents)
        .context("--merge requires a block-style journeys list; expand inline YAML first")?;
    let start = header.end();
    let rest = &contents[start..];
    let mut offset = start;
    let mut end = contents.len();
    let mut indent = None;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if indent.is_none() && trimmed.starts_with("- ") {
            indent = Some(line.len() - trimmed.len());
        }
        if !line.trim().is_empty()
            && !line.starts_with(char::is_whitespace)
            && !line.starts_with('#')
            && !line.starts_with('-')
        {
            end = offset;
            break;
        }
        offset += line.len();
    }
    let padding = " ".repeat(indent.unwrap_or(2));
    let yaml = serde_yaml::to_string(journeys)?;
    let rows = yaml
        .lines()
        .map(|line| format!("{padding}{line}\n"))
        .collect::<String>();
    Ok(format!(
        "{}{}{rows}{}",
        &contents[..end],
        if contents[..end].ends_with('\n') {
            ""
        } else {
            "\n"
        },
        &contents[end..]
    ))
}

pub fn seed(root: &Path, qa: &QaConfig, merge: bool) -> Result<Status> {
    let path = root.join(&qa.registry.path);
    anyhow::ensure!(
        merge || !path.try_exists()?,
        "registry already exists; use cas qa journeys --seed --merge"
    );
    let files = tracked(root)?;
    let discovered = discover(root, qa, &files)?;
    let (output, added) = if merge && path.try_exists()? {
        let original = fs::read_to_string(&path)?;
        let existing = qa_registry::parse(&original)?;
        let mut additions: Vec<_> = discovered
            .journeys
            .into_iter()
            .filter(|new| {
                !existing
                    .journeys
                    .iter()
                    .any(|old| match (&new.entry, &old.entry) {
                        (Some(a), Some(b)) => {
                            a.url == b.url
                                && new.owners.globs().iter().any(|file| {
                                    qa_registry::matches_path(existing.owner_globs(old), file)
                                })
                        }
                        (None, None) => {
                            new.id == old.id || new.name.is_some() && new.name == old.name
                        }
                        (None, Some(_)) => {
                            !new.owners.globs().is_empty()
                                && new.owners.globs().iter().all(|file| {
                                    qa_registry::matches_path(existing.owner_globs(old), file)
                                })
                        }
                        _ => false,
                    })
            })
            .collect();
        for journey in &mut additions {
            journey
                .owner_groups
                .retain(|name| existing.owner_groups.contains_key(name));
        }
        unique_ids(
            &mut additions,
            &mut existing.journeys.iter().map(|j| j.id.clone()).collect(),
        );
        if additions.is_empty() {
            return status(root, qa);
        }
        (append_journeys(&original, &additions)?, additions.len())
    } else {
        let added = discovered.journeys.len();
        (serde_yaml::to_string(&discovered)?, added)
    };
    let report = qa_registry::parse(&output)?.validate(&files);
    anyhow::ensure!(
        report.problems.is_empty(),
        "seed would create an invalid registry: {:?}; run cas qa journeys --check",
        report.problems
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if merge {
        // Same-directory rename prevents an interrupted write from truncating curation.
        let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(root))?;
        std::io::Write::write_all(&mut temp, output.as_bytes())?;
        if let Ok(metadata) = fs::metadata(&path) {
            temp.as_file().set_permissions(metadata.permissions())?;
        }
        temp.persist(&path).map_err(|error| error.error)?;
    } else {
        use std::io::Write;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?
            .write_all(output.as_bytes())?;
    }
    let mut result = status(root, qa)?;
    result.added = added;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::{qa_registry, test_support::TestEnvGuard};
    use std::{fs, path::Path, process::Command};

    #[test]
    fn status_lists_planned_specs_not_yet_written() {
        let registry = qa_registry::parse(
            "journeys:\n  - {id: deals, status: dark, spec: tests/deals.spec.ts, spec_status: planned}\n  - {id: home, status: dark, spec: tests/home.spec.ts, spec_status: planned}\n",
        )
        .unwrap();
        let status = super::status_with_registry(
            &Default::default(),
            Some(Ok(registry)),
            &["tests/home.spec.ts".into()],
        );
        assert_eq!(
            status.planned_specs_line().as_deref(),
            Some("  Planned specs (not written): deals: tests/deals.spec.ts")
        );
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn write(repo: &Path, path: &str, text: &str) {
        let path = repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn journeys(flags: &[&str]) -> anyhow::Result<()> {
        let args = ["cas", "qa", "journeys"]
            .into_iter()
            .chain(flags.iter().copied());
        crate::cli::run(crate::cli::try_parse_from_with_wordmark(args)?)
    }

    #[test]
    fn qa_seed_fixture_passes_check_and_owns_every_surface() {
        let mut env = TestEnvGuard::temp_home();
        let repo = env.home().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        write(
            &repo,
            "src/pages/signup.vue",
            "<template>Sign up</template>",
        );
        write(
            &repo,
            "web/app/account/page.tsx",
            "export default function Page() {}",
        );
        write(&repo, "src/routes/+page.svelte", "<h1>Home</h1>");
        write(
            &repo,
            "api/accounts.controller.ts",
            "@Controller('accounts')\nclass Accounts {\n@Get(':id') find() {}\n@Post() create() {}\n}\n@Controller()\nclass Root {\n@Get('root') root() {}\n}",
        );
        write(
            &repo,
            "server/routes.ts",
            "router.get('/health', handler);\napp.post('/login', handler);\nRouter::new().route(\"/ready\", get(ready));",
        );
        write(
            &repo,
            "src/components/Button.tsx",
            "export const Button = () => null;",
        );
        write(
            &repo,
            "PRODUCT_OVERVIEW.md",
            "# Product\n## User flows\n### Create account\nStart at `/signup` to create an account.\n### Invite teammate\nAvailable after onboarding.\n",
        );
        write(
            &repo,
            ".claude/CODEMAP.md",
            "# Map\n- `src/` — Account interface\n- `api/` - Account API\n",
        );
        write(&repo, "styles/public.css", "body {}");
        write(&repo, "styles/private.css", "body {}");
        write(
            &repo,
            ".cas/config.toml",
            "[qa]\nuser_facing_paths = ['**/*.vue', '**/*.svelte', '**/*.tsx', '**/*.css', '!styles/private.css']\n",
        );
        git(&repo, &["add", "."]);
        env.set_current_dir(&repo);
        env.set("CAS_ROOT", repo.join(".cas"));
        journeys(&["--status", "--json"]).unwrap();
        assert!(!super::status(&repo, &Default::default()).unwrap().present);
        journeys(&["--seed"]).unwrap();
        journeys(&["--check"]).unwrap();
        journeys(&["--status", "--json"]).unwrap();
        let coverage = super::status(&repo, &Default::default()).unwrap();
        assert!(coverage.present && coverage.valid);
        assert!(coverage.seeded > 0 && coverage.unowned_surfaces.is_empty());
        let registry = qa_registry::load(&repo.join(qa_registry::DEFAULT_PATH)).unwrap();
        assert!(registry.journeys.iter().all(|j| j.seeded));
        for url in [
            "/signup",
            "/account",
            "/",
            "/accounts/:id",
            "/accounts",
            "/health",
            "/login",
            "/ready",
            "/root",
        ] {
            assert!(
                registry
                    .journeys
                    .iter()
                    .any(|j| j.entry.as_ref().is_some_and(|e| e.url == url)),
                "missing route {url}"
            );
        }
        assert!(
            registry.journeys.iter().any(|j| j.id == "create-account"
                && j.entry.as_ref().is_some_and(|e| e.url == "/signup"))
        );
        assert!(
            registry
                .journeys
                .iter()
                .any(|j| j.id == "invite-teammate" && j.status == "dark")
        );
        assert!(
            registry
                .owner_groups
                .values()
                .any(|g| g.description == "Account interface" && g.globs == ["src/**"])
        );
        for file in [
            "src/pages/signup.vue",
            "web/app/account/page.tsx",
            "src/routes/+page.svelte",
            "api/accounts.controller.ts",
            "server/routes.ts",
            "src/components/Button.tsx",
            "styles/public.css",
        ] {
            assert!(
                qa_registry::matches_path(registry.effective_surfaces(&Default::default()), file),
                "missing surface {file}"
            );
            assert!(
                registry
                    .journeys
                    .iter()
                    .any(|j| qa_registry::matches_path(registry.owner_globs(j), file)),
                "unowned {file}"
            );
        }
        assert!(!qa_registry::matches_path(
            registry.effective_surfaces(&Default::default()),
            "styles/private.css"
        ));
        let bytes = fs::read(repo.join(qa_registry::DEFAULT_PATH)).unwrap();
        assert!(
            journeys(&["--seed"]).is_err(),
            "must refuse overwriting a registry"
        );
        assert_eq!(
            fs::read(repo.join(qa_registry::DEFAULT_PATH)).unwrap(),
            bytes
        );
    }

    #[test]
    fn qa_seed_merge_adds_route_without_rewriting_curated_entry() {
        let mut env = TestEnvGuard::temp_home();
        let repo = env.home().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        write(&repo, "pages/signup.vue", "<template>Signup</template>");
        let curated = "version: 1\nupdated: '2026-10-20'\nsurfaces: ['pages/**']\njourneys:\n  # Keep this hand-curated entry, including its formatting.\n  - id: curated-signup\n    status: live\n    name: 'Create an account'\n    entry: {url: /signup, trigger: Join}\n    owners: ['pages/signup.vue', 'components/**']\n    steps: [Join, Confirm]\n    invariants: [ACCOUNT]\n    traffic: {share_pct: 70, basis: new accounts}\ninvariants:\n  ACCOUNT: {statement: Account exists, check: Read account}\n";
        write(
            &repo,
            "components/Shared.tsx",
            "export const Shared = () => null;",
        );
        write(&repo, qa_registry::DEFAULT_PATH, curated);
        git(&repo, &["add", "."]);
        env.set_current_dir(&repo);
        write(&repo, "pages/settings.vue", "<template>Settings</template>");
        git(&repo, &["add", "."]);
        journeys(&["--seed", "--merge"]).unwrap();
        journeys(&["--check"]).unwrap();
        let merged = fs::read_to_string(repo.join(qa_registry::DEFAULT_PATH)).unwrap();
        let registry = qa_registry::parse(&merged).unwrap();
        assert_eq!(registry.journeys.len(), 2);
        let original_entry = curated
            .split("journeys:\n")
            .nth(1)
            .unwrap()
            .split("\ninvariants:\n")
            .next()
            .unwrap();
        assert!(
            merged.contains(original_entry),
            "curated entry bytes changed"
        );
        assert_eq!(registry.journeys[0].id, "curated-signup");
        assert_eq!(
            registry.journeys[0].traffic.as_ref().unwrap().share_pct,
            Some(70.0)
        );
        assert!(
            registry
                .journeys
                .iter()
                .any(|j| j.seeded && j.entry.as_ref().is_some_and(|e| e.url == "/settings"))
        );
        journeys(&["--seed", "--merge"]).unwrap();
        assert_eq!(
            fs::read_to_string(repo.join(qa_registry::DEFAULT_PATH)).unwrap(),
            merged,
            "merge must be idempotent"
        );
    }
}
