# Violet thread outcomes

| Field | Contract |
| --- | --- |
| First two lines | A thread succeeds only when receipts confirm the root and every reply; a stopped thread identifies its failed index. |
| Scannable | Existing posted-message rows retain each index and Slack message ID, including after a readability refusal. |
| Readable | Readability diagnostics name the root or reply and list the indices already posted. |
| Machine output | One JSON document carries reconciled `ok`, `posted`, `failed_index` and `resume_safe`; readability detail adds `part_index`, `part` and `posted_indices`. |
| Omitted | No automatic writes or retries reconcile an uncertain result; existing file-attachment confirmation remains required. |

All drafts receive the existing local readability checks before connecting.
Whole-thread preflight for the hub's remaining rules is tracked in
Richards-LLC/violet_ps#113; the CLI does not invent an unsupported validation call.

## Verification

Scripted-hub regressions exercise stale success flags, incomplete delivery and
local or partial hub readability refusals. The real-binary regression parses
the entire stdout stream on a thread refusal. Existing terminal rows retain
their layout; user-path review belongs to release batch QA.
