---
metadata:
  managed_by: cas
---

# Design it twice

Adapted from mattpocock/skills `codebase-design/DESIGN-IT-TWICE.md`,
MIT © 2026 Matt Pocock; the technique follows John Ousterhout.

Use when a chosen module needs materially different interface designs before
committing to a consequential seam. This is interface exploration, not parallel
implementation or multiple owners of the assigned task.

## Frame the shared problem

Write the problem space for the invoking user or supervisor: the capability,
constraints every proposal must satisfy, dependency categories, and an
illustrative code sketch that grounds the constraints without proposing an
answer. Use the vocabulary in [SKILL.md](../SKILL.md) and settled project terms
from Cassy memory/spec records; framework vocabulary takes precedence.

Give each agent a self-contained technical brief: relevant file paths, actual
coupling, the proposed module's responsibilities, what must stay behind the
seam, compatibility constraints, and these dependency categories:
in-process, local-substitutable, owned remote, or true external. Gather evidence
for those facts before treating them as constraints.

## Independent parallel proposals

Spawn at least three sub-agents in parallel, with distinct briefs and no edits
or task lifecycle ownership:

1. Minimum interface: aim for one to three entry points and high caller leverage.
2. Maximum flexibility: support justified extensions and multiple use cases.
3. Common caller: make the ordinary case trivial and name exceptional cases.
4. Ports and adapters, when cross-seam dependencies apply: separate the domain
   port from concrete local/remote adapters and their error/retry contracts.

Ask agents to challenge the illustrative sketch rather than converge on it.
If the runtime lacks parallel agent capacity, name that limitation in the
completion note; sequential alternatives are deferred parallel proof, not a
claim that the independent exploration ran.

## Output specification for every agent

Require these five parts, using the same problem and domain vocabulary:

1. Interface: types, entry points, parameters, invariants, ordering, errors,
   configuration and relevant performance facts a caller must know.
2. Usage example showing a real caller crossing the interface.
3. Hidden implementation complexity and the deletion-test result.
4. Dependency strategy: ports, adapters, ownership and how each category is
   exercised through the interface.
5. Trade-offs: where leverage is strong/thin, locality, compatibility and the
   cost of adding or changing a caller.

## Compare and recommend

Present proposals sequentially so each is legible, then compare depth, locality
and seam placement under the original constraints. Reject designs that merely
rename the same interface; expose facts moved onto callers and speculative
extension costs. Give one final recommendation with reasons. Propose a hybrid
only when its chosen parts form a coherent smaller interface, and state what
was rejected.

Record the chosen seam, interface facts, hidden complexity, deletion test and
two rejected alternatives in the main skill's completion note. Use a Cassy
spec for a hard-to-reverse decision; do not open parallel glossary/ADR files.
