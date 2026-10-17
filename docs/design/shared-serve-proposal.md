# Proposal: one serve per canonical project database

**Decision for v47 (2026-10-17, cas-77d6):** defer the shared-service prototype and macOS allocator changes after the measurements below. Sharing remains a separate operator decision; this delivery adds visibility. The October 16 host incident counted 33 independent servers holding 12.5 GiB RSS plus swap. The reproduced dominant cost is freed glibc arena memory, so sharing alone would leave that retention problem inside the shared process.

**Topology.** Keep `cas serve` as each harness's stdio child, but make it a small authenticated Unix-socket client. Start one user-owned service per canonical project database (device/inode identity, with replacement detection), using a socket in a private 0700 user runtime directory. Worktree paths resolving to the same database use that service; different databases and Unix users stay separate. This avoids a host-wide process mixing every project's state. The service owns pooled SQLite connections, Tantivy readers/mappings, maintenance ownership and upstream lifetimes. The stub carries only transport buffers and its session binding. Existing per-process idle-memory limits remain necessary in the service.

**Identity and isolation.** Bind each socket connection to an immutable caller context at handshake: registered agent/session, factory session, role, account, canonical database, worktree/cwd and harness capabilities. Verify peer UID with OS credentials and the registration/start capability; never accept a caller-supplied role as authority. Include connection identity when routing JSON-RPC IDs, notifications and replies. Service handlers must take explicit caller context: today's environment variables, cwd, global account/config state and cached `CasCore.agent_id` cannot represent multiple sessions safely. Scope worker credential restrictions and proxy policies per connection. Upstreams with different account credentials or policy fingerprints cannot be shared.

**Lifecycle and recovery.** Elect service startup using a nonblocking lock, probe socket protocol/build/schema compatibility, and reject incompatible clients with a clear restart instruction. Maintain one session lease per stub, releasing only that session's claims on disconnect. A lost connection during a mutation returns an unknown/committed receipt; never automatically replay writes. Drain the service after the last session and an idle grace, retaining no orphan maintenance writer. Reconnect reads with bounded backoff; let an ordinary stdio fallback remain available if the service is unavailable. Limit per-session queued bytes, in-flight requests and response sizes so one worker cannot exhaust the shared service.

**Risks and migration.** A shared crash affects every connected session; one slow synchronous handler can delay peers; identity or notification mistakes can cross account boundaries; upgrades and database replacement complicate pooling. First refactor explicit caller context under existing stdio behavior and test two simultaneous sessions with different roles/accounts/cwds. Then ship socket transport behind an opt-in setting, exercise crash/disconnect/upgrade and memory soaks, and compare aggregate private RSS plus swap, latency and upstream counts against independent stdio. Keep old and new protocols versioned; roll back by selecting stdio, without changing stored data. Make sharing the default only after isolation and failure recovery pass. The operator owns that rollout decision.


## Follow-up measurements and decisions (cas-77d6)

Read-only samples on 2026-10-17; raw process rows and the reproducible Linux sampler
are published as `art-4857100c`. These are observations under ongoing factory work,
not a controlled before/after allocator experiment or a six-hour soak.

| Sample | Processes | RSS MiB | Swap MiB | PSS MiB | Private resident MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| soundwave, 11:16:11 UTC | 11 | 1313.8 | 114.4 | 881.7 | 837.6 |
| soundwave, 214 s later | 11 | 1061.4 | 116.6 | 630.6 | 587.5 |
| prowl, first native ps | 3 | 530.9 | unavailable | unavailable | unavailable |
| prowl, 226 s later | 3 | 484.0 | unavailable | unavailable | unavailable |

**Shared serve: defer implementation in v47.** All 11 Linux processes use the
same canonical project database; their ages are 24–2099 s in the first sample.
RSS includes shared pages, so adding it overstates physical cost. Even deleting
*all* private resident and swapped pages would save at most 952.0 MiB initially,
704.1 MiB at the repeat. That is an optimistic ceiling, not an achievable saving:
a shared server must retain live session state and its own baseline. The host
has 62.6 GiB RAM and about 48 GiB available at observation. Current measurements
do not justify the caller-identity, per-account policy, mutation-recovery and
shared-crash changes in the proposal above. No shared prototype was measured;
revisit if post-update long-session measurements show renewed pressure.

**macOS reclamation: defer allocator changes in v47.** Native prowl SSH was
restricted to read-only ps/RSS: three processes aged 5–6 minutes initially,
139.2–198.1 MiB RSS each; the same PIDs totaled 484.0 MiB 226 s later, with a
207.4 MiB maximum. The exact proposed reporting command, `ps -p 35242 -o rss=`,
returned `212400` KiB. This proves the native RSS source, not allocator parity.
No version inspection, installs, restarts, allocations or pressure probes were
authorized there, so no glibc-equivalent retention defect or reclamation benefit
has been measured. Keep the Linux/glibc trim as-is; native allocator work needs
a representative macOS long-session workload and a permitted control/treatment
experiment before changing its allocator. No macOS memory fix is claimed.

**Delivered visibility.** `factory action=gc_report` reports per-session RSS and
a summed byte count for registered local project sessions. It verifies process
start times before and after sampling, deduplicates aliases by PID, excludes
foreign-machine and recycled-PID records, and marks read failures unavailable.
Legacy registrations without a start-time fingerprint are omitted. Linux reads
`/proc/<pid>/status`; macOS uses native `ps` RSS (KiB converted to bytes).
This is project-scoped RSS visibility, not host-wide PSS or swap accounting.
