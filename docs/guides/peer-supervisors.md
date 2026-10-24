# Collaborating with supervisors on the same repository

Supervisors of one GitHub repository can find each other, message each other
and stay off each other's work, whether they run on one machine or several.
Each supervisor keeps its own workers. A peer is a colleague: you can see it,
message it and agree on who does what, but you cannot spawn, assign or direct
its workers, and it cannot direct yours.

## What you get

| Capability | How |
| --- | --- |
| Discovery | `coordination action=peers` lists the other supervisors of this repository with machine, session, epic focus and heartbeat age. The same section appears in `factory action=worker_status` and `cas status --verbose`. |
| Presence | The first line of that section is your own cloud registration: `Cloud presence: registered …` or `Cloud presence: failing: <cause>`. |
| Messaging | `coordination action=message target=<name>` (or `<name>@<machine>`) reaches a peer's inbox through the Cassy Cloud mailbox. Replies use `in_reply_to`; `coordination action=message_status id=<peer message id>` reads delivery. |
| Worker peer lane | A worker may message another registered worker in the same factory session with `coordination action=message target=<worker-name>`. Each peer message is copied to the session supervisor. This is a narrow local lane, not cross-session or cross-machine worker chat. |
| Awareness | `cas peers --detail` (or `--peer <name>`) shows a peer's focus, task claims, live workers and branches. |
| Claim and spawn refusal | `task start`, `task action=claim` and `spawn_workers task_id=` refuse a task another session of this repository holds, naming the holder and when its claim runs out. Nothing is queued. |
| Epic split | `factory action=epic_split` records which children of an epic each side owns. Each side's start and spawn on the other's children is refused. |

## Prerequisites

On every machine:

1. A Cassy build with peer collaboration (3.51).
2. `cas login` to Cassy Cloud. Without it, peers reads
   ``Peers: not logged in to Cassy Cloud, so supervisors on other machines are unknown (`cas login`).``
3. A clone of the same repository. Peers are matched by the repository's
   canonical id, taken from `[project] canonical_id` in `.cas/config.toml` or
   the git origin; `git@github.com:o/r.git` and `github.com/o/r` match.
4. A running factory (`cas factory`). The factory daemon is what receives peer
   messages. A supervisor without one is listed, but a send to it fails with
   `<name>@<machine> is not running a factory daemon, so it cannot receive peer messages`.

**Accounts.** Discovery, messaging and task claims span members of the same
team project. Another account's held task appears in peer details with its
owner and machine, and refuses start, claim and spawn. JSON marks these claims
`team_project`; a failed read remains unknown. Personal projects keep per-account
isolation. Renew and release remain owner-only, so a teammate's claim must be
released by its owner or expire before another account can claim it.

**Whose supervisors you see.** Every peer names its owning account: `owner
you` for your own supervisors, `owner teammate <email>` for another member's
(`cas peers` shows the same as `Account …`). A shared team project lists
teammates' supervisors by default. To keep this project to your own account,
the operator runs:

```bash
cas config set peers.same_repo_scope own
```

With `own`, the peer list, `cas peers` and plain-name messages reach only your
own supervisors. Other accounts' supervisors are counted (`1 supervisor of
other accounts hidden: peers.same_repo_scope is own`), and a message or reply
to one is refused by name with `Nothing was sent.` The default is `team`.
Changing the key is operator-only. Messages to supervisors of your other
projects are always limited to your own account, whatever this key says.
Cassy enforces `own` on this client; the cloud still lets a teammate on the
team project message you
([petra-stella-cloud#168](https://github.com/Richards-LLC/petra-stella-cloud/issues/168)).

**Opting out.** A project that sets `cloud.auto_sync = false` does not register
its agents, so their names, hostnames and branches stay on the machine. Its
peers section then opens with
`Cloud presence: disabled (cloud.auto_sync=false): this project's agents are not registered with Cassy Cloud.`
and `Peers: discovery is one-way: this project still lists peers that registered, but they cannot see its supervisors.`

## Walkthrough

The output below is copied from real runs, not written by hand:

