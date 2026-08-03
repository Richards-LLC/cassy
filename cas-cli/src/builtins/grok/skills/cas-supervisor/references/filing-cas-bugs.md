# Filing CAS-system bugs

A standing directive: **file every CAS-system bug you observe, by reflex.** Do not leave it in chat, defer it to "later", or say only "report it upstream". A CAS-system bug you noticed but did not preserve is a bug that resurfaces.

"CAS-system" means a defect in CAS itself: the verifier, hooks, factory/director orchestration, MCP dispatch, the task-verifier agent, worker/supervisor prompts, or builtin skills — regardless of which downstream project surfaced it.

## Route by repository

- **If this repository is the CAS source:** create an in-repo task (`task action=create task_type=bug`) and let a worker fix it there.
- **In any downstream project:** use the configured GitHub issue intake below. Other projects consume CAS; they do not modify it locally.

## Configure the upstream issue target

The target is project-local configuration in `.cas/config.toml`:

```toml
[issues]
repo = "owner/repo"
```

Set or inspect it with:

```sh
cas config set issues.repo owner/repo
cas config get issues.repo
```

**Do not derive this value from the current repository's `origin` remote.** In a downstream project, `origin` normally identifies the consumer's repository, not its CAS upstream; inferring it would silently send a CAS-system bug to the wrong tracker. Never hardcode one installation's repository in a builtin skill.

## File without risking report loss

1. First write the complete, public-safe report to a new, uniquely named `docs/requests/BUG-<slug>.md` in the current repository. Create the directory if needed; never overwrite an existing report. Include a concise title, environment/version, reproduction steps, expected and actual behavior, and relevant evidence. Redact credentials, tokens, private URLs, customer names, and unrelated-product details before any upload.
2. Read the target with `issues_repo="$(cas config get issues.repo 2>/dev/null || true)"`.
3. If `issues_repo` is empty, **do not guess a target**. Keep the local report and follow the durable fallback below. Tell the user to run `cas config set issues.repo owner/repo`.
4. If `command -v gh >/dev/null 2>&1` fails because `gh` is not installed, keep the report and use the durable fallback.
5. If `gh auth status` fails because `gh` is not authenticated for the target host, keep the report and use the durable fallback.
6. Only after those checks, file the staged body:

   ```sh
   gh issue create \
     --repo "$issues_repo" \
     --title "<concise CAS-system bug title>" \
     --body-file "docs/requests/BUG-<slug>.md"
   ```

7. Capture and report the issue URL. Remove the local staging file only after `gh issue create` succeeds and the URL is known. If the command fails for any reason, preserve the file and use the fallback.

## Durable fallback

Unset configuration, `gh` not installed, `gh` not authenticated, and GitHub command failures all take the same safe path: preserve `docs/requests/BUG-<slug>.md` in the current repository and make it visible to collaborators.

If committing is within the current task's authorization:

```sh
git add "docs/requests/BUG-<slug>.md"
git commit -m "docs: preserve CAS bug report"
```

If a commit is not authorized or cannot be created, stop and tell the user the exact uncommitted path and why GitHub filing failed. Do not claim the report was filed, delete it, or continue silently. A local fallback that is neither committed nor explicitly handed off is still a lost report.
