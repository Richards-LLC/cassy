# Request Intake and Archive

> **Deprecated for new outbound actionable requests.** File Cassy defects in
> [`Richards-LLC/cassy`](https://github.com/Richards-LLC/cassy/issues) and file requests for
> a Richards-LLC-controlled team directly on that team's GitHub issue board
> (for example, [`Richards-LLC/petra-stella-cloud`](https://github.com/Richards-LLC/petra-stella-cloud/issues)). Never write, commit, or push in another team's checkout. Save a Cassy memory receipt with the issue URL, one-line ask, and date. Examples: cloud-to-Cassy GH #215; Cassy-to-cloud [#44](https://github.com/Richards-LLC/petra-stella-cloud/issues/44).

This directory is a historical archive and legacy inbound-response reader. It remains acceptable for prose-heavy specifications or design documents until cross-project task proposals ship.

## Legacy staged files

The destination is project-local configuration in `.cas/config.toml`:

```toml
[issues]
repo = "owner/repo"
```

Set or inspect the `issues.repo` key with:

```sh
cas config set issues.repo owner/repo
cas config get issues.repo
```

Do not create a new staged report for actionable work. This material only explains how to preserve and sweep a file that was already staged under the former workflow.

## Legacy write-first flow for existing staged files only

This former workflow is retained solely to migrate a report that already
exists; it is not authorization to create a new outbound request file.

1. Write the complete, public-safe report to a new, uniquely named staging file in this directory:

   ```text
   BUG-<lowercase-kebab-slug>.md
   FEATURE-<lowercase-kebab-slug>.md
   ```

   Never overwrite an existing report. Before upload, remove credentials, tokens, private URLs, customer names, and unrelated-product details.

2. Read `issues.repo`. If it is unset, do not guess a destination; preserve and commit the staging file as described below.

3. If `gh` is installed and `gh auth status` succeeds, file the staged report:

   ```sh
   issues_repo="$(cas config get issues.repo)"
   gh issue create \
     --repo "$issues_repo" \
     --title "<concise request title>" \
     --body-file "docs/requests/BUG-<slug>.md"
   ```

4. Capture the issue URL. Remove the staging file only after `gh issue create` succeeds and the URL is known. If the file is intentionally retained as a historical source, add the issue backlink instead.

## When GitHub Filing Cannot Complete

If `issues.repo` is unset, `gh` is unavailable or unauthenticated, or issue creation fails, keep the staging file. **A local file is not visible to anyone else until it is committed.** When committing is authorized:

```sh
git add "docs/requests/BUG-<slug>.md"
git commit -m "docs: preserve request report"
```

If a commit is not authorized or cannot be created, report the exact uncommitted path and the filing failure; do not claim the request was filed or silently leave it behind.

## Historical Archive

- `completed/` is the historical record for completed file-based requests. Keep it intact.
- The 11 reports migrated to GitHub Issues #55–#65 keep their issue backlinks and must not be deleted. They were moved to `completed/` on 2026-08-07 once every one of those issues closed as completed and each fix was verified against a commit on `main`; each carries a disposition note naming that evidence.
- For any committed file-only fallback that completes before migration to an issue, append its existing completion note, move it to `completed/`, and commit the move.
- A staged report leaves this directory's root only with a disposition note at its head: the GitHub issue it was filed as, or the issue/commit that already covers it. Verify a "covered" claim against the actual fix — a matching title is not evidence.
- `RESPONSE-*.md` files are replies, not reports. Archive them in `completed/` alongside the request they answer.
