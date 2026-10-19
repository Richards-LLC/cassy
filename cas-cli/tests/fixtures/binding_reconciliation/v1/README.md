# Binding reconciliation v1 fixtures

Synthetic, isolated examples of approved spec-b489 / art-d358cc91, authored for
Cassy task cas-9021db58f400ac61. These are not server captures or deployment proof.
They belong to `cassy.binding-reconciliation/v1`, independently of the immutable
`coordination_contract/v1` fixture family.

`completed.json` describes an ordinary activation with its original activated
fence receipt. `legacy-intake-active.json` describes an attempted fence refused
by active legacy readers and leases. The parser tests also derive a repeat
attempt, an unknown mutation outcome and a pre-authorization refusal through the
same public wire parser.

Cloud9864/spec-b489 clarification (supervisor receipt4197864): nonnull
activation_id pins latest_attempt to this invocation, even under concurrent
requests. Its settlement and completed reason follow that immutable receipt.
An already-managed retry preserves its original fence receipt, including any
historical ADMIN attestations; only the new ordinary invocation must attest no
consumers. `cloud-receipt-identity-r1.json` is the unchanged JSON block from
[Cloud's approved addendum](https://github.com/Richards-LLC/petra-stella-cloud/issues/178#issuecomment-6239734244),
canonical artifact art-2ec9e7fb. The existing completion test consumes all seven
complete examples (three accepts, four single-invariant rejects). This proves
parser compatibility with the published design examples, not deployed behavior.

The request target must already contain a Cloud-resolved canonical project.
The parser preserves exact identities, takes enrolled workspace only from the
Cloud response, and requires it to agree with the binding. Even an evidenced
response supplies configuration evidence, not work authority. Transport or parse
failure cannot prove that a submitted mutation had no effect; an unknown attempt
must retain its conservative pause until authoritative reconciliation. A negative
authority lookup alone does not settle an earlier timed-out POST.
