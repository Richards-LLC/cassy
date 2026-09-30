# Coding standards for review

Use this document in the Standards review context. Workers receive their task
and lifecycle guidance separately. Spec review owns acceptance-criterion coverage
and scope creep; keep its verdict separate from the Standards verdict.

These are judgment calls. Cite the changed hunk, the relevant criterion and the
consequence for a caller. Report a possible smell rather than treating its name
as a defect. A documented repository decision overrides the baseline. Skip rules
already enforced by the compiler, lint, contract registry or gate; a mechanical
violation belongs in a check chore, not another prose rule here.

## Tests that justify confidence

- Prefer a caller's observable result through a public interface. A test of an
  internal call or storage side channel can pass while the promised operation
  fails. Judge whether the selected seam actually exercises that operation.
- Expected results need an independent source: a specified example, external
  contract or observed output checked independently of the implementation.
  Repeating the production algorithm in the assertion can repeat its defect.
  A reviewed snapshot is useful when its output and contract matter; copying
  the implementation's formatting into an expected string is weak evidence.
- Ask what plausible behavioral defect would make the test fail. A constant
  asserted against itself, its restated literal, or a fixture with no meaningful
  assertion supplies little confidence about its consumers. A deliberate change
  detector can still protect a documented compatibility contract.
- Source-as-text, source-order and prose assertions need an actual textual
  consumer. Registration inventories and parser/security tokens can warrant a
  structural check; textual presence alone does not establish runtime wiring,
  dispatch order or successful execution. Prefer a real handler/transport run
  for those claims. Keep necessary textual contracts centralized with a reason.
- Group cases by one observable capability so a failure identifies the broken
  promise. Judge whether setup and private choreography make a harmless internal
  refactor break the test; avoid prescribing a universal number of assertions.
- Choose boundary evidence that resembles the caller's input. For wire-facing
  paths, captured payloads reveal aliases, missing fields and shape differences
  that constructing the destination Rust struct bypasses. Look at downstream
  values and effects as well as successful deserialization.

## Dependencies and module shape

Use real in-process implementations and practical database fixtures for owned
code. Substitute external services, clocks, randomness or unsuitable filesystem
and transport boundaries where controlling the result helps isolate behavior.
Framework provider overrides are suitable when the real consuming module runs
and the replaced dependency is separately owned. An internal mock used solely
to assert that another internal object was called couples the test to design.

Prefer typed per-operation ports at external seams over a generic fake that
switches on URL/method and returns unrelated shapes. Keep transport details in
the adapter. A seam earns its abstraction by hiding real complexity or making
a required behavior observable; an extra forwarding layer alone adds little.
Refactoring belongs in review after the implementation's green behavioral proof;
preserve that proof while improving the design.

## Smell baseline

Adapted from the Fowler baseline in Matt Pocock's `code-review` skill. Each row
is a possible symptom to inspect, not a required rewrite. Generated catalogs,
framework vocabulary and public compatibility can justify an apparent smell.

| Possible smell | Evidence to inspect | Candidate improvement |
| --- | --- | --- |
| Mysterious Name | A name hides what an operation does or a value holds. | Name the domain action or concept; reconsider the seam if no honest name fits. |
| Duplicated Code | The same logic must change together in several places. | Share that logic when the cases truly have the same contract. |
| Feature Envy | An operation mainly manipulates another module's data. | Move behavior toward its owner. |
| Data Clumps | The same related fields repeatedly travel together. | Give the group a meaningful type. |
| Primitive Obsession | Strings or numbers carry domain invariants invisibly. | Introduce a small domain type where it clarifies a real invariant. |
| Repeated Switches | Several sites classify the same variants independently. | Centralize the classification or share the dispatch. |
| Shotgun Surgery | One behavior change requires scattered coordinated edits. | Gather code that changes for the same reason. |
| Divergent Change | One module changes for several unrelated reasons. | Split along the reasons for change. |
| Speculative Generality | Parameters or extension hooks support no required case. | Remove the unused variation. |
| Message Chains | A caller navigates through several private representations. | Expose the needed operation at the owning boundary. |
| Middle Man | A layer forwards without hiding complexity or enforcing a contract. | Call the owner directly when compatibility permits. |
| Refused Bequest | An implementation discards most of its inherited contract. | Prefer composition or a smaller interface. |

## Provenance

Test and mock criteria adapt `cas-tdd` and its worked examples, MIT © 2026 Matt
Pocock; the smell baseline adapts his `code-review` skill under the same
[MIT notice](cas-cli/src/builtins/skills/cas-tdd/LICENSE). The Cassy test audit
adds source-text, source-order and prose-contract distinctions.
Promoted rule-172's wire-shape incidents inform the boundary-evidence criterion.
Promoted rule-026 (environment guard) and rule-175 (release completeness) route
to mechanical enforcement and retain their evidence in the rule store; they
add no judgment rule to this document.
