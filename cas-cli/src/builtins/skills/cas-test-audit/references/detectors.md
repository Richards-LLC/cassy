# Detector scope and evidence

Run `scripts/audit.py` once for all classes; it is a portable lexical inventory, not a compiler or a deletion verdict. `pattern` is the stable machine identifier. Every row carries file/line, test or helper scope, evidence, confidence and a proposed fix. Comments, strings and Rust raw strings are masked before code matching. Native test attributes, test modules, conventional test files and JS test callbacks define scope; production code outside them is excluded.

| Pattern / class | Signal | Review and fix |
| --- | --- | --- |
| fixed-sleep / flake-prone, slow | Rust thread/Tokio sleep, JS setTimeout, Playwright waitForTimeout | Fixed readiness guesses wait on the event. Recognized fake timers and event-bounded predicate polling are excluded; aliases need review. Runtime saving is unmeasured until CI timing proves it. |
| instant-port / flake-prone | One connection-must-fail assertion | Observe the server's shutdown/ready event; a refused attempt alone cannot establish lifecycle completion. |
| instant-teardown / flake-prone | Kill/stop/close followed by a one-shot process/file assertion without a wait/join | Await the exact completion or cleanup event. Synchronous owned-file operations are not teardown races. |
| tight-deadline / flake-prone | Small real-time upper bounds or event timeouts, including elapsed assertions | Review the awaited event's own budget. Test timing contracts with controlled clocks; runner timeouts are safety bounds, not event assertions. Lower bounds are not upper deadlines. |
| shared-port, shared-path / flake-prone | Literal/global fixed listener ports; writes to shared temp paths | Use port zero with retained listener ownership or per-test temporary directories. A fixture string/pure path builder is not shared I/O. |
| shared-state, order-dependence / flake-prone | Unguarded process environment, static mut, or mutation of a binding shared by test callbacks | Reset with a full-duration guard or allocate state per test. Helpers' local variables and existing reset hooks are not global state. |
| unseeded-random / flake-prone | Unseeded rand/Math.random/random inputs | Inspect whether the expectation depends on the draw. Seed varying test data; random cryptographic challenge round-trips may be invariant and are not proven flakes. |
| disabled-test / dead candidate | ignore/skip/todo | Check runner registration, subprocess helper callers and retirement history. Disabled does not mean dead. |
| duplicate-body / duplicate candidate | Identical normalized bodies within one file | Quote both assertion paths and imports; check differing enclosing fixtures before merging. |
| wording-assertion / wording-pinned | Human wording asserted on output/error channels | Check stable codes or behavior instead. Assertion diagnostic messages, exact caller payloads and protocol data are not wording preferences. |
| style-assertion / style-pinned | CSS/source appearance assertions | Keep DOM/behavior checks and documented named accessibility-property contracts; visual QA owns appearance. Source metadata or exact preserved files may be data contracts. |
| no-assertion / restates-implementation candidate | Trivial test body with no assertion or observable call | Trace helper/exception assertions before deletion. Compiler/caller evidence still owns dead or superseded classifications. |

The scanner does not expand macros, resolve aliases/imports, parse JS template interpolation or prove production reachability. Its signal is deliberately a **candidate**. Hand review owns semantic distinctions and records false negatives as well as false positives. Superseded tests need the actual retiring commit; an automated skip flag cannot supply it.

## Historical pass

The same invocation reads thirty-day GitHub run and issue metadata without rerunning jobs or changing issues. Grouping uses workflow/event and the identical SHA, or a locally measurable identical Git tree. Aggregate status flips remain low confidence until a named native failure and executed pass are inspected. Reruns, queue ejections and issue reports each retain their own evidence and uncertainty.

Supplement the bounded metadata with native artifacts, current skip/exclusion configuration and Cassy incident notes using `--history-evidence`. Include source URLs/paths, SHA/tree, run attempt, native identity, outcome and confidence. A cap, absent credentials or missing logs is missing evidence, never zero historical flakes. Only original native evidence establishes a high-confidence historical flake; history-reported and repaired/deleted cases remain separately labelled.

## Browser smells and silent green

Treat every match below as a review candidate; sample 10 and retain the precision floor from the skill. A matcher name alone is not a defect.

| Candidate detector | Confirmed defect | Replacement / evidence |
| --- | --- | --- |
| CSS/Quasar class locators | Styling structure is the primary identity | Inspect role/label/test id; retain intentional machine selector contracts |
| Banned strictness fix: .first() | It suppresses strictness rather than testing specified order | Narrow a semantic container and prove the promised outcome |
| Banned readiness substitute: networkidle | It substitutes connection silence for user readiness | Web-first state assertion or the required response plus UI outcome |
| Banned completion substitute: waitForTimeout | A sleep substitutes for completion | Event/barrier; controlled clock only for time logic |
| .catch(() => undefined) | It swallows a required action/assertion failure | Propagate the error; preserve intentional context-close cleanup separately |

The five confirmed patterns above are banned authoring fixes. Also inspect isVisible() branching, soft returns, copy/word-list assertions and retries used to mask failures. toHaveText on fixture data and accessible-name locator inputs remain valid.

Trace each required test command to an enabled CI job and uploaded full report. Flag missing-secret conditions that skip every test step, zero globally executed required tests, unexplained skips and disabled required lanes; an alternate real lane can satisfy coverage. Empty individual shards and declared optional lanes are not silent green.

For a measured flake hunt, use a named selection with --repeat-each and --shuffle <seed>, plus the failures console reporter. Keep full structured reports and original failure evidence. Report failures/executions and seed/worker count; never infer a cause from a single rerun.

Faith review pairs each title with its final assertion and independent expected value. Record title, assertion location, meaningful red control and the exact missing promise. Read-only audits report gaps; they never weaken assertions or auto-run the healer. A separately authorized scratch diagnosis may accept reviewed locator/wait fixes only, never assertion changes.
