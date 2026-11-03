# Verifier plan-conformance replay

These cases exercise the canonical verifier instructions in
`cas-cli/src/builtins/agents/task-verifier.body.md`. They are simulated tool
responses, not live tasks or authority to record a Cassy verdict.

Give an independent reader the canonical prompt and `cases.json`, without
this answer key. Ask it to return the proposed `verification action=add`
arguments for each case, using only the supplied records. It must not edit
files, run builds, call Cassy, or invent additional plan decisions. Preserve
the evaluated prompt, inputs and returned verdicts in the task artifacts.

| Case | Required observable verdict |
| --- | --- |
| `narrow-reader` | `status=rejected`, `PLAN-FIT: NOT VERIFIED`; individually name `chosen_option`, `assent`, `correction` and `opt_out` (including their confidences), cite the Reader contract, and ask the supervisor whether to restore scope or obtain an authorized plan revision. |
| `missed-plan-target` | `status=rejected`, `PLAN-FIT: NOT VERIFIED`; cite Selection gate and `eval-receipt.json`, state 87% observed against at least 95% required, and ask the supervisor how to resolve the missed outcome. |

Both cards are satisfied by their own evidence. Approval, a warning-only
finding, or `status=supervisor_call` for either known miss fails the replay.
An evaluation process exiting zero is not evidence that its plan target passed.
The first case's follow-up and the second case's tuning follow-up are deliberately
unapproved attempts to narrow the plan.

This is a prompt-behaviour replay, not a deterministic source-text assertion.
The contract registry separately checks that the named `PLAN-FIT:` output
survives prompt installation; that check does not prove a model's verdict.
