# Proof after a correction

Use when a reviewed delivery changes before merge. Compare the old reviewed
tip with the new one, including build scripts, dependencies, generated inputs,
catalogs and runners, and classify the actual delta, not the commit title or
file extension: a Markdown fixture or CLI help string can change the product.

## Choose the affected proof

Keep source review, runtime proof, implementer bundle validation, independent
QA and CI as separate records. Scope the rerun with this table, then apply the
binding limits below.

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
- **Implementer bundle:** `executed_head_sha` keeps the execution commit and
  `head_sha` names the delivered tip. Reuse needs the `doc_only_rebind`
  validator to accept it; a filename or an unchanged-runtime assertion is no
  substitute.
- **Independent QA:** inspect `verification action=qa_status task_id=<task-id>`.
  Cassy carries a verdict when the source patch matches; require that recorded
  carry at the new tip. A bundle rebind alone carries no verdict. Otherwise get
  a current-tip round or a logged, authorized waiver; the reviewer scopes checks
  to this delta and keeps the original execution identities in the ledger.
- **Test-only limits:** no test-only attestation replaces bundle validation or
  current-tip QA. Keep unaffected runtime proof without relabelling it; if a
  gate rejects reuse, name the gate and its missing proof and owner, and use its
  correction path. Policy text grants no bypass.
- **CI:** the new code or test tip needs its own CI before merge; red holds.
  For a documentation tail, use only the code-check SHA the merge receipt
  accepts. Neither patch-id equality nor an old candidate is a CI result.
  <!-- cas-src:begin -->
  The merge queue owns final integration proof; add no duplicate build or suite.
  <!-- cas-src:else -->
  The final integration gate (CI on the landing SHA) owns that proof; add no
  duplicate build or suite.
  <!-- cas-src:end -->

## Share one immutable candidate

Inspect the delivery's existing candidate receipt before requesting a build.
One build owner supplies one immutable path to implementer and reviewer, with
the commit, build command, binary path, version, SHA-256 and environment
recorded in the task note or ledger header
([binary evidence](../../cas-qa-craft/references/evidence-ledger.md)).

Consumers verify identity and use that candidate instead of rebuilding the
same inputs. A new candidate gets a new path; never overwrite the old one or
relabel an old run as execution of a new tip. Missing or invalidated proof gets
a named owner.
