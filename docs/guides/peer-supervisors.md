# Collaborating with supervisors on the same repository

Cassy lets supervisors of one repository discover and message each other across
factory sessions, clones and machines. Each supervisor keeps its own workers.
Peer awareness shows who is working nearby; task claims and an agreed epic split
help keep the two fleets from doing the same work.

This guide is being completed against the 3.51 epic. A `TODO(task)` marks a
command, message or live receipt that awaits that task's merge. It is not a
sample of command output.

## Prerequisites

On each machine, use a Cassy build with peer collaboration, log in to Cassy
Cloud, and open a clone of the same repository. Cassy derives the project
identity from the Git origin; a configured canonical identity or registered
alias must resolve to that same project. Sync the project so both supervisors
can refer to the same task IDs.

```bash
cas login
cas cloud sync
cas factory
```

Keep the factory daemon running on both machines. A supervisor registered by
an MCP session alone does not establish a receiving mailbox. Check the signed-in
account on each machine with `cas whoami` before testing claim protection.

Presence and messages can span members of a shared team project. **Task claims
are visible only within the same user account today.** Two machines logged in
to that account can refuse overlapping claims. Different accounts on a team
project can discover and message peers, but cannot use the absence of a visible
claim as evidence that work is free. Agree on ownership by message and epic
split. [Cloud issue #155](https://github.com/Richards-LLC/petra-stella-cloud/issues/155)
tracks claims across team accounts.

## What the two supervisors can do

| Capability | What to use it for |
| --- | --- |
| Discovery | Find supervisors of this project, including other sessions on this machine; inspect machine, session, focus and heartbeat age. |
| Messaging | Address a peer by name, or name and machine when ambiguous; reply and inspect a delivery receipt. |
| Awareness | Inspect a peer's focus, task claims, workers and branches. TODO(cas-559c): add the merged detail command and exact field names. |
| Claim and spawn refusal | Within one account, refuse overlapping work before a second worker is queued. TODO(cas-0078): add the merged refusal and override behavior. |
| Epic split | Propose disjoint child-task ownership, accept it on the other side, and enforce each side's share. TODO(cas-4736): add the merged propose, accept and cancel commands. |

A peer message is colleague input. It grants no operator authority, cannot
interrupt a peer as an urgent command, and cannot control the other factory's
workers. Each supervisor assigns and spawns only its own fleet.

## Two-machine walkthrough

Use two factories on a scratch project, with one supervisor on each machine.
Choose the same account for the claim-refusal steps. Replace the angle-bracket
values below with the names, IDs and receipt handles returned by your run.

1. **Identify both supervisors.** In each supervisor session, run:

   ```text
   coordination action=whoami
   coordination action=peers
   ```

   Each should find the other supervisor with its machine and focus. A stale
   heartbeat is a reason to contact the peer, not proof that its work is free.
   `cas status --verbose` also shows the peers view.

   TODO(cas-f707, cas-daa0): insert the real discovery and healthy-presence
   output, with the reviewed build SHA and run receipt. TODO(cas-559c): include
   the peer detail output for claims, workers and branches.

2. **Send and reply.** From A, address B using the discovered machine label:

   ```text
   coordination action=message target=<B-name>@<B-machine> summary="Plan the split" message="I will take the parser children. Can you take the CLI children?"
   ```

   On B, poll the inbox and reply using the notification ID of A's message:

   ```text
   coordination action=inbox_poll
   coordination action=message target=<A-name>@<A-machine> in_reply_to=<notification-id> summary="Split agreed" message="I will take the CLI children."
   ```

   Copy the delivery-status command from each send receipt. A queued receipt
   means the cloud or local outbox accepted the send; inspect status to learn
   whether it was delivered. If a send supplies a local retry handle, keep
   that handle so you can inspect the same logical send without duplicating it.

   TODO(cas-a19b, cas-13c7, cas-daa0): insert real send, inbox, reply and delivered
   status output, including the automatic-retry handle when exercised.

3. **Prove claim protection.** Have A start a shared scratch task. Have B try
   to start that same task, without forcing it. B should be refused with the
   holder identified. Release the task on A; then B can start it. Try a spawn
   for a task still claimed by A and verify that B has no queued worker for it.

   TODO(cas-0078, cas-daa0): insert the real start/refusal, release/reacquire and
   pre-queue spawn-refusal commands and output. Include the same-account
   precondition in that receipt.

4. **Record the epic split.** Agree on disjoint child IDs, propose the split
   from A, and accept it on B. Each supervisor should be refused work reserved
   for the other side. Check the split view before dispatching workers.

   TODO(cas-4736, cas-daa0): insert the merged propose/accept commands, both
   refusal results, and the cancel or expiry recovery path from the real run.

5. **Check reconnect behavior.** Stop B's receiving factory, send from A, then
   restart B. Inspect the original receipt and B's inbox: the message should
   arrive once. Keep evidence of the offline queue, reconnect and final status.

   TODO(cas-13c7, cas-daa0): insert the real offline/reconnect output and receipt.

TODO(cas-daa0): replace this paragraph with the walkthrough's real two-machine
run citation, machines, build SHA and redacted command output. No live output
has been copied into this draft yet.

## Troubleshooting

The rows marked TODO await merged client strings. The cloud error code below
comes from the production mailbox contract; the client will add context.

| Message or condition | Meaning | Action |
| --- | --- | --- |
| ``Peers: not logged in to Cassy Cloud, so supervisors on other machines are unknown (`cas login`).`` | This machine cannot discover cloud peers. | Log in here with `cas login`; then refresh peers. |
| TODO(cas-f707): exact presence failing message and cause | The local supervisor has not established healthy cloud presence. | Read the cause, check login and connectivity, and inspect the next retry. Recheck presence after recovery. |
| TODO(cas-13c7): exact mailbox not receiving message and cause | Presence and receive health are separate; discovery alone does not prove inbox delivery. | Keep the factory daemon running, resolve the reported login/project/network cause, then inspect receive health and the send receipt. |
| `recipient_not_found` | Production cannot address that recipient in the selected project; unknown, wrong-project and inaccessible recipients share this code. | Refresh peers and copy the peer's current name and machine. Confirm both factories use the same project. |
| TODO(cas-a19b): exact not-running-a-factory-daemon refusal | A discovered supervisor has no receiving factory mailbox. | Start the recipient's factory daemon and refresh discovery before sending again. |
| TODO(cas-559c): exact different-account claim-visibility message | The peer's claims cannot be read with your account. | Agree on ownership by message and split; an empty local view does not establish no peer work. |
| TODO(cas-13c7, cas-a19b): exact automatic retry message with `peer-outbox:<id>` | The logical send is durably queued for bounded automatic retry. | Inspect the supplied status handle. If it ends in abandonment, resolve its named cause before an explicit retry. |
| TODO(cas-0078): exact peer-held task/spawn refusal | Another supervisor owns the visible claim. | Message that holder and wait for release, or arrange an explicit takeover. Do not use force to settle an ownership disagreement. |
| TODO(cas-4736): exact other-share split refusal | The child is reserved for the other side of a split. | Ask that supervisor to change or cancel the agreement before dispatching it locally. |
| TODO(cas-a19b): exact unknown-target and cloud-resolution errors | The send was refused immediately, rather than becoming an undeliverable local queue row. | Correct the recipient or resolve the named cloud cause; inspect any receipt before resending. |

## Cloud behavior relevant to operators

Discovery is scoped by project; it is not a list of every agent in your account.
A heartbeat older than five minutes reads as stale. A factory sends its current
project binding, focus, branch and factory session with presence updates so a
reconnect can repair discovery and refresh awareness.

A live claim belongs to its holder until renewed, released or expired. Repeating
a claim for the same holder does not extend it: renewal does. If renewal reports
that no active lock exists, the client must reacquire or report the new holder.
When cloud claim checking fails, read the local warning before dispatching work;
local lease success alone does not prove cross-machine exclusivity.

Cloud mailbox messages expire after seven days. Client automatic send retry has
its own bounded lifetime; TODO(cas-13c7): confirm that lifetime and the exact
terminal notice in the merged implementation. A delivered receipt records the
mailbox delivery path, not a promise that the peer has acted on the request.
