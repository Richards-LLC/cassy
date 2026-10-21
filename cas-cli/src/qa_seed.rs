//! Bootstrap journey registries from tracked source and project context.

#[cfg(test)]
mod tests {
    use crate::{qa_registry, test_support::TestEnvGuard};
    use std::{fs, path::Path, process::Command};

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
            "@Controller('accounts')\nclass Accounts {\n@Get(':id') find() {}\n@Post() create() {}\n}",
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
        git(&repo, &["add", "."]);
        env.set_current_dir(&repo);
        journeys(&["--status", "--json"]).unwrap();
        journeys(&["--seed"]).unwrap();
        journeys(&["--check"]).unwrap();
        journeys(&["--status", "--json"]).unwrap();
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
        let curated = "version: 1\nupdated: '2026-10-20'\nsurfaces: ['pages/**']\njourneys:\n  # Keep this hand-curated entry, including its formatting.\n  - id: curated-signup\n    status: live\n    name: 'Create an account'\n    entry: {url: /signup, trigger: Join}\n    owners: ['pages/signup.vue']\n    steps: [Join, Confirm]\n    invariants: [ACCOUNT]\n    traffic: {share_pct: 70, basis: new accounts}\ninvariants:\n  ACCOUNT: {statement: Account exists, check: Read account}\n";
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
