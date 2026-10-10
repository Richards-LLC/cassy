# Many-agent load harness

`scripts/many-agent-load.py` reproduces the shared-database stall from GH #1165 (2026-10-10). It runs N real `cas serve` MCP processes and a headless factory daemon against a scratch copy of a project database. It measures latency and checks the many-agent SLOs. It uses only the Python standard library and reads `/proc`, so it runs on Linux only.

## What it runs

- **Agents.** N `cas serve` processes, 16 by default. Agent 0 is the factory supervisor, because peer messages need one registered in the session; the rest are workers. Each agent creates one chore task, then loops on a weighted mix until the run ends:
  - task `show`, `list`, `ready`, `mine`
  - task `notes` and `update` on its own task
  - coordination `message` to a peer, `inbox_poll` and `heartbeat`
  - memory `remember`
  - `search`

  Agents think for an exponential pause (mean `--think-mean`, 3 s) between calls and keep one call in flight, as a real agent does.
- **Background write pressure.** Synthetic write-lock holders. See [Background write pressure](#background-write-pressure).
- **Factory daemon.** `cas factory daemon --workers 0 --no-worktrees`. The supervisor CLI is a stub: it answers the daemon's `--help` and `--version` probes with the installed CLI's text and otherwise sleeps. Turn it off with `--no-daemon`.
- **Background work.** Whatever the binary starts on its own: each `cas serve`'s maintenance daemon, cloud sync attempts and the canonical process's code index. Pass `--project-src <repo>` (and `--project-rev`) so the code index has real source to work on.

## Background write pressure

N agents alone do not reproduce the stall. With 16 agents and no other load, the 0d670c33c baseline met the latency SLOs: p99 was 0.14 s and nothing waited on the lock. The field stall needed a background process holding the WAL write lock: during the GH #1165 window, one `cas serve` read 253 GB while holding it. A scratch database has no such job, so the harness adds its own declared pressure: `--bg-writers` threads that open `BEGIN IMMEDIATE`, insert a 4 KiB row and hold the transaction before committing.

| Profile | Flags | Use |
|---|---|---|
| Standard (default) | `--bg-writers 2 --bg-hold-ms 400 --bg-gap-ms 600` | SLO receipts. Writes contend, but a build that does not convoy reads behind writes can meet the SLOs. |
| Stress | `--bg-writers 3 --bg-hold-ms 800 --bg-gap-ms 400` | Field-like severity. On 0d670c33c it gave 10 s lock waits and 13 s daemon passes. |
| None | `--bg-writers 0` | Agents and daemon only. |

Hold and gap times are randomised between 0.5x and 1.5x. The receipt records the profile and how many transactions committed, so receipts from different builds are compared under the same pressure.

## Isolation

- **Database.** The live database is opened read-only and copied with SQLite's online backup into `<scratch>/db-cache/cas.db`. Each run gets its own copy. The harness refuses a `--scratch` inside the live `.cas`.
- **Environment.** Every child gets a scratch `HOME`, a scratch `CAS_ROOT` and a fake cloud token. Their cloud endpoint is `http://127.0.0.1:9`, a closed port, so nothing leaves the host. They still run logged in, as the field fleet did, so the syncing task store and `task-sync-intents.lock` are in play.
- **Cleanup.** The scratch database copy is deleted after the run unless you pass `--keep-scratch`. Logs stay in the run directory.

## What it measures

| Signal | Source |
|---|---|
| Per-call latency p50, p99 and max for each tool and action, plus calls at the 55 s deadline | Harness timing of every `tools/call` |
| SQLite busy and write-lock retry warnings, per minute | `<scratch .cas>/logs/*.log` |
| Waits on `task-sync-intents.lock` and its per-task stripes (`task-sync-intents.d/*.lock`) of at least 0.4 s | `/proc/locks`, sampled every 200 ms |
| Daemon loop pass health | `.cas/factory-daemon/<session>.loop.json`, plus the daemon main thread's `wchan` every 200 ms |

The daemon writes `loop.json` every 5 s. Builds with cas-04db add per-window pass timings to each snapshot: `window_passes`, `passes_over_100ms`, `p99_pass_ms` and `max_pass_ms`. With those fields, the run's pass p99 is under 100 ms exactly when fewer than 1% of the timed passes took 100 ms or more. Older builds such as 0d670c33c lack them, so the harness reports a lower bound instead: the larger of the longest pass seen in progress (minus the 0.5 s headless sleep) and the mean pass estimate.

## SLOs

The run passes only when all three hold:

- MCP `task` and `coordination` p99 is under 2 s.
- No call reaches the 55 s MCP deadline.
- The daemon loop pass p99 is under 100 ms.

The exit status is 0 when every SLO holds, 1 when one is violated and 2 when the run produced no calls.

## Run it

```bash
A=~/.cas/artifacts/<project-key>/<task-id>
python3 -I scripts/many-agent-load.py \
  --cas-bin ~/.local/bin/cas \
  --label baseline-0d670c33c \
  --agents 16 --duration 300 \
  --source-db ~/Petrastella/cassy/.cas/cas.db \
  --source-config ~/Petrastella/cassy/.cas/config.toml \
  --project-src . --project-rev 0d670c33c \
  --scratch /mnt/rewind/cas/scratch/load-harness \
  --out "$A"
```

Use a scratch root on disk. `/tmp` is tmpfs on the factory hosts, and the database copy is about 1.6 GB. To compare builds, run the same command with `--cas-bin` pointing at each binary and a different `--label`. Change `--agents` for scale: 16 is the field fleet size, and 32 is the stretch point.

## Outputs

Each run writes three files to `--out`:

- `load-<label>-n<N>.json`: the receipt. It holds the binary version, config, per-tool latency, lock waits, busy warnings, the daemon summary and the SLO verdicts.
- `load-<label>-n<N>.md`: a Markdown summary of the receipt.
- `load-<label>-n<N>.calls.jsonl`: one row per call.

Per-process stderr and the daemon log stay in the run directory under `--scratch`.

## Not modelled

- Claude and Codex hook processes (`cas hook ...`), which each open the database once per tool use.
- Real worker PTYs.

Both add load in the field, so a pass here is necessary but not sufficient.
