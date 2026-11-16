# Caller-authority audit

The test-only scanner in `gate_audit_refusals.rs` extends `gate_audit.rs`.
It discovers refusals independently of gate declarations and bypass flags.
Policy roots cover factory, gates, hooks, task lifecycle, merge, verification
and release modules, plus existing registered policy source files and their
protective scripts.

Only a refusal whose controlling condition reads caller identity is a new
candidate. Sources include the caller's CAS environment role/ID, registered
caller resolvers, typed invocation principals and explicit caller-ID arguments.
Scope frames track values at each read position: mutable and immutable lets,
shadowing, tuple/struct/slice/reference patterns, if-let/match/loop bindings,
parameters, closure captures and assignments. Pattern components keep separate
provenance; leaving a scope restores shadowed names. Returned scalar
identity/predicate helpers carry provenance from their actual returned values. Generic role/owner fields do not. An unrelated helper that
merely logs the role does not. Ordinary I/O, parse and input-validation guards
are outside this discovery pass; the existing declaration/bypass audit remains.

`gate-audit-authority-baseline.json` records the remaining uncoded candidates.
Each key names the file, function, refusal API and normalized controlling
condition; its positive count is the existing allowance for that identity.
Line numbers and diagnostic wording are excluded. Role wire values and CAS
identity environment keys remain data. Duplicate guards consume additional
allowance, so a legacy entry cannot authorize unlimited copies.

Run the existing `gates::registry_tests` through the capped worker runner.
The same scan writes `target/gate-audit.md`, `target/gate-audit-authority.tsv`
and `target/gate-audit-authority-candidates.json`. The report distinguishes
registered codes, baseline sites and new uncoded sites. The registry total is
unchanged by this baseline; candidates are not new gate declarations.

A new authority refusal must emit a registered code. Do not add it to the
baseline. When legacy sites are removed or coded, their baseline entries can
be reduced or removed using the generated candidate inventory. In a complete
Git checkout, the test checks the working allowance against the landed target's
first-parent baseline versions. A target without baseline history permits the
first seed; intermediate commits on an unmerged delivery do not constrain it.
A landed reduction cannot be undone. A landed deletion is an empty allowance,
so restoring an older file cannot regain entries. The scan reads the existing
CI `BASE_SHA`; locally it uses `git merge-base HEAD origin/main`. Git fixtures
pass an explicit target to `scan_against`; parser fixtures need no Git history.
The same test exercises first landing, growth, shrinkage, deletion/restore,
duplicate/new guards, metadata exclusion and wording-only edits.
