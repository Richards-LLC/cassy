## Core Concepts

### Failing tests from verifier findings

A blocking rejected finding can carry `triage.failing_tests`, a list of concrete test identifiers. Recording the verdict also merges these names with failures already recorded for the task. A later repair request needs a covering `class_audit`; a failed verdict transaction leaves neither a verdict nor new failure facts behind.

Findings without test names and older verdicts keep their existing behavior. Invalid identifiers are dropped and logged, and warnings do not create a repair obligation. The verifier prompt asks for known tests and forbids invented names.
