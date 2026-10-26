# Rounds and feedback

A **round** is one bundle of client feedback, sent after one delivery and
answered by the next. Asking the client to bundle feedback into rounds keeps
the team from switching context on every message. Round 1 is the first
delivery. Client answers to gap questions (`Partial` rows) are not rounds:
they close our open questions and do not add work.

## `docs/sow/<sow-slug>/ROUNDS.md`

```markdown
# Rounds: <SOW title>

size: medium_large        # or small (from the SOW front matter)
rounds_included: 3        # from DELIVERY.md for this size
review_cycle_days: 14

| Round | Delivered | Review due | Environment | Report | Client feedback received |
| --- | --- | --- | --- | --- | --- |
| 1 | 2026-10-20 | 2026-11-03 | staging | reports/2026-10-20-<slug>-round-1.pdf | 2026-10-24 |

## Round 2 items

| Item | Feature | Requested (client words, quoted) | Estimate h (incl. QA) | In SOW scope | Approval | Rounds on this feature | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| C2.1 | <feature> | "<quote>" | 1.5 | yes | not needed (≤ threshold) | 1 | done |
| C2.2 | <feature> | "<quote>" | 4 | yes | PM approved 2026-10-25 | 2 | done |
```

`Review due` is `Delivered + review_cycle_days`. `Rounds on this feature`
counts the rounds that changed the same feature, including this one.

## Rules

Apply each rule when you add an item. The PM owns scope and billing calls,
so the developer records the decision and does not make it.

| Condition | Action before any work on the item |
| --- | --- |
| Estimate > `approval_hours` | Draft **Approval request** below and record the PM's decision in `Approval` |
| SOW `size: small` (or `rounds_included = 0`) | Every new request is out of the included scope. Draft **Billing surface** and do no work until the PM answers |
| `Rounds on this feature` > `pm_ping_after_round` | Draft **Next-round ping**, so the PM can tell the client this feature is entering another round |
| Round number > `rounds_included` | Draft **Next-round ping** for the whole SOW |
| The request contradicts or exceeds the SOW text | Treat it as out of scope; same as the small-SOW row |

## PM message templates

Post these in `internal_channel` and tag `pm_mention`. Fill every `{slot}`.

### Approval request

> {pm_mention} Change request on {sow_title}, round {round}: "{request_quote}".
> Estimate {hours} h including QA, above the {approval_hours} h threshold.
> In SOW scope: {in_scope}. OK to proceed?

### Billing surface

> {pm_mention} {client_name} asked for "{request_quote}" on {sow_title}. This SOW
> is {size} with no included tweak rounds, so I have not started it. Estimate
> {hours} h including QA. Flagging for billing.

### Next-round ping

> {pm_mention} {feature} on {sow_title} has had {rounds_on_feature} rounds of
> feedback. The client's latest bundle would start round {next_round}. Do you
> want to present that to {client_name} before we start?
