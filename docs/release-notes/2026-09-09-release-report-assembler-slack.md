# Slack draft — release report assembler fixes (main merge, PR #790)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: the new release-report command's first real run came out with placeholder "Was" lines, cut-off sentences, and a project named after a folder. Now: it reads your project settings from any checkout, keeps every changelog sentence whole, and tells the Was → Now story from your own release notes.

Reply:
• Settings found from any checkout — Was: run from a secondary checkout, the command could not find the project's issue tracker setting, so it reported no issues, no release, and a project named after the folder. Now: it resolves the project store the same way the rest of Cassy does and names the project from its settings.
• Whole sentences — Was: a changelog entry that wrapped onto a second line was cut at the first line. Now: wrapped entries are joined before they are rendered.
• Your release notes, not placeholders — Was: every article said "the prior behavior is not stated" and the user and developer sections were identical. Now: the Was → Now bullets from the release-notes draft fill the report, split into what a user sees and what a developer changed, with the changelog as fallback.
• PDF without setup — Was: the PDF step stopped if the browser automation module was not installed locally. Now: it fetches a disposable copy and continues.

## Dev thread

Top-level:
Live on production — Dev — Was: `cas release report` loaded config from `project_root/.cas` only, split changelog bullets per line, hard-coded a placeholder Was, fed both audience sections from the same entries, and exited when `require('playwright')` failed. Now: store detection via `find_cas_root_from`, joined bullets, draft Was/Now articles partitioned User/Dev, preserved inline-code headings, and a disposable Playwright install fallback (PR #790).

Reply:
• Config resolution — Was: `acquire_sources` read `project_root/.cas` directly, so a git worktree saw no `issues.repo` and named the project from the directory. Now: one `find_cas_root_from` lookup and `[project].canonical_id` for the name.
• Changelog parser — Was: `parse_changelog_section` recorded each `- ` line alone. Now: continuation lines are joined into the bullet.
• Article source — Was: `was_now_sections` used changelog entries with a hard-coded placeholder Was and fed both audience sections from the same selection. Now: the draft's Was/Now bullets become articles partitioned by thread, with per-section changelog fallback; inline-code headings keep the opening backtick.
• PDF fallback — Was: `PDF_SCRIPT` exited on `require('playwright')` failure. Now: on MODULE_NOT_FOUND the command performs a disposable `npm install --no-save playwright` and retries.

Proof: release_report unit 11/11; release_report_test 2/2 including a git-worktree fixture; terminal-qa PASS 12 runs; real v3.22.0 `--pdf` run, 5-page A4. Follow-up cas-ca80 in progress. PR #790.

## POSTED
Posted 2026-09-09 17:56Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788976548.834519 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788976548834519 (reply ts 1788976557.259729)
- Dev top-level ts 1788976560.075769 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788976560075769 (reply ts 1788976573.004119)