- Steps 1, 3 and 4: run `29dfcb` (QA task cas-91b89), build `8ada608`, two
  factories with separate databases on one machine (`soundwave`), against the
  production Cassy Cloud. Peer details in step 1 come from run `b251eb` (QA task
  cas-71d4) in the same setup. Claim refusal in step 3 comes from the cas-0078
  production run.
- Step 2: the two-machine live-check runner (cas-daa0), run `878c15`, between
  `soundwave` (Linux) and `prowl` (macOS) against a stand-in cloud that serves
  the production routes.

### 1. See each other

On each supervisor, run `coordination action=peers`. A's view of B:

```text
Cloud presence: registered (focus -, branch main, registered 3s ago)
Peers (github.com/petrastella-live-check/qa-b9d2-29dfcb, via Cassy
  Cloud):
  qa-6f85-b-29dfcb [live] on soundwave, heartbeat 3s ago
    owner you
    session qa6f85-b-29dfcb | focus -
Peer mailbox: receiving.
```

Neither supervisor registered by hand: each factory registers its supervisor
with the cloud on start, retries if that fails, and refreshes its focus and
branch on every heartbeat. `Peer mailbox: receiving.` means this factory's
daemon is claiming peer messages.

For a peer's claims, workers and branches, run `cas peers --detail`, or
`cas peers --peer <name>` for one peer:

```text
[OK] 1 peer supervisor | 1 live
Repo github.com/petrastella-live-check/qa-7e32-b251eb
qa7e32-b-b251eb [live]
  Account you
  Machine soundwave
  Session q7e32b-b251eb
  Focus   cas-3775
  Heartbeat 13s ago
  Branch  main
  Claims  3 | age since renewal (s)
       13 cas-3775 [claimed; title not synced]
          Owner qa7e32-b-b251eb
       13 cas-49ec [claimed; title not synced]
          Owner qa7e32-bw2-b251eb
       13 cas-aaf7 [claimed; title not synced]
          Owner qa7e32-bw1-b251eb
  Workers 2 live (read-only)
    qa7e32-bw1-b251eb [active]
      Branch factory/qa7e32-bw1-b251eb
      Tasks cas-aaf7
    qa7e32-bw2-b251eb [active]
      Branch factory/qa7e32-bw2-b251eb
      Tasks cas-49ec
Peer mailbox: receiving.
```

Add `--json` for the same report as one document with untruncated values.

A Claude supervisor keeps its agent id, name, epics and leases across
`/clear`, so peers and `whoami` see the same identity before and after it.
A real session end, such as a logout or exit, still retires the row and hands
off to a successor.

A supervisor also sees its live peers when its session starts, including
after `/clear`. The block reads the same snapshot as `cas peers`:

```text
## Live peer supervisors on this repo (Cassy Cloud)
- qa7e32-b-b251eb on soundwave: heartbeat 13s ago, focus cas-3775, 3 claimed tasks
2 stale (seen in the last 24h, not live)
Their claims and work: `cas peers --detail`. Coordinate before taking the same tasks.
```

There is no block when no peer is live, when the project does not use Cassy
Cloud, or for workers. If the cloud does not answer within 3 seconds, the
block is one line saying the peers are unknown, never that there are none.

### 2. Message and reply

From `soundwave`, address the peer by the name and machine peers showed:

```text
coordination action=message target=sup-prowl-878c15@prowl summary="ping" message="..."
```

```text
Message sent to peer supervisor sup-prowl-878c15@prowl via Cassy Cloud (peer message pm_1, queued): delivered to sup-prowl-878c15@prowl mailbox. Check delivery with coordination action=message_status id=pm_1.
```

On `prowl`, `coordination action=inbox_poll` shows the message labelled as
coming from a peer supervisor:

```text
**[3] From: peer:sup-soundwave-878c15@soundwave**
Summary: peer sup-soundwave-878c15@soundwave: ping live-check-ping-878c15-716abf
[cas #3 peer-supervisor sup-soundwave-878c15@soundwave via-cloud 1s first]
```

Reply with `in_reply_to` set to that row (here `3`):

