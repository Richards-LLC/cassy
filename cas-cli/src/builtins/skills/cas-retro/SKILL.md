---
name: cas-retro
description: Use when explicitly asked for a post-release or weekly environment retrospective that turns Cassy evidence into improvement tasks; not for release notes or an individual bug fix.
disable-model-invocation: true
license: MIT
metadata:
  managed_by: cas
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills retro (MIT, © 2026 Matt Pocock).
---

# Environment retrospective

Imported and adapted from mattpocock/skills `retro`, MIT © 2026 Matt Pocock.
See [LICENSE](LICENSE) for the upstream permission notice.

Run only on an explicit invocation. The supervisor invokes this after each
release and for a weekly review; this skill installs no schedule. Improve the
agent's environment by filing work, rather than patching it during the review.

## Procedure
1. Read `cas-writing-for-agents`. Bind the review to a project, epic and release
   run directory, or the requested weekly interval. If unspecified, use the
   current project's latest completed release; record its version, tested SHA,
   time bounds and exact run path in an epic note. If evidence cannot identify
   that scope, ask the invoking supervisor for it before filing scoped tasks.
2. Collect primary evidence, retaining failures from earlier attempts instead
   of trusting only the final green log. Read release task notes, especially
   `request_changes` reasons, and `verification action=list task_id=<id>`
   records. Read assembly/CI failures, `gate.log`, `rows/*/*.log`,
   `interventions.log`, stage/assembly receipts, and QA ledgers' `NOT EXERCISED`
   rows. Read `rule action=list_all` drafts and existing encode-as-check chores.
   Follow references to original logs; an intervention's stage label alone
   does not identify its cause. Record unavailable sources as evidence gaps.
3. Examine all seven categories below. For each candidate, cite a task/verdict
   ID or a file with line/row, build revision and observed outcome. Separate
   observation from inference; one occurrence suffices for a mechanical check.
   Consult the existing lint/check commands, hooks and CI wiring first: a check
   that exists but is unwired or broken calls for repair, not a second check.
4. Rank by demonstrated impact, then recurrence. Match candidates to existing
   open and closed tasks by cause and proposed mechanism using `task action=list`
   and `task action=show`; include `status=closed`, prior releases and active encode chores.
   Append new evidence to an open match. For a closed match, verify whether the
   failure preceded its fix; record already-fixed cases on the epic. File a new
   regression task only for evidence after the fix. A QA coverage gap is missing
   proof, not proof that the product failed; scope its task to obtaining proof.
5. File each uncovered improvement with `task action=create`, in the bound
   project and epic, without starting or assigning it. Set a concrete title,
   priority, type, category label (`retro-navigation`, `retro-automated-checks`,
   `retro-coding-standards`, `retro-agents-md`, `retro-tool-economy`,
   `retro-no-ops`, or `retro-information-access`) and `retro` label. Include:
   **Category**, **Evidence** (source and revision), **Mechanism** (entry point
   and enforcement), **Acceptance** (observable result and failing fixture),
   and **Related work** (duplicates, rule IDs or dependencies). Put acceptance
   in `acceptance_criteria` too. Keep distinct mechanisms separate; merge
   repeated symptoms of the same mechanism. Do not promote draft rules here.
6. Add one short `task action=notes` summary to the epic: scope, new task IDs,
   reused task IDs, already-fixed findings, categories with no finding, and
   unavailable sources. Return the task list in severity order. The tasks and
   epic note are the output; write no prose report file.

## Categories and mechanisms
| Category | Evidence to seek | Proposed mechanism |
| --- | --- | --- |
| Navigation | Repeated searches, hidden cross-file dependencies | Repair an existing map or add a navigation pointer. |
| Automated checks | Mistakes that lint, typing, tests or filesystem checks could catch; missing guardrails | Wire or repair the cheapest deterministic check at the earliest applicable boundary. |
| Coding standards | A reviewer missed a violation | Mechanical syntax/API/import/location violations get deterministic checks; reserve `CODING_STANDARDS.md` for judgement calls such as cross-file consistency. |
| AGENTS.md | Large project/global steering repeats enforceable rules | Move enforcement to checks and review judgement to standards; keep short navigation pointers in always-loaded instructions. |
| Tool economy | Expensive repeated calls, builds, oversized tool output | Measure the cost and propose caching, scoped calls or a smaller tool response. |
| No-ops | Steering that cannot change an observable decision | Remove redundant/stale instructions with an example showing unchanged enforced behavior. |
| Information access | Missing logs, inaccessible evidence, unexercised QA conditions | Expose a bounded read-only source or instrument the missing receipt/log; state access limits. |

Reviewers own judgement standards: they receive the diff with less exploration
pressure. Implementation instructions stay small. Treat steering text as
pointers to existing checks and references, rather than accumulating reminders.

For a historical replay and expected deduplicated task list, use
[references/v3.38.0.md](references/v3.38.0.md).
