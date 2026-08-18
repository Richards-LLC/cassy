# BUG: Agent staged 17GB into tmpfs /tmp — no guardrail, host constraints trapped in project-scoped memory

**Filed:** 2026-07-22
**Source:** Penguinz factory supervisor (session `5340a10a-0f8b-4c78-b471-8f9b9bf0ddf4`), incident produced by ozer project session `f9b6f755-c16c-4882-b7b9-a709aef5c979`
**Severity:** high — degrades the operator's whole machine and put sole copies of work product at reboot-loss risk
**Classification:** systemic gap (two halves: memory scoping + missing write guardrail)

## Incident

Between 2026-07-05 and 2026-07-21, agent sessions in the ozer project staged sleep-widget
audio into `/tmp` on soundwave: `/tmp/sleep-masters` (13GB of "OZER SLEEP-*HR app.mp3"
masters), `/tmp/sleep-app-renditions` (3.8GB of duration-ladder m4a renditions +
manifest.tsv), `/tmp/sleep-probe` (550MB duplicate). Paths were chosen ad hoc
mid-conversation — no committed script references them.

soundwave's `/tmp` is a **32GB tmpfs**. Consequences:

1. 17GB of write-once, read-rarely media became resident memory; cold pages were
   swapped out and **saturated the 16GB swapfile to 100%**, permanently.
2. With zero swap headroom, the kernel OOM-killed the operator's applications
   repeatedly over days. User quote: "I can't have my machine overloading by using
   my main tool."
3. The tmpfs copies were the only local copies of the assets — **one reboot from
   total loss** of the masters.

## Root cause

1. **Host constraint trapped in project scope.** The lesson "soundwave /tmp is tmpfs;
   stage large files on /mnt/datacube or /home" had already been learned and stored —
   in the *Penguinz* project's memory. The ozer session never surfaced it. Machine-level
   facts (mount semantics, disk budgets, staging conventions) are per-host, not
   per-project, but Cassy memory recall is project-scoped by default and nothing promotes
   or injects host-level constraints across projects on the same machine.
2. **No write guardrail.** Nothing in Cassy (hooks, rules, worker prompts) warns or
   blocks when a session writes multi-GB files to a tmpfs-backed mount — a silent,
   cumulative failure mode whose blast radius is the host, not the project.

## Suggested fixes

1. **Host-scoped memory class:** a `host`/`machine` scope (or convention + auto-tag)
   whose entries are injected into session context for *every* project running on that
   hostname. Mitigation applied meanwhile: a global-scoped Cassy memory entry
   (2026-07-22-4) documenting the constraint.
2. **tmpfs write guardrail:** PreToolUse-style check (or worker rule): before/after
   large writes, if the target path resolves to a tmpfs/ramfs mount (`findmnt -T`) and
   the write exceeds a threshold (e.g. 1GB cumulative), warn and require the agent to
   restate an approved staging location. Cheap, catches the whole class.
3. **Per-host staging convention in config:** e.g. `config.toml` `[host.soundwave]
   staging_dir = "/mnt/datacube/staging"` that worker/supervisor prompts surface as
   the default for large artifacts.

## Evidence

- `df -h /tmp` → tmpfs 32G, 17G used (54%) at time of diagnosis
- `free -h` → 62Gi total, swap 15Gi/15Gi used (1.5MiB free)
- File mtimes: masters 2026-07-05→08, renditions 2026-07-21 09:41–10:25
- Transcript hit: `~/.claude/projects/-home-pippenz-Petrastella-ozer/f9b6f755-*.jsonl`
  contains the `/tmp/sleep-masters` staging decision
- No repo references: `grep -rln "/tmp/sleep" ~/Petrastella` (excl. node_modules) → empty

---

## Resolution (2026-07-22)

Shipped in v2.28.4 (epic merge c329363). All three suggested fixes implemented:
1. **tmpfs write guardrail** — warning-only PostToolUse check tracking per-session written bytes AND usage growth per memory-backed mount (separate accounting, flocked state, single-shot Bash fills caught on first sample, all tmpfs/ramfs mounts enumerated incl. /dev/shm). Hot-path safe: zero I/O for non-Write/Edit/Bash calls.
2. **Host-scoped memory** — global memories tagged `host:<hostname>` inject into SessionStart for every project on the machine (SQLite query-layer filtered, size-capped). The soundwave tmpfs constraint memory migrated as first user.
3. **Per-host staging convention** — `[staging] large_artifact_dir` (host-level fallback, staging-section-only merge), surfaced in supervisor/worker SessionStart guidance and named in guardrail warnings.

Immediate mitigation same day: 17.3GB rsynced to /mnt/datacube/staging with symlinks left in /tmp; swap pressure released (free RAM 1.8Gi → 14Gi).