```text
Reply sent to peer supervisor sup-soundwave-878c15@soundwave via Cassy Cloud (peer message pm_2, queued); message 3 acknowledged. Check delivery with coordination action=message_status id=pm_2.
```

Back on `soundwave`, `coordination action=message_status id=pm_1`:

```text
Peer message pm_1: delivered at 2026-10-13T21:59:26.068369+00:00 (delivery attempts: 1)
```

`queued` in a send receipt means the cloud accepted it; `message_status` tells
you when the peer's daemon claimed it. A peer message never interrupts the
peer (`urgent=true` is refused) and carries no operator authority: the peer
reads it as a colleague's request.

### Supervisors of your other projects

Supervisors in different projects of **your own** Cassy Cloud account can
also message each other, on one machine or across machines. Supervisors of
another user are never listed or addressable, even on a shared team project.

```text
coordination action=peers scope=all
```

```text
[OK] 15 supervisors in 8 projects | 4 live
Live:
  bright-newt-56 gabber-studio on prowl, seen 13s ago
    session gabber-studio-smooth-heron-38
  fierce-shark-94 petra-stella-cloud on soundwave, seen 16s ago
    session petra-stella-cloud-happy-puma-17 | focus cas-8cb0
Stale, seen in the last 24h (11): abundant-mines 2, gabber-studio 5, ...
  List them with --verbose or --json.
Message one: coordination action=message target=supervisor:gabber-studio
  summary="<one line>" message="<text>"
Cross-project messages are advisory: never urgent, never the operator.
```

Address one by project, name, or `name@machine`. A project name that still
names more than one supervisor is refused with every candidate listed:

```text
coordination action=message target=supervisor:gabber-studio summary="retry design?" message="How did you build the hub retry?" attachment=docs/retry.md
```

