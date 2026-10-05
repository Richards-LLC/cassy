# Commander delivery receipts

- **Audience and task:** The operator needs to distinguish a command still waiting, a send whose outcome is uncertain, and a reply retained by this browser.
- **Argument:** A device-storage receipt is earned by a durable application commit; it never substitutes for reading the reply.
- **Hero form:** Existing conversation bubbles with a short status caption; the message remains the leading content.
- **Distinctive move:** Use the existing Commander ruled conversation surface and muted receipt caption, with explicit device-local wording.
- **Omitted:** No account badges or cloud/read claims appear before those facts have an authenticated contract.

## States and safety

Waiting commands persist before leaving the composer, share atomic client-reference ownership across tabs, and keep the existing cancellation/expiry behavior. A write with an uncertain outcome remains unconfirmed across reload. Reply captions progress from Forwarded to Stored on this device only after strict IndexedDB commit. Storage failure leaves Forwarded and withholds the application ACK. Exact hub URL/device/session identities isolate this unenrolled journal; account-enrolled storage awaits its own feed contract.

Confirmed sends share a small, payload-free receipt across tabs. A committed receipt settles the same client reference to Delivered in every open thread and removes Retry. Explicit Retry retains the original client reference, so the daemon's bounded deduplication can return its first receipt. The journal refuses to re-claim a confirmed reference, even for a stale Retry callback or snapshot. A missing receipt remains uncertain; observing a socket write alone never claims delivery.

A peer's existing Not confirmed caption cannot overwrite a committed explicit Retry. Only dispatch advances a held send to a wire claim; held retries stay available to another tab if the writer closes before dispatch.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Existing purple conversation bubbles retain the Commander language; the device-local receipt is a short, explicit caption. |
| Fit to argument | 5 | Forwarded and Stored on this device name the two measured outcomes immediately below the reply. |
| Hierarchy | 5 | Message content leads; the smaller receipt caption supports it without introducing another attention surface. |
| Craft | 4 | Strict local DOM plus committed CSS QA passed eight stored/forwarded desktop/phone light/dark renders with zero findings; both receipt captions wrap within the bubble. |
| Accessibility | 4 | Strict contrast and overflow checks pass in both schemes, and real-build captures prove forced colors, reduced motion and increased contrast; no screen-reader or Android claim. |

Scored by daring-leopard-53 on 2026-10-05. Evidence is in the task's qa/ directory. Final commit-bound QA must refresh these receipts after source integration.
