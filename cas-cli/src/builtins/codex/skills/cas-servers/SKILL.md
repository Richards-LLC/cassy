---
name: cas-servers
description: Use when starting, inspecting, or stopping a long-lived local server, watcher, API stub, preview, or Playwright webServer.
managed_by: cas
---

# Long-running servers go through the registry

**Never background a server yourself.** `npm run dev &`, `nohup ... &`, `setsid ... &` and
friends are the ambient-orphan pattern: nothing records that the process exists, nothing
knows which task it belongs to, and nothing can stop it except hunting through `ps` and
`lsof`.

Start it through Cassy instead:

```
mcp__cs__coordination action=server_start command="npm run dev" port=5173 task_id=<your task>
```

**Registered servers are the only ones that survive worker teardown.** When a worker is torn
down, Cassy kills its entire process group *and* its containment cgroup — including descendants
that detached themselves with `setsid` (which is what Node's `spawn(..., {detached: true})`
does, and therefore what Playwright's `webServer` and most `npm run dev` wrappers do). An
unregistered server has no way to escape that. A registered one is placed outside the
worker's containment scope on purpose.

## The three actions

### Start

```
mcp__cs__coordination action=server_start command="npm run dev" cwd=apps/web port=5173 task_id=cas-1234
```

| Parameter | Meaning |
|---|---|
| `command` | **Required.** The shell command, exactly as you would type it. Runs under `sh -c`. |
| `cwd` | Where to run it. Defaults to the current directory. |
| `port` | The port you expect it to bind. Advisory — `server_list` reports what it *actually* bound. |
| `task_id` | The task this server belongs to. Always set it: it is how a supervisor knows who to ask. |
| `id` | A short name (`dev-web`). Defaults to something derived from the command. |
| `shared` | `true` when the server must outlive your task. Default `false`. |

Output and errors go to a log file, never to your terminal — the path is in the response.
A server that dies on startup leaves its reason in that log.

### Choosing `shared`

- **`shared=false` (default)** — a server *you* need for *this* task: a dev server you are
  about to run tests against, a stub API for one integration check. It lives in your
  worker's containment scope and dies when your worker is torn down. That is the correct,
  tidy default: no leftovers.
- **`shared=true`** — a service that is *supposed* to outlive the task, or that other
  workers use: a preview build the supervisor will look at, a long-lived database
  container, a dev server several workers hit. It is placed outside worker containment,
  so **you are responsible for stopping it.** Say so in your handoff if you leave one running.

### List

```
mcp__cs__coordination action=server_list
mcp__cs__coordination action=server_list task_id=cas-1234
```

Answers "what is listening, and who started it" — name, pid, the ports actually bound, the
owning task and worker, the command, the cwd, and whether the entry survives teardown.
Recently stopped and dead entries stay visible as history so "what happened to it?" has an
answer. A pid that has gone away is reported dead; Cassy never restarts anything on its own.

### Stop

```
mcp__cs__coordination action=server_stop id=dev-web
```

Takes the name or the id from `server_list`. Stops the whole server, not just its wrapper
script — `npm run dev` is a launcher whose real server is a child process.

Cassy refuses to signal a pid it cannot prove is still the process it started (pid reuse
happens on long-lived machines). If you see that refusal, the server is already gone; the
registry entry is marked dead and nothing was killed.

## Rules

1. **Any process that keeps running after the command returns goes through `server_start`.**
   If you catch yourself typing `&`, `nohup`, `setsid`, `screen` or `tmux` to keep something
   alive, that is the signal.
2. **Always pass `task_id`.** An unattributed server is the thing this registry exists to end.
3. **Check before you start.** Run `server_list` first — the server you need may already be
   running, started by another worker. Starting a second one usually just fails to bind.
4. **Stop what you started**, especially anything `shared=true`. `server_stop` before you
   close your task, or name it explicitly in your handoff.
5. **One-shot commands do not belong here.** Builds, tests, migrations, scripts that exit on
   their own — run them normally and wait for them.

## Not for

- Commands that finish by themselves (`npm run build`, `cargo test`, `pytest`).
- Anything you need the output of *right now* — the registry captures output to a log
  rather than returning it.
- Background work that is really a task: use a factory worker, not a server.
