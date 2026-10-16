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
- **Synthetic PTY writers** (factory loop v2 C6). With `--synthetic-writers 38,63,236`, the daemon starts one worker pane per rate (`--workers 3`). The stub CLI in each worker pane prints a codex-like status line at that rate, in writes per second. These are GH #1210's measured codex rates. Each write carries a sequence number, so the receipt records the write rate that reached a client.
- **Key-to-echo probe.** On by default with the daemon; turn it off with `--no-echo-probe`. The probe connects to the daemon's GUI socket the way Commander and the desktop app do. It then repeats:
  - type one key into the supervisor pane, which is quiet because its stub sleeps;
  - time how long until an `Output` frame carries the terminal's echo back;
  - clear the line with ^U;
  - wait `--echo-interval`, 0.1 s by default.

  A key with no echo within 5 s counts as 5 s.
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
| Key → echo latency p50, p99 and max, plus timeouts | The echo probe on the GUI socket |
| Delivered synthetic writer rates | Write sequence numbers in the `Output` frames the probe receives |
| Host load | `/proc/loadavg` every 5 s: start, middle, end and the 1-minute range |

The daemon writes `loop.json` every 5 s. Builds with cas-04db add per-window pass timings to each snapshot: `window_passes`, `passes_over_100ms`, `p99_pass_ms` and `max_pass_ms`. With those fields, the run's pass p99 is under 100 ms exactly when fewer than 1% of the timed passes took 100 ms or more. Older builds such as 0d670c33c lack them, so the harness reports a lower bound instead: the larger of the longest pass seen in progress (minus the 0.5 s headless sleep) and the mean pass estimate.

## SLOs

The run passes only when all four hold:

- MCP `task` and `coordination` p99 is under 2 s.
- No wait of 0.4 s or more on the shared `task-sync-intents.lock` lease. Waits on the per-task stripes (`task-sync-intents.d/NN.lock`) are same-task serialization and are reported, not judged.
- No call reaches the 55 s MCP deadline.
- The daemon loop pass p99 is under 100 ms: under 1% of daemon-timed passes take 100 ms or more. When the daemon reports `phase_latency` (cas-ee9ab), the receipt totals it per loop phase, so slow passes are attributed to a phase.
- With the echo probe on: key → echo p99 is under 50 ms.

## Factory loop v2 receipts

The receipt's `flv2_receipts` rows hold the two receipt budgets from spec §7 (`docs/specs/2026-10-15-factory-loop-v2.md`):

- `flv2_control_pass_p99_under_100ms_with_16_agents`: the daemon pass p99, met only with at least 16 live agents;
- `flv2_key_to_echo_p99_under_50ms_under_load`: the echo p99, met only with at least 16 live agents and every configured synthetic writer delivering.

An agent whose `cas serve` failed is not counted as live. A serve that exits during boot is restarted, up to `--boot-retries` times (2 by default). Each restart is recorded in `agent_boot_failures`, so a boot bug stays visible rather than thinning the load; cas-30e3 is one such bug. Writer rates compare as a multiset.

Each row also records:

- the binary path, its SHA-256 and its version;
- the configured and live agents;
- the writer rates and the pressure profile;
- the host's 1-minute load range and CPU count.

These are reference-host receipts, not merge gates. The factory hosts are shared, so read a receipt next to its load average and the `no_pressure` control. The factory loop v2 receipt command is:

```bash
python3 -I scripts/many-agent-load.py --cas-bin <candidate> --label flv2-<sha> \
  --agents 16 --duration 300 --synthetic-writers 38,63,236 \
  --source-db ~/Petrastella/cassy/.cas/cas.db \
  --scratch /mnt/rewind/cas/scratch/load-harness --out "$A"
```

The verdict comes from the run as configured, which by default is the standard pressure profile. After a run with pressure, the harness runs a second phase on a fresh copy with the background writers off and records it in the same receipt under `no_pressure`. Turn that off with `--no-control`.

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
  --code-index --prewarm-secs 20 \
  --scratch /mnt/rewind/cas/scratch/load-harness \
  --out "$A"
```

Options worth knowing:

- `--code-index` / `--no-code-index` set `[code] enabled` in the scratch config. Without either flag, the copied config decides.
- `--prewarm-secs` runs one supervisor-role `cas serve` alone before the load. It waits for that serve's one-time boot work to finish:
  - the cas-8256 purge of non-canonical code-index copies, about 39 s on the cassy store;
  - with indexing on, the first index build.

  That work therefore stays out of the measured window. A worker-role serve only reads the code index, so it would never do this work.
- `--exclude TOOL.ACTION` drops one workload entry for diagnosis, for example `--exclude coordination.message`. It can be repeated.
- `--bg-writers`, `--bg-hold-ms` and `--bg-gap-ms` select the pressure profile.

Use a scratch root on disk. `/tmp` is tmpfs on the factory hosts, and the database copy is about 1.6 GB. To compare builds, run the same command with `--cas-bin` pointing at each binary and a different `--label`. Change `--agents` for scale: 16 is the field fleet size, and 32 is the stretch point.

## Outputs

Each run writes three files to `--out`:

- `load-<label>-n<N>.json`: the receipt. It holds:
  - the binary version and harness commit;
  - the configuration and pressure profile;
  - the prewarm report and code-index counts at copy, load start and end;
  - per-tool latency, lock waits (by lock file) and busy warnings;
  - the daemon summary, including its slow 5 s windows;
  - the SLO verdicts and the `no_pressure` control.
- `load-<label>-n<N>.md`: a Markdown summary of the receipt.
- `load-<label>-n<N>.calls.jsonl`: one row per call.

Per-process stderr and the daemon log stay in the run directory under `--scratch`.

## Not modelled

- Claude and Codex hook processes (`cas hook ...`), which each open the database once per tool use.
- Real agent CLIs in the worker PTYs. The synthetic writers reproduce their output rates, not their input handling or rendering.

Both add load in the field, so a pass here is necessary but not sufficient.

`scripts/test-many-agent-load.py` covers the stub writer, which runs in a real PTY, plus the GUI framing, the echo probe against a fake daemon socket and the receipt rows.
