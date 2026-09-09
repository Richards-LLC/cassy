# Slack draft — supervisor guidance parked in a reference (main merge, PR #791)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: three pieces of coordinator guidance (how to report, who owns a release, where to file a bug) were cut from the built-in playbook to make room and survived only in a start-up prompt. Now: they live in a reference page the playbook points to, so they are read on demand and cannot be lost to a size trim again.

Reply:
• Guidance parked, not lost — Was: the reporting-style rules, the release-train ownership note, and the detailed bug-routing paragraph existed only in a session start-up prompt after the playbook trim. Now: they sit word-for-word in a reference page linked from the playbook's "On-demand references" line, in every harness flavor.
• Room stays — Was: restoring the text in the playbook body would have eaten the headroom just recovered. Now: the body grows by one link, keeping over 2 KB of headroom under its limit, and the doctor budget row stays green.

## Dev thread

Top-level:
Live on production — Dev — Was: the cas-6a20 trim dropped the Reporting style, Release train, and Cross-team routing sections from `cas-supervisor.md` in all three flavors. Now: `cas-supervisor/references/reporting-and-routing.md` carries them verbatim, registered in the Claude/Codex/Grok catalogs, with a breadcrumb in the body; bodies 5,861 / 5,859 / 5,856 B, doctor SessionStart budget 2,530 B headroom (PR #791).

Reply:
• Reference file — Was: no home for the trimmed text. Now: `skills/cas-supervisor/references/reporting-and-routing.md` (three byte-identical mirrors).
• Catalog and breadcrumb — Was: a new reference is invisible to `cas update --sync` without registration. Now: `BuiltinFile` entries in all three catalogs; the body keeps the registry keys and the standing filing directive.
• Ledger — `reference-history.json` regenerated from committed history in a separate commit.

Proof: skill guardrails 10, flavor drift 17, agent contract 7, issue intake 2, factory parity 2, cli::factory::parity 6; `--proof builtins::tests` 92/92. PR #791.

## POSTED
Posted 2026-09-09 18:06Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788977160.516519 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788977160516519 (reply ts 1788977168.680019)
- Dev top-level ts 1788977170.820409 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788977170820409 (reply ts 1788977182.336229)
