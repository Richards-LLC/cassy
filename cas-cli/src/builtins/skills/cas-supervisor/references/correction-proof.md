# Proof after a correction

Use when a reviewed delivery changes before merge. Compare the old reviewed
tip with the new tip, including build scripts, dependencies, generated inputs,
catalogs and runners. Classify the actual delta, not the commit title or file
extension; a Markdown fixture or CLI help string can change the product.

## Choose the affected proof

Keep source review, executed runtime proof, implementer bundle validation,
independent QA verdict and CI as separate records. Use this table to scope
work, then apply the supported binding limits below.

| Delta | Repeat | Preserve when unaffected |
| --- | --- | --- |
| Product behavior, build/dependency or browser-test inputs | Review changed behavior; build a new candidate when its inputs changed; run affected checks and QA cells | Evidence for unrelated inputs; use the affected browser-test selection, not a new full matrix |
| CLI help text only | Inspect the exact rendered help from the revised candidate and any affected snapshots; build once if compiled help changed | Prior runtime/terminal cells outside help; a source comment that cannot affect output needs source review only |
| Tests only, such as missing request-field initializers | Compile affected test targets, run the named regression and require code CI on the new tip | Original runtime captures as proof of their executed revision when product/build inputs are unchanged; no automatic binary rebuild or unrelated matrix |
| Documentation/ledger only | Review the document and validate any supported bundle rebind | Original execution when the rebind validator accepts ancestry and unchanged protected inputs; a catalog or rendered fixture edit is not exempt |
| Pure rebase | Compare source patch-ids and inspect base changes for affected inputs/integration; require current CI | Source review for an identical patch; runtime evidence only if relevant base/build inputs also stayed unchanged |

## Apply binding limits

- **Source review:** follow [patch-id review](workflow.md#re-verify-a-rebased-lane-by-patch-id).
  A new diff needs review of that delta, not another review of unchanged code.
- **Implementer bundle:** `executed_head_sha` retains the actual execution
  commit; `head_sha` names the delivered tip. The existing `doc_only_rebind`
  validator must accept readable ancestor history, non-surface paths and
  unchanged protected product/browser/build inputs. A filename or an
  unchanged-runtime assertion cannot substitute for that validation.
- **Independent QA:** inspect `verification action=qa_status task_id=<task-id>`.
  Cassy can carry a verdict when the source patch matches (excluding its
  recognized generated build output); require the recorded carry at the new
  tip. Bundle rebind alone does not carry an independent verdict. Otherwise
  obtain the required current-tip independent round or an explicitly authorized,
  logged waiver. The reviewer uses this delta to scope checks and preserves
  original execution identities in the ledger.
- **Test-only limits:** there is no general test-only attestation that replaces
  bundle validation or current-tip QA. Retain unaffected runtime proof without
  relabelling it; if the installed gate rejects reuse, name that gate and its
  missing proof/owner. Use its supported correction path or track a separate
  capability fix; policy text does not grant a bypass.
- **CI:** require green validation for the new code/test tip before merge;
  pending/red remains a hold. For a documentation tail, consume only the
  code-check SHA explicitly accepted by the merge receipt. Neither patch-id
  equality nor an old candidate is a CI result. The merge queue owns final
  integration proof; do not add a duplicate full build or suite.

## Share one immutable candidate

Before requesting a build, inspect the delivery's existing candidate receipt.
Have one authorized build owner supply one immutable path to implementer and
reviewer with the executed commit/tree, build command and relevant input
comparison, binary path, version and SHA-256, environment, and existing proof
paths/owners. Keep these in the existing task note or ledger header; see
[binary evidence](../../cas-qa-craft/references/evidence-ledger.md).

Consumers verify identity and use that candidate rather than queue another
build of the same inputs. A new candidate gets a new path; never overwrite the
old one or relabel an old run as execution of a new tip. Missing or invalidated
proof gets a named owner; a gate that requires a new candidate still applies.
