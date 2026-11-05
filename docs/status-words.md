# Status words and their evidence

Cassy status text describes the observation the tool made. A local ref is a
cache of origin, a queue write is an enqueue, a committed mutation can still
have unfinished follow-up work, and integration does not establish publication
or installation. Each observation applies at the time it was made.

This is phase one of the communication honesty audit: tool and CLI output.
The supervisor/worker instruction changes, evidence checks on model-written
messages, and historical conversation replays are separate work. The
`worker_status` Git badges and shutdown/finish recovery are owned by the
companion worker-status fix; their implementation is not changed here.

## Definitions

| Word | Meaning and check | When that evidence is missing |
| --- | --- | --- |
| **pushed** | Git acknowledged a push of the named commit to the named destination, or a live `git ls-remote --heads origin refs/heads/<branch>` answered with that commit. Cloud sync counts instead mean successful records acknowledged by the sync operation, not Git commits. | A matching remote-tracking ref alone is described as **cached**. A failed or timed-out publication attempt says **publication unconfirmed**, with the error and cached tip; it cannot establish that the remote is behind or that no update occurred. |
| **clean** | A successful `git status --porcelain` observation found no relevant tracked or untracked changes. The scope is Git-visible files, with its normal ignore and index rules; it does not prove a task is complete or a branch is published. | Assignment's `--untracked-files=no` probe says **tracked files unchanged**. An unreadable status is unknown. The worker-status companion fix owns the badges' scope and failure behavior. |
| **done** | Only the specifically named operation or phase completed: for example, an update phase returned `Ok`, or a list handler finished producing its list. Requirements saying work must be done are prospective instructions, not observations. | A write with follow-up still running says **write committed**. A stale inbox instruction says **no longer current**, with its task/Git reason; a task in progress or cancelled is not described as done. Hook cleanup says **processed**, retaining `dry_run` and error counts. |
| **merged** | The named source is integrated into the named target, proven by Git ancestry or the existing qualified content-equivalence/evolution checks. A successful merge operation includes its commit and a separate publication result. Memory/account merges instead refer to successful store/file combination operations. | Worktree lifecycle views say **recorded status: merged** when reading the store; that does not prove current branch containment. A removal timestamp says **removal recorded**, not merged. Cleanup's safety probe says **tip reachable from local refs**. |
| **confirmed** | The particular postcondition was observed. Context reset requires a new session transcript recording `/clear`, and prints the session transition and evidence path. Operator confirmations identify the operator's decision, not execution of the requested action. | Spawn enqueue already says **NOT YET CONFIRMED**. Missing recipient evidence, reset evidence or executable evidence stays unconfirmed. Transport alone cannot confirm model action. |
| **shipped** | A claim about publication needs publication evidence for the specific release. “Source: shipped with Cassy” identifies a builtin embedded in this running binary's catalog; it is not a claim about another host or a task's release. | CHANGELOG-derived report counts say **changes listed**. Doctor's task-delivery containment check says **release-tagged issues**, explicitly naming **local release tags**. Epic/default-branch containment says **integrated**, not shipped. |
| **installed** | The named local installation was observed: service unit/plist exists, an executable was located and probed, builtin files were compared, or the updater successfully installed its versioned binary. Installation and service activity are separate fields. | An unavailable/unusable CLI or service manager retains its diagnostic. Updating the on-disk binary does not establish that an already-running daemon has restarted. |
| **closed** | The authoritative task store read returned `Closed`, or the close/QA transaction successfully wrote that state. GitHub issue closure comes from its fetched state and closure time. | A pending verification, parked delivery, cancelled task or intermediate close write is not Closed. Closing a rejected QA review completes that review task; it does not approve the implementation. |

## Where the checks live

Paths below are relative to `cas-cli/src/`. The functions identify the actual
producer or consumer, rather than asking a reader to infer a claim from its
label alone.

