---
metadata:
  managed_by: cas
---

# Test examples and audit patterns

Adapted from mattpocock/skills `tdd/tests.md`, MIT © 2026 Matt Pocock.
See [LICENSE](../LICENSE) for the permission notice. These Cassy-owned examples
use repository APIs; anti-examples and snippets labelled schematic are teaching
material, not tests to install unchanged in another project.

## Contents

- [Public behavior and persistence side channels](#public-behavior-and-persistence-side-channels)
- [Real Git fixtures](#real-git-fixtures)
- [Environment and cwd isolation](#environment-and-cwd-isolation)
- [Independent expectations and missing assertions](#independent-expectations-and-missing-assertions)
- [Hub-web: behavior versus internal collaborators](#hub-web-behavior-versus-internal-collaborators)
- [Tests that pin unsafe behavior](#tests-that-pin-unsafe-behavior)
- [Source and prose audits](#source-and-prose-audits)

## Public behavior and persistence side channels

Name one observable capability per test and assert through the interface its
caller uses. Multiple assertions can describe that capability; independent cases
can share a small table. A behavior-preserving refactor should keep the test green.

This Rust unit-test example uses the public store contract, with a real temporary
SQLite store. Imports inside the crate use `crate`; integration tests use `cas`.

```rust
use crate::store::{init_cas_dir, open_task_store};
use crate::test_support::TestEnvGuard;
use crate::types::Task;

#[test]
fn added_task_can_be_retrieved() {
    let env = TestEnvGuard::temp_home();
    let project = env.home().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let root = init_cas_dir(&project).unwrap();
    let store = open_task_store(&root).unwrap();
    let task = Task::new("cas-abcd".into(), "Document test seams".into());

    store.add(&task).unwrap();

    let retrieved = store.get("cas-abcd").unwrap();
    assert_eq!(retrieved.title, "Document test seams");
}
```

Anti-example (schematic): after `store.add`, query a private SQL connection with
`SELECT title FROM tasks WHERE id = 'cas-abcd'` and assert the row. That can pass
while `store.get` cannot retrieve the task. Test SQL directly only when the SQL
schema or migration itself is the contract under test. Likewise, a store test
proves the store contract; test the real MCP handler to claim MCP retrieval works.

## Real Git fixtures

Exercise Git operations against a disposable repository rather than teaching a
mock subprocess which commands it should see. This example uses
`cas-cli/src/worktree/git.rs`'s public `GitOperations` seam and checks a caller's
ability to detect uncommitted edits.

```rust
use crate::test_support::TestEnvGuard;
use crate::worktree::GitOperations;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git").args(args).current_dir(repo).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn edited_tracked_file_is_reported_dirty() {
    let env = TestEnvGuard::temp_home();
    let repo = env.home().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.test"]);
    std::fs::write(repo.join("note.txt"), "before\n").unwrap();
    git(&repo, &["add", "note.txt"]);
    git(&repo, &["-c", "commit.gpgsign=false", "commit", "-m", "fixture"]);
    let operations = GitOperations::new(repo.clone());
    assert!(!operations.has_uncommitted_changes(&repo).unwrap());

    std::fs::write(repo.join("note.txt"), "after\n").unwrap();

    assert!(operations.has_uncommitted_changes(&repo).unwrap());
}
```

Anti-example (schematic): return canned `git status` output from a mocked
`Command`, then assert it received `status --porcelain`. That proves command
choreography, not that an edit is detected. Use real commits, branches and merge
conflicts when testing delivery ancestry; a boolean fake cannot represent the DAG.

## Environment and cwd isolation

Use one `TestEnvGuard` for each test touching process env/cwd (rule-026), and pass
it into helpers instead of nesting guards. It serializes mutation, restores state
on unwind, retains temporary HOME and scrubs ambient Cassy/factory variables.
Tests reaching host stores need a temporary HOME, not just a project `.cas`.
This unit-test fixture asserts root resolution through the public context seam:

```rust
use crate::store::CasContext;
use crate::test_support::TestEnvGuard;

#[test]
fn explicit_root_wins_over_current_directory() {
    let mut env = TestEnvGuard::temp_home();
    let root = env.home().join("chosen/.cas");
    let elsewhere = env.home().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    env.set("CAS_ROOT", &root);
    env.set_current_dir(&elsewhere);

    assert_eq!(CasContext::from_cwd().unwrap().root(), root.as_path());
}
```

Anti-example: call `unsafe { std::env::set_var(...) }` and
`std::env::set_current_dir(...)`, then restore them only at the end. A panic skips
cleanup and parallel tests race. Integration harnesses share the guard via
`#[path = "../src/test_env_guard.rs"] mod test_env_guard;`, as in
`cas-cli/tests/task_update_verification_type_test.rs`.

## Independent expectations and missing assertions

Take expected values from a specification, independently worked example,
known-good literal or external contract. Repeating production's algorithm can
repeat its bug. A hand-derived snapshot with the same formatting/mapping logic
has the same problem; review public-output snapshots against the actual contract.
These schematic anti-examples use hub-web's `backoffDelay`:

```typescript
// Anti-example: duplicates the exponential calculation and jitter formula.
const expected = Math.round(Math.min(30_000, 1_000 * 2 ** attempt) * (0.8 + sample * 0.4));
expect(backoffDelay(attempt, () => sample)).toBe(expected);

// Anti-example: exercises no retries or exhaustion behavior.
expect(MACHINE_RETRY_CEILING_MS).toBe(MACHINE_RETRY_CEILING_MS);

// Anti-example: a successful return need not contain the promised result.
it("reports a retry delay", () => { backoffDelay(2, () => 0.5); });

// Good: independently worked 4-second delay at the midpoint jitter sample.
expect(backoffDelay(2, () => 0.5)).toBe(4_000);
```

A constant compared with a restated literal is a change detector, not proof of
retry behavior. A no-panic test needs an explicit no-panic contract; error or
rejection tests must inspect the returned error rather than discard a `Result`.

## Hub-web: behavior versus internal collaborators

Use Vitest against the exported capability, as in
`hub-web/src/connection-state.test.ts`. Keep `backoffDelay` real and control only
its randomness seam. Copy the good example beside `connection-state.ts`:

```typescript
import { expect, it } from "vitest";
import { backoffDelay } from "./connection-state";

it("caps retry delays at thirty seconds with midpoint jitter", () => {
  expect([0, 1, 2, 5, 6].map(attempt => backoffDelay(attempt, () => 0.5)))
    .toEqual([1_000, 2_000, 4_000, 30_000, 30_000]);
});
```

Anti-example (schematic): replace an internal delay calculator and assert only
that it was called. The public result could still be wrong or missing:

```typescript
const calculator = { delay: vi.fn().mockReturnValue(4_000) };
const retry = makeRetryScheduler({ calculator }); // hypothetical internal seam
retry.nextDelay(2);
expect(calculator.delay).toHaveBeenCalledWith(2);
```

A private-method test or an internal-call-count test freezes the decomposition.
For a separately owned boundary whose request is itself the contract, assert that
request and the caller-visible result; see [mocking.md](mocking.md).

## Tests that pin unsafe behavior

Choose the authorized outcome before writing expectations. A green test can
preserve a security bug. This schematic handler pair illustrates the distinction:

```rust
// Anti-example: freezes the existing defect just because it is observed today.
assert!(send_message(&read_only_device, "hello").is_ok());

// Good: the protocol contract refuses a device without message:send authority.
assert_eq!(send_message(&read_only_device, "hello"), Err(SendError::Forbidden));
```

Use the real request/handler and assert no delivery occurred through its supported
observable seam. Do not bypass authorization or weaken the assertion to make green.
Snapshot/phrase updates must preserve the intended refusal, not merely match the
latest output. The names above are illustrative, not existing Rust APIs.

## Source and prose audits

| Pattern | Why it fails as behavioral proof | Replacement or intentional contract |
| --- | --- | --- |
| Reading `.rs` source with `include_str!`/filesystem reads and asserting text | Correct text can exist in dead or unwired code. | Exercise the exported behavior. For genuine cross-file wiring, keep a structural check with a specific `// pin: <reason>`. |
| Comparing `.find()` positions or asserting source line order | Source order can change without changing execution order. | Send inputs through the real handler and assert effects; pin textual order only if a source consumer requires it. |
| Asserting a phrase in a skill/doc | Wording can change without behavior changing; the phrase cannot prove execution. | Check routing/installation/parser behavior, or use a reasoned contract registry for tokens an actual consumer requires. |

In the Cassy source repository, `scripts/check-test-shape.py` detects mechanical
source-as-text and constant/literal violations; `--changed-since <base>` scopes
its audit. Other projects use their equivalent lint. A reasoned pin names the
consumer, not a wording preference. Put prose pins in the contract registry rather
than adding scattered literal assertions.
