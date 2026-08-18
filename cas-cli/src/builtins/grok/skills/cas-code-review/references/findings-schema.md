# Findings Schema — cas-code-review

All eight reviewer personas (correctness, testing, maintainability,
project-standards, fallow, security, performance, adversarial) **must** emit a
single JSON object matching this schema. The orchestrator parses, validates,
merges, and routes these outputs. Any persona output that fails validation is
treated as a reviewer error, not a finding.

The canonical Rust types live in
[`crates/cas-types/src/code_review.rs`](../../../../../../crates/cas-types/src/code_review.rs)
(`Finding`, `ReviewerOutput`, `Severity`, `AutofixClass`, `Owner`). Keep this
doc and that file in sync — the Rust types are authoritative at runtime.

## Envelope — `ReviewerOutput`

Every persona returns exactly one object of this shape:

```json
{
  "reviewer": "correctness",
  "findings": [ /* zero or more Finding objects, see below */ ],
  "residual_risks": [
    "Unverified: retry loop may livelock on transient network errors"
  ],
  "testing_gaps": [
    "No test covers the case where the lock file is stale"
  ]
}
```

**Field guidance:**

| Field            | Type              | Required | Notes                                                                                         |
| ---------------- | ----------------- | -------- | --------------------------------------------------------------------------------------------- |
| `reviewer`       | string            | yes      | Persona name, lowercase. Must match the seven canonical names.                                |
| `findings`       | array of Finding  | yes      | May be empty. An empty array is a valid clean pass.                                           |
| `residual_risks` | array of string   | no       | Suspicions the persona could not confirm. Surfaced but **never** auto-routed to tasks.        |
| `testing_gaps`   | array of string   | no       | Coverage holes the persona noticed. Informational; review-to-task may create tasks from these. |

Unknown top-level fields are rejected.

## Finding

```json
{
  "title": "Unwrap on parsed int can panic on user input",
  "severity": "P1",
  "file": "src/foo.rs",
  "line": 42,
  "why_it_matters": "Panics crash the worker before task.close fires, leaking the lease.",
  "autofix_class": "safe_auto",
  "owner": "review-fixer",
  "confidence": 0.85,
  "evidence": [
    "let n: u32 = s.parse().unwrap();  // src/foo.rs:42"
  ],
  "pre_existing": false,
  "suggested_fix": "Use `?` or `.map_err(|e| Error::Parse(e))`",
  "requires_verification": false
}
```

### Field reference

| Field                   | Type           | Required | Rules                                                                                                              |
| ----------------------- | -------------- | -------- | ------------------------------------------------------------------------------------------------------------------ |
| `title`                 | string         | yes      | ≤ 100 characters. Non-empty after trim. One-line human label.                                                      |
| `severity`              | enum           | yes      | `"P0"` \| `"P1"` \| `"P2"` \| `"P3"`. Maps to Cassy task priorities 0–3.                                             |
| `file`                  | string         | yes      | **Relative** path from repo root. Absolute (`/…`, `C:\…`) paths are rejected.                                      |
| `line`                  | u32            | yes      | 1-based. Use the most relevant single line even for multi-line issues.                                             |
| `why_it_matters`        | string         | yes      | Consequence if unaddressed. Must be concrete — no "could be bad".                                                  |
| `autofix_class`         | enum           | yes      | `"safe_auto"` \| `"gated_auto"` \| `"manual"` \| `"advisory"`.                                                     |
| `owner`                 | enum           | yes      | `"review-fixer"` \| `"downstream-resolver"` \| `"human"`.                                                          |
| `confidence`            | float          | yes      | Inclusive range `0.0..=1.0`. Orchestrator suppresses `< 0.60` except `P0 ≥ 0.50`.                                  |
| `evidence`              | array of string| yes      | **At least one** entry. Each must be non-empty. Quote code, file:line refs, or command output.                     |
| `pre_existing`          | bool           | yes      | `true` if the finding exists on the base ref; `false` if introduced by the diff.                                   |
| `suggested_fix`         | string         | no       | Optional concrete fix — diff, patch hint, or prose.                                                                |
| `requires_verification` | bool           | no       | Defaults to `false`. Set `true` when the fix needs a follow-up test or smoke run.                                  |