| Output family | Observation/check behind it | Disposition |
| --- | --- | --- |
| Merge target push and already-current receipt | `worktree/git/branch_ops.rs`: `publish_branch_to_origin_bounded`, `origin_branch_tip`, `publish_pinned_source_to_origin`; `mcp/tools/core/workflow/worktree_ops.rs`: `describe_target_push_state` | Matching cached/local tips now trigger a bounded origin read. A moved remote takes the real push path; a deleted branch stays absent; an unreachable remote is unconfirmed. No force push is introduced. |
| Merge source, QA head, recovery receipt | `mcp/tools/core/workflow/worktree_ops.rs`: `requested_merge_source_tip` and source resolution; `qa_pass`'s `pushed_branch_tip`; `mcp/tools/core/task/lifecycle/close_ops.rs`: `validate_pushed_task_receipt` | Origin-only sources use live remote reads/fetches for the named source/head. A requested local source is read locally and publication is checked separately. Local-only delivery keeps its explicit local authority. |
| Local close/park diagnostics and epic base guidance | `mcp/tools/core/task/lifecycle/close_ops.rs`: `stale_fresh_park_receipt_refusal`, `apply_awaiting_merge_anchor_advance`, `unpushed_delivery_tip_refusal`, `check_factory_branch_merge_reality_with_delivery_mode`, `run_factory_branch_merge_gate_for_delivery` | Cached origin evidence is named as cached; absence no longer claims “never pushed,” and cache presence no longer claims current remote publication. Gate decisions are unchanged. |
| Reviewed correction and parked branch publication | `mcp/tools/core/task/lifecycle/close_ops/verified_delivery.rs`: reviewed correction's fetched/local ref; `mcp/tools/core/task/lifecycle.rs`: `publish_parked_delivery_branch` | Names the measured ref for both push and local-merge modes. Successful push no longer promises that lane CI ran. |
| Early mutation reply | `mcp/tools/service/server_handler.rs`: `call_with_deadline_and_grace`; `mutation_receipt.rs`'s committed boundary | Replaces action “is done” with “write committed,” retaining the named unfinished follow-up and retry guidance. The close receipt still requires its terminal write. |
| Inbox suppression | `mcp/tools/service/agent_search_system/message.rs`: `inbox_poll`, `assignment_stale_task`, lifecycle/merge revalidation | Names an obsolete instruction and the observed task status or task/Git state. Notification IDs and settlement behavior are preserved. No completion or acknowledgement is inferred. |
| Assignment freshness | `mcp/tools/core/task/update.rs`: `clean_idle_assignment_note` | Names the successful tracked-file probe. Untracked work is not erased or represented as checked. |
| Worktree views and cleanup | `mcp/tools/core/workflow/worktree_ops.rs`: `worktree_list`, `worktree_show`, `worktree_cleanup_target`; close response's `removed_at` observation | Distinguishes recorded lifecycle status, local reachability and removal metadata from a live merge result. |
| Epic containment and regenerated catalog | `mcp/tools/core/task/lifecycle/close_ops.rs`: `collect_epic_branch_statuses_with_options`, `without_regenerated_ledger`, `run_epic_close_merge_gate_with_budget` | Describes integration or regenerated target-catalog equality; neither is advertised as shipment. Existing exact anchor/content checks remain in force. |
| Main-worktree WIP report | `hooks/handlers/session_hygiene.rs`: `porcelain_status`, `wip_candidates`; factory GC report | Successful current porcelain read includes untracked entries; failure produces no clean observation. Retained. |
| Release report | `cli/release_report.rs`: `assemble_markdown`, `changelog_changes`, publication/source notes | CHANGELOG totals and area summaries are “listed”; publication and assets retain their own evidence or explicit absence. |
| Doctor release-tag check | `cli/doctor.rs`: `shipped_issues_check`; `issue_reconcile.rs`: `shipped_open_issues`, `release_tag_containing` | Task Closed state plus delivery containment in a local `v*` tag; the output now names that limited check. The old doctor category spelling remains accepted. |
| Hook cleanup | `cli/hook.rs`: `execute_cleanup`, `find_settings_files_with_cas_hooks`, `cleanup_single_file` | No candidates in the searched/readable settings says `no_candidates_found`; completed iteration says `processed`, with dry-run/results/errors. The scan does not certify inaccessible or excluded directories. |
| Update phases and installation | `cli/update.rs`: `UpdateStepTracker::run` and post-install version convergence; `cli/hub_service.rs`: service status; `cli/setup.rs`: `service_is_ready` | “done” belongs to a successful named phase. Service installed/active are checked separately; post-install refresh is not reported as converged when the binary version differs. Retained. |
| Context-reset confirmation | `mcp/tools/service/factory_ops.rs`: `factory_clear_context`; `context_reset`'s transcript detection | Only fresh reset evidence yields CONFIRMED; enqueue/timeouts retain explicit unconfirmed results. Retained. |
| Task/QA status and close receipts | `mcp/tools/core/task/query.rs`; terminal close write and `already_closed_message`; `mcp/tools/service/qa_pass_ops.rs`: `close_qa_task` | Store-backed lifecycle state, not a release claim. Intermediate mutation receipts do not establish Closed. Retained. |
| Cloud/team sync and memory/account merge counts | `mcp/tools/service/worktree_verification_team_ops.rs`; `cli/cloud.rs`; `mcp/tools/core/memory.rs`; `cli/accounts.rs` | Successful producer results count acknowledged sync/store/file operations. They do not describe Git publication or task shipment. Retained. |
| Git badges, shutdown and finish | `mcp/tools/service/factory_ops.rs` Git-status/shutdown helpers; finish skill and supervisor command | Companion task owns this surface. This branch does not claim that companion implementation is present or verified. |

