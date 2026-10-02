# Process-state test lint (rule-026)

`python3 scripts/check-test-env.py` scans Rust sources in every member of the
root Cargo workspace. It checks tracked files and untracked files that Git
would admit. It tokenizes source without compiling or executing Rust.
`release-gate.sh --fast-rows --base <commit>` and the full release gate require
the `test-env` row. Fast mode supplies `--changed-since <commit> --changed-paths`:
it analyzes every Rust path in crates affected by changed or deleted Rust files,
including untracked files. Keeping the entire affected crate preserves unchanged
helper definitions and callers of changed helpers. A scripts-only or web-only
lane analyzes no Rust paths. Lint implementation, baseline, or Cargo manifest
changes require the full inventory. Without `--changed-paths`, the lint still
scans the whole workspace.

Fast mode runs the lint's fixture suite when its implementation or fixtures
change. The full release gate always scans the whole workspace and runs that
suite. Scoped admission checks stale baseline entries only in audited paths,
including deleted sources; baseline growth and disposition checks retain their
complete Git history comparison.

Tests and test support that mutate the process environment or current directory
must use the canonical `TestEnvGuard` setters (`set`, `remove`, and
`set_current_dir`). Raw `std::env::set_var`, `remove_var`, and `set_current_dir`
calls remain findings even while a guard is held: serialization does not capture
the prior value for restoration on panic. The diagnostic keeps the historical
`unguarded-mutation` kind so existing exact finding identities remain stable.
Another mutex, an independently named
RAII restore helper, or nextest process isolation cannot establish that ownership
for this lint. A direct guard parameter is an ownership witness; an optional
parameter or a fallible/optional returned guard is insufficient. `Command.env`, `env_remove`, and `current_dir` affect
a child process and are outside this check.

The scanner recognizes unit-test attributes, `cfg(test)` modules, integration
tests, test-support paths, and named helpers reached from tests. It follows
qualified helpers across files in the same crate and imported aliases. Bare
helper calls resolve explicit `crate::module::*` imports to that module;
relative or unresolved imports retain conservative crate-level resolution.
Simple local declarations and function
parameters shadow bare helper names within their lexical scope; a declaration
does not shadow its own initializer. Local closure bodies remain checked for
process mutations. It
tracks guard constructors, returned guards, lexical scopes, explicit `drop`,
and same-thread callbacks. A second constructor while an owner remains live,
including a constructor reached through a helper, is a violation. Each unsafe
test-helper call also gets a caller-site identity: allowing a legacy helper
mutation does not allow a new test to call that helper, even under a held guard.
A reviewed raw mutation in production source remains a finding at its source,
but does not propagate the same known hazard to every test caller. This narrow
policy excludes `cfg(test)`, `tests/`, and other recognized test support; their
caller-site ratchet remains intact. Changed or unreviewed production mutations
still propagate. For example, the public retrieval-evaluation API's existing
environment hazard remains recorded at `NeutralHookEnv` until cas-7cc95 injects
its environment; its test callers do not receive duplicate allowances. Thread/task
spawn closures and named spawn callbacks start without the parent's ownership;
this only models ownership, and does not prove that a parent waits safely.
Comments and string literals never become calls or grant exceptions.

## Strict baseline

`scripts/test-env-baseline.json` records each exact legacy finding with its
reason. The initial seed on epic `175f78c5` contained 1302 legacy findings and
10 exact exceptions. Correcting local closure name resolution on the combined
epic removed 236 false-positive allowances: pure `run_command`/`run` closures
had incorrectly reached unrelated production CLI functions. The retained
inventory contains 1066 legacy findings (284 direct mutation sites and 782
calls reaching unsafe helper or restoration paths) and the same 10 exceptions.
The new artifact-write test uses the shared guard rather than gaining an
allowance. The
legacy entries preserve existing tests while their fixture lifetimes are
converted. They do not certify those patterns as safe.

An identity contains the file, named function, violation kind, call-token hash,
and occurrence within that function. Arguments are included; comments,
whitespace, and unrelated line shifts are excluded. Diagnostics also print the
current line. Repeating an identical call creates another identity.

Both new findings and stale baseline entries fail. Fix a violation and prune
its exact entry in the same change. Every entry requires a nonempty reason;
duplicates and invalid schemas fail. Git comparisons reject baseline growth
or a change between legacy and exception dispositions against HEAD, every
retained baseline revision, and the supplied lane base. Thus committing an
allowance, or reintroducing a previously pruned allowance, cannot turn a new
violation green. The first installation has no older baseline; subsequent
admission uses its committed history. No inventory command edits the baseline.
Use `--inventory` for review evidence, never as an automatic acceptance step.

The nine canonical exceptions are the precise environment/cwd mutation sites
inside `TestEnvGuard` and `AmbientEnvRestore`. Setters and restoration run while
the shared lock is held; the ambient seed helper takes that same lock before a
guard-invariant test constructs its guard. No whole-file exemption exists, and
ordinary tests in the canonical file cannot use implementation exceptions.

The tenth exception is the exact second constructor in the ignored
`test_support::nested_test_env_guard_panics_with_clear_message` child. Its
parent re-execs that exact test with `--ignored` and kills it after five seconds
if it hangs. The child's expected panic is the behavior being tested. A
subprocess exception must name an exact finding, include a reviewed isolation
reason, and belong to an ignored test. An inline `subprocess-only` comment or
an ordinary test that launches a subprocess does not grant permission to
mutate its own parent's state. Future exceptions need a reviewed policy
change; the ratchet rejects additions automatically.

## Verification and analysis boundary

`python3 scripts/test-check-test-env.py` exercises the three incident shapes
(cas-da84, cas-4a8e, cas-4cf0), scopes, aliases, explicit drops, returned guards,
callbacks, threads, child command APIs, exact exceptions, and real-Git baseline
admission/pruning/growth. It parses fixture strings; it never runs their Rust.
`scripts/test-fast-release-rows.py` exercises the real gate against committed
miniature Git workspaces with a Cargo tripwire, including newly unsafe tests
in another crate, nesting, missing checker, baseline growth and stale entries.

This is a conservative source lint, not Rust name resolution or a borrow
checker. Test-support helpers are checked independently; a shared guard parameter
witnesses ownership but cannot excuse a raw mutation. Reachable fixture owners include their restoration
methods. Unknown local receiver methods are conservatively matched to local
implementation methods. Such sites can require a fixture conversion even when
a caller currently happens to hold a guard. Arbitrary macro expansion,
function-pointer dataflow, conditional/destructuring pattern name resolution,
generated/include-only source outside workspace
members, indirect trait dispatch, value moves, and full control-flow proofs
are outside the analysis. Production-only process initialization is excluded.
The canonical guard's runtime nesting panic remains the defense for paths that
source analysis cannot resolve. Rust runtime verification belongs to epic
assembly; passing these source fixtures does not claim a Rust execution result.