Unknown fields are rejected at parse time.

### Severity guidance

| Severity | Meaning                                                                             | Autofix effect                                              |
| -------- | ----------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| `P0`     | Blocker. Data loss, panic on common path, auth bypass, corruption, crash loop.      | Hard-blocks `task.close`. Supervisor override required.     |
| `P1`     | High. Likely bug, broken invariant, meaningful test gap on a shipped path.          | Becomes a high-priority downstream task if not auto-fixed.  |
| `P2`     | Medium. Maintainability, smell, marginal correctness risk.                          | Medium-priority task.                                       |
| `P3`     | Low. Nit, style drift outside `project-standards`, opinion.                         | Low-priority task or advisory-only.                         |

### `autofix_class` guidance

- `safe_auto` — Deterministic, local, reversible. The fixer sub-agent can apply
  without human review (e.g., `unwrap` → `?`, rename an unused var).
- `gated_auto` — Mechanically applicable but needs an ack (e.g., changing a
  public signature, touching a migration).
- `manual` — Requires human or downstream-agent judgment.
- `advisory` — Informational. **Never** becomes a task. Appears in orchestrator
  output only.

### `owner` guidance

- `review-fixer` — Only valid with `safe_auto` or `gated_auto`.
- `downstream-resolver` — Becomes a Cassy task via the review-to-task flow.
- `human` — Surfaces to the supervisor / developer. No automation routes here.

On disagreement between personas about owner, the orchestrator keeps the
**more restrictive** owner (`human` > `downstream-resolver` > `review-fixer`).

### `confidence` guidance

- `0.90+` — Grounded in a direct quote from the diff plus an obvious invariant.
- `0.70–0.89` — High confidence, some inference.
- `0.60–0.69` — Worth surfacing but borderline; suppressed unless P0.
- `< 0.60` — Put it in `residual_risks` instead.

### `evidence` guidance

Every finding **must** cite at least one concrete piece of evidence. Acceptable:

- Quoted code with `file:line` anchor
- Quoted error / test output
- Quoted rule or doc reference
- A specific prior incident reference

Unacceptable (will be suppressed as low-quality):

- "The code looks risky"
- "This pattern is generally discouraged"
- Unquoted paraphrase of the diff

## Minimal valid examples

**Clean pass — no findings:**

```json
{
  "reviewer": "security",
  "findings": [],
  "residual_risks": [],
  "testing_gaps": []
}
```

**Single P0:**

```json
{
  "reviewer": "correctness",
  "findings": [
    {
      "title": "Missing null check before dereferencing session pointer",
      "severity": "P0",
      "file": "cas-cli/src/stores/tasks/close_ops.rs",
      "line": 118,
      "why_it_matters": "Segfaults the worker on concurrent close; tasks get stuck in pending_verification.",
      "autofix_class": "manual",
      "owner": "downstream-resolver",
      "confidence": 0.92,
      "evidence": [
        "let s = session.as_ref().unwrap();  // close_ops.rs:118 — session can be None after release_lease"
      ],
      "pre_existing": false,
      "requires_verification": true
    }
  ],
  "residual_risks": [],
  "testing_gaps": [
    "No test exercises concurrent close on the same task"
  ]
}
```

## Validation summary

A persona output is valid iff:

1. It parses as the envelope shape above, with no unknown top-level fields.
2. `reviewer` is non-empty after trim.
3. Every `Finding` inside:
   - has a non-empty `title` of ≤ 100 characters,
   - has `confidence` in the inclusive range `0.0..=1.0` (not NaN),
   - has a non-empty `evidence` array with no empty entries,
   - has a non-empty **relative** `file` path,
   - has a non-empty `why_it_matters`,
   - uses only the declared enum variants for `severity`, `autofix_class`, `owner`.

Any other shape is a reviewer error and the orchestrator logs it and moves on.