## Regression evidence

The tests exercise real handlers, stores or disposable Git repositories. The
principal failing-first cases are:

| Named test | Boundary it proves |
| --- | --- |
| `stalled_post_commit_work_after_a_note_answers_committed_and_keeps_running_cas_b1dd` | A committed task write with unfinished follow-up does not say the action is done. |
| `a_committed_merge_with_a_slow_close_reports_its_outcome_cas_3937` | A merge receipt cannot declare its pending close done. |
| `cas99d2_inbox_poll_withholds_a_consumed_assignment_gh127` | An in-progress task's obsolete assignment is named without a completion claim. |
| `cas8aee_inbox_poll_names_terminal_assignment_and_spawn_intro_gh336` | Closed and cancelled assignments retain their actual distinct states. |
| `test_clean_idle_worker_behind_target_assigns_and_starts_cas_c54f` | The tracked-file check can coexist with an untracked draft and cannot call the worktree clean. |
| `worktree_status_labels_stored_merge_without_claiming_current_containment_cas_4c6f` | A recorded merge plus later unmerged commits is labelled as recorded in both list and show. |
| `publish_branch_to_origin_is_already_current_when_nothing_to_push` | Equal cached/local tips with unchanged, moved, deleted and unreachable origin require different results. |
| `changelog_summarized_356_headings_count_changed_and_fixed_entries` | A report with changes but no publication evidence counts listed changes. |
| `shipped_issues_doctor_row_lists_open_issues_whose_fixes_shipped_cas_dd56` | Local release-tag containment is reported as such, with offline lookups still explicit. |

The task receipts record the actual failing and passing commands, commit SHAs,
test counts and any deferred assembly checks. This document is the definitions
and source map, not a claim that a release or the companion change has shipped.

## Audit boundaries

The initial literal inventory examined `cas-cli/src/mcp/tools` and
`cas-cli/src/cli`, then followed the relevant Git, queue, sync, installation and
store producers. Its 420 candidate literals across 76 files include errors,
requirements, parser vocabulary and non-status text; that count is not a count
of factual status assertions. Dynamic task/worktree status rendering was also
examined. User-supplied message bodies, historical notes, and model-written
claims are not certified by a word search.

JSON consumers of hook cleanup should accept `no_candidates_found` and
`processed` in place of `clean` and `done`; `candidates` replaces the former
incorrect zero `files_checked` field. Worktree text consumers should read the
explicit recorded-status label. Generic unsuccessful merge publication uses
`PUSH NOT CONFIRMED` rather than a definitive `NOT PUSHED`; a Git rejection
that specifically establishes non-fast-forward retains its separate result.
