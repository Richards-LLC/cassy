# Cassy Cloud contract fixtures

Recorded request/response pairs from the **production** Cassy Cloud
(`petra-stella-cloud`, after PR #153). They cover every route the 3.51 client
uses. cas-c7c7 uses them to make the Rust fake clouds match production
(`cas-cli/tests/distributed_factory_test_cases/fake_cloud.rs` and the fake
in `cas-cli/tests/mcp_tools_test/task_tools/peer_claims.rs`).

| file | routes |
| --- | --- |
| `agents.json` | register (with metadata), heartbeat with and without a metadata body, `GET /api/agents?project=` (keyset paging to the empty end page, with and without `sort_cursor`, `status=active`, `status=stale`, unknown project, no project) |
| `claims.json` | claim, same-agent re-claim, 409 conflict, canonical_id ignored, renew, lock, account locks, agent locks, takeover after expiry, renew-after-takeover 404, release, idempotent release |
| `peer_messages.json` | send, duplicate, same key with a different body, dedupe from a second sender, every send error code reachable with one account, claim (lease, `denied`, invalid lease), ack (per-item results, invalid id, re-ack), status (leased, delivered, stranger 404) |
| `teardown.json` | releases, message drain, agent shutdowns and the verification result |

## Format

Each file is one JSON document:

```text
{ "schema": 1, "group", "captured_on", "endpoint", "capture_script",
  "exchanges": [ { "group", "name", "note", "expect",
                   "request": { "method", "path", "query", "headers", "body" },
                   "response": { "status", "body" } } ] }
```

`expect` is the status the documented contract predicts. It is `null` where
the outcome is only being observed: `send_dedupe_key_conflict` and the
teardown calls.

Values are normalised, and equal values share one placeholder, so relations
survive normalisation:
- `<agent-a>`, `<agent-b>`, `<agent-c>`: the scratch agents
- `<project>`: the throwaway canonical id
- `<task-key-1>`, `<task-key-2>`: claim keys
- `<factory-session>`, `<consumer>`, `<host>`
- `cc-<run>-…`: other run-scoped names
- `<ts-N>`: timestamps
- `<uuid-N>`: server ids (message ids, lock ids, user, team and project ids)

The bearer token is always `Bearer <redacted-token>`, and the account email
is `<redacted-email>`. The 64 001-character `send_invalid_request_body`
request body is elided as `{"_oversized_body_bytes": N}`.

## Limits of one account

- Task locks and peer-message status are visible only to the same user
  account, so a cross-account conflict, the cross-account
  `dedupe_key_conflict` and a true stranger's status read cannot be captured
  with one account.
- `status_stranger` uses an unknown id. The cloud deliberately makes that
  indistinguishable from a message the caller may not see.
- `invalid_project` (400) was not reached with one account:
  - an unknown but well-formed project id, a malformed id and a bare URL all
    answered 404 `recipient_not_found`;
  - an empty project id answered 400 `invalid_request`.
  The three attempts are recorded as `send_unknown_project`,
  `send_invalid_project` and `send_invalid_project_url`.
- The same `dedupe_key` from a second sender of the same account answers 200
  `duplicate` with the original id. The 409 `dedupe_key_conflict` needs a key
  another account already used.

## Where production differs from the contract handoff

The handoff text (`cloud-contract-2026-10-13.md`) and production differ, and
the fixtures record production:
- **Claim 409:** the body keeps the legacy envelope `status: "conflict"`,
  `result: null`, `lock: null`. Its `agent_id` names the holder, as
  `owner_agent_id` does.
- **Agent paging:** `sort_cursor` is not needed with the default order.
  - `next_sort_cursor` equals `next_cursor` on every page captured.
  - A page requested with only `cursor` and `cursor_id`, as the 3.51 client
    does, is identical to one that also sends `sort_cursor`.
  - A full last page still returns `next_*` cursors; the end of the list is
    an empty page with null cursors.
- **Ack:** a non-UUID ack id rejects the whole request with 400
  `invalid_request`. An unknown UUID is a per-item 404, and a re-ack is a
  per-item 409.

## Re-capturing

```bash
python3 -I scripts/cloud-contract-capture.py          # about 6 minutes
python3 -I scripts/test-cloud-contract-fixtures.py --token-file ~/.cas/cloud.json
```

- **What the capture does:** it reads `~/.cas/cloud.json` once, read-only (override with
  `--credential`), and copies the token to a 0600 file under
  `/mnt/rewind/cas/scratch/cloud-contract-<run>/`. The token is never
  printed. It then works in a throwaway project,
  `github.com/petrastella-live-check/qa-<run>`, with scratch agents
  `cc-<run>-a|b|c`.
- **Teardown:** this always runs, even after a failure. It releases every
  claim, acks every claimed message and rejects anything still queued. It
  shuts every agent down, deletes the token copy, and writes the
  verification to `teardown.json`. Shutdown is production's removal surface:
  the agent rows remain with status `shutdown`.
- **The stale wait:** about five of the six minutes are spent waiting for
  agent c's heartbeat to read `stale`. `--skip-stale` skips the wait for a
  trial run. Point a trial at a scratch directory with `--out`, because the
  self-test requires the stale exchange.
- **After an interrupted run:** run
  `scripts/cloud-contract-capture.py --cleanup-run <run>` to drain, release
  and shut down that run's leftovers. A message leased when the run died
  becomes claimable once its lease (120 s) expires.
- **Exit status:** the capture exits 1 if teardown could not be verified. It
  lists any status that differed from the documented contract, and those
  differences are findings for cas-c7c7.
- **Checking the result:** the self-test checks the schema, coverage, recorded
  statuses and error codes, the contract relations cas-c7c7 depends on, and
  redaction. Pass `--token-file` to also scan for the exact token.
