# Hosted queue precheck

| Field | Decision |
| --- | --- |
| First two lines | Show the hosted run state and the command to read it again. |
| Scannable | Show one run state, the full commit SHA, then the run and receipt locations. |
| Readable | The completed status compares its elapsed minutes with the October dispatches. |
| Machine output | `--json` returns one object with `sha`, `ref`, `repository`, `run_id`, `run_url`, `status`, `conclusion` and `dispatched_at`; status adds `jobs`, `elapsed_minutes` and `october_dispatch_minutes`. |
| Omitted | Job details and complete paths stay in JSON and the durable receipt. |

The command embeds its dispatcher, pushes the requested commit without changing
HEAD and starts `ci.yml`. Precheck refs select required queue jobs on hosted
runners, without the advisory manual-dispatch builds. The requested commit owns
its workflow definition; historical commits use their historical CI definition.

Design: `DESIGN.md@b43940e43`.