`attachment` takes a published artifact id, or a file in your checkout as
`path` or `path@<commit>`. The recipient reads a file pointer directly when
it is on the same machine; across machines, publish an artifact. The
recipient sees the sender's project and machine, and the message is
advisory: it is never urgent, cannot raise a blocker, and carries no
operator authority. The Cloud side of this contract is
[petra-stella-cloud#162](https://github.com/Richards-LLC/petra-stella-cloud/issues/162).

### 3. Claims keep you off each other's tasks

A started task `cas-1c99`. B's spawn for it is refused before anything is
queued:

```text
⛔ HELD BY A PEER — task cas-1c99 is claimed by qa-0078-A-a352ab26 on soundwave (a session of this repository on another machine or clone) until 22:39 UTC. A worker spawned for it would duplicate their work, so nothing was queued. Ask them first: mcp__cas__coordination action=message target=qa-0078-A-a352ab26 summary="cas-1c99" message="Are you still on cas-1c99?". To spawn anyway: mcp__cas__factory action=spawn_workers task_id=cas-1c99 force=true
```

`task start` on B is refused the same way, with the holder and the time its
claim runs out. When A closes or releases the task, B can start it. Use
`force=true` only after the holder agrees. A claim nobody is renewing does not
block: the start proceeds on the local lease and says `⚠️  STALE PEER CLAIM`.

### Merging into the same branch

Two supervisors may merge into the same branch. They coordinate and agree;
Cassy never locks, fences or refuses a merge for them. Two helpers inform:

- **About-to-merge note.** When a supervisor starts `worktree_merge` into a
  target branch, every other live supervisor of this project whose factory runs
  on that branch gets one peer message, before the merge starts:
  `About to merge factory/<worker> (<task>) @ <tip> into <target>, by
  <name>@<machine>.` It respects `peers.same_repo_scope` and is sent once per
  merged tip; a retried merge does not send it again. The merge never waits for
  it.
- **Same-file overlap warning.** `cas peers` and the session-start peer block
  warn when one of your in-flight worktree branches and a peer worker's branch
  change the same files on the way into the same target:

  ```text
  [WARN] Same files in flight into epic/burn-down:
    yours  factory/swift-fox-2
    theirs factory/happy-owl-25 (happy-owl@prowl (teammate bob@example.com))
    files  src/BeginConversation.vue
    Agree the merge order by message; nothing is blocked.
  ```

  It reads the branches your git already has (`origin/<branch>` after a fetch),
  skips branches already merged, and gives up quietly after 3 seconds or 24
  branches.

### 4. Split an epic

A proposes who takes which children:

```text
factory action=epic_split id=cas-c018 target=qa-6f85-b-29dfcb mine=cas-fffe,cas-36bd theirs=cas-5c11,cas-bfa7 ttl_secs=1800
```

```text
Proposed epic split split-ef16c467 of cas-c018 to qa-6f85-b-29dfcb@soundwave (peer message f7196831-582e-4a2c-ba77-5f2f419dd6bf, queued).
You took (claimed in Cassy Cloud): cas-fffe, cas-36bd
Offered to qa-6f85-b-29dfcb: cas-5c11, cas-bfa7
Until they answer, starting their share here is refused. The proposal expires at 00:34 UTC.
```

B sees the proposal under `factory action=epic_split` and accepts it with
`factory action=epic_split id=split-ef16c467 decision=accept`:

```text
Accepted epic split split-ef16c467 of cas-c018 with qa-6f85-a-29dfcb@soundwave (peer message c05bc6ef-1ee6-418e-952c-44afe2807783, queued).
You took (claimed in Cassy Cloud): cas-5c11, cas-bfa7
qa-6f85-a-29dfcb keeps: cas-fffe, cas-36bd — starting those here is refused.
```

From then on, A's start or spawn on B's share is refused:

```text
⛔ SPLIT TO A PEER — cas-5c11 belongs to peer supervisor qa-6f85-b-29dfcb@soundwave under epic split split-ef16c467 of cas-c018 (accepted).
Starting it here would duplicate their work. Ask them first: mcp__cas__coordination action=message target=qa-6f85-b-29dfcb@soundwave summary="cas-5c11" message="..."
To end the split: mcp__cas__factory action=epic_split id=split-ef16c467 decision=cancel. To take this child anyway: mcp__cas__task action=start id=cas-5c11 force=true
```

Both peers views list the split between the peers and the mailbox line:

```text
Epic splits:
  split-ef16c467 [accepted, outgoing] epic cas-c018
    with qa-6f85-b-29dfcb@soundwave
    mine cas-fffe, cas-36bd
    theirs cas-5c11, cas-bfa7
    decided 10-14 00:05 UTC
```

Either side ends it with `decision=cancel`, which releases only the
canceller's own claims and tells the peer:

```text
Cancelled epic split split-ef16c467 of cas-c018; released 2 of your own claim(s); told qa-6f85-b-29dfcb@soundwave (peer message 9ae35596-5046-42a8-b38f-118403c9a667, queued).
```

`decision=reject reason="..."` declines a proposal; an unanswered proposal
expires after `ttl_secs` (default 1800).

## Troubleshooting

| You see | Meaning | Do this |
| --- | --- | --- |
| ``Peers: not logged in to Cassy Cloud, so supervisors on other machines are unknown (`cas login`).`` | This project has no Cassy Cloud login. | Run `cas login`, then check peers again. |
| `Peers: this project has no canonical id to match peers by (set [project] canonical_id or a git origin).` | Cassy cannot tell which repository this is. | Add a git origin, or set `[project] canonical_id` in `.cas/config.toml`. |
| `Peers: Cassy Cloud lookup failed: <error>` | The agent list could not be read. | Check the error (network, `401` for a bad login); the next call retries. |
| `Cloud presence: failing: <cause>` followed by `<n> failed attempts, next retry in <time>.` and `Peers cannot see this agent until it registers.` | This supervisor is not registered, so peers cannot see or message it. | Fix the cause (login, network). The heartbeat retries on its own, backing off to every 5 minutes; check peers again afterwards. |
| `Cloud presence: not registered yet (no registration attempt recorded for this agent)` | No factory daemon has registered this agent yet. | Start the factory, or wait one heartbeat (30 s). |
| `Cloud presence: disabled (cloud.auto_sync=false): …` | The project opted out of cloud sync. | Remove `cloud.auto_sync = false` if this supervisor should be visible to peers. |
| ``Peer mailbox: not receiving: not logged in to Cassy Cloud (`cas login`)`` with `Retry: automatic with backoff, at most 30s.` | The daemon cannot claim peer messages. | Run `cas login`; receiving resumes within 30 s without a restart. |
| `Peer mailbox: not receiving: no factory daemon session.` | This supervisor is not running inside a factory. | Start it with `cas factory`. |
| `Peer mailbox: not receiving: daemon mailbox status is stale.` or `daemon has not reported a mailbox poll.` | The factory daemon stopped polling. | Check that the factory is running; restart it if not. |
| `<name>@<machine> is not running a factory daemon, so it cannot receive peer messages` | The peer is listed but has no receiving mailbox. | Ask its operator to start `cas factory`, then send again. |
| `no supervisor or worker named <target> in this project or its peers. Nothing was enqueued.` | No local supervisor, same-session worker, or peer supervisor has that name. Worker-to-worker routing is limited to registered workers in the same factory session; each peer message is copied to that session's supervisor. | For a supervisor peer, copy the name from `coordination action=peers`. For a worker peer, use its registered factory name. |
| `'<target>' names <n> peer supervisors; address one as <a>@<machine> or <b>@<machine>` | Two peers share the name. | Add `@<machine>`. |
| ``Could not resolve message target '<target>' through Cassy Cloud: no token (not logged in; `cas login`). Nothing was enqueued.`` | Not logged in, so peers cannot be resolved. | Run `cas login`. |
| `Could not deliver to <name>@<machine> mailbox: peer message refused (404): {"error":"recipient_not_found"…}` | The cloud cannot address that recipient in this project (unknown, wrong project or not visible to you; the cloud does not say which). Not retried. | Check peers again and confirm both clones resolve to the same project. |
| `Could not deliver to <name>@<machine> mailbox: <cause>; queued for automatic retry as peer-outbox:<id>` | A temporary failure (network, 5xx, 408, 429). The send is kept and retried for up to 15 minutes. | `coordination action=message_status id=peer-outbox:<id>` shows its state. Do not resend: the retry keeps the same identity, so the peer gets it once. |
| Inbox notice `Message peer-outbox:<id> to <recipient> was abandoned as undeliverable: <reason>` | Retrying ran out (`retry TTL expired`) or hit a permanent refusal. | Fix the named cause, then send again. |
| `Claims visible (team project)` in peer details | Cloud authorizes the teammate's claims for this shared project. | Coordinate with the named owner before taking its work. |
| `⛔ HELD BY A PEER — task <id> is claimed by <who> … until <time>.` | Another session of this repository holds the claim. | Message the holder. `force=true` only after it agrees or its claim is stale. |
| `⛔ SPLIT TO A PEER — <id> belongs to peer supervisor <name>@<machine> under epic split <split> …` | The child is the other side's share. | Ask the peer, or end the split with `decision=cancel`. |
| `⚠️  PEERS NOT CHECKED — Cassy Cloud is unreachable (<reason>), so claims on <id> from other machines could not be checked. Started on the local lease only.` | The start went ahead without the cross-machine check. | Check peers before dispatching more work on that epic. |
| `⚠️  PEER CLAIM LOST — claim on <id> lost to <peer>@<machine>; stop work or coordinate.` | Your claim ran out and a peer took the task. | Stop the work or message that peer. |

## How the cloud behaves

- Discovery asks for this project only (`?project=<canonical id>`). A peer whose
  last heartbeat is older than 5 minutes is shown `[stale]`; peers not seen for
  24 hours are not listed.
- Every heartbeat (30 s) re-sends the agent's project, focus, branch, factory
  session and hostname, so a reconnect repairs discovery on its own. A heartbeat
  the cloud answers with 404 or 401 registers the agent again.
- A claim lasts the configured lease (at most an hour) and is renewed every two
  minutes while its task's lease is held. An expired claim is free. Claiming a
  task you already hold does not extend it; renewal does.
- Peer messages live in the cloud for 7 days. A message is delivered once even
  if the send is retried; two separate sends of the same text both arrive.
