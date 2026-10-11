#!/usr/bin/env python3
"""Background journey evaluation on green integration tips (cas-bb5e).

A full journey evaluation costs about 12.5 minutes of four browsers (about
3.7 cores, 3.5 GiB). The factory daemon therefore evaluates a hub-web/dist
tree in the background only when:

- the integration that produced it is green;
- that dist tree has not been evaluated (or is not being evaluated) already;
- no evaluation is running: a running one is never cancelled, and when it
  finishes only the newest pending tree starts (intermediate ones are skipped);
- the build guard reports the host idle (no cargo builders);
- at least --min-interval-secs (default 1800) passed since the last start.

The receipt is keyed by the dist tree and also records the evaluated journey
inputs (the hub-web tree with its specs, the catalog, the runner scripts). The
cut's `journey-eval.sh --full` reuses it only when all of them match; the
taste-lane report stays a cut-time step.

  journey-background.py offer --repo R --tip SHA [--green]
  journey-background.py tick  --repo R [--host-idle] [--min-interval-secs N]
  journey-background.py run   --repo R --tip SHA --tree TREE --artifacts DIR
  journey-background.py reuse --repo R --artifacts DIR
"""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

SCHEMA = 1
DEFAULT_MIN_INTERVAL = 1800
RUNNER_INPUTS = ("scripts/journey-eval.sh", "scripts/journey-receipt.py",
                 "scripts/journey-bundles.py", "scripts/journeys-for-diff.py")
REUSED_FILES = ("journeys", "playwright", "journey-receipt.json", "journey-selection.json")
# Harness identity must not reach the runner: journey-receipt.py reserves
# --full for a supervisor or an unset role.
SCRUBBED = ("CAS_AGENT_ROLE", "CAS_AGENT_NAME", "CAS_AGENT_ID", "CAS_SESSION_ID",
            "CAS_SUPERVISOR_NAME", "CAS_FACTORY_SESSION")


# Pure scheduling ---------------------------------------------------------

def empty_state():
    return {"schema": SCHEMA, "last_started_epoch": 0, "running": None, "pending": None,
            "receipts": {}}


def offer(state, tip, tree, evaluated_inputs, green, now):
    """Queue `tree` (built at `tip`) for evaluation, newest wins."""
    if not green:
        return state, "skip: integration not green"
    if tree in state["receipts"]:
        return state, "skip: dist tree already evaluated"
    running = state.get("running")
    if running and running["tree"] == tree:
        return state, "skip: dist tree is being evaluated"
    pending = state.get("pending")
    if pending and pending["tree"] == tree:
        return state, "skip: dist tree already pending"
    skipped = []
    if pending:
        skipped = pending.get("skipped", []) + [pending["tree"]]
    state["pending"] = {"tree": tree, "tip": tip, "inputs": evaluated_inputs,
                        "offered_epoch": now, "skipped": skipped}
    return state, "pending"


def reap(state, alive, now):
    """A runner that died without recording its result failed."""
    running = state.get("running")
    if running and not alive(running["pid"]):
        state = record_finish(state, running["tree"], "FAIL", running["tip"],
                              running.get("artifacts", ""), running.get("inputs", {}),
                              "runner exited without a result", now)
    return state


def decide(state, now, host_idle, min_interval, alive):
    """Return ("start", pending) or ("wait", reason). Never cancels a run."""
    state = reap(state, alive, now)
    if state.get("running"):
        return "wait", "evaluation running"
    pending = state.get("pending")
    if not pending:
        return "wait", "nothing pending"
    elapsed = now - state.get("last_started_epoch", 0)
    if elapsed < min_interval:
        return "wait", f"rate cap: next start in {int(min_interval - elapsed)} s"
    if not host_idle:
        return "wait", "host busy"
    return "start", pending


def record_finish(state, tree, status, tip, artifacts, evaluated_inputs, detail, now):
    running = state.get("running")
    if running and running["tree"] == tree:
        state["running"] = None
    state["receipts"][tree] = {"status": status, "tip": tip, "artifacts": artifacts,
                               "inputs": evaluated_inputs, "detail": detail,
                               "finished_epoch": now}
    return state


def lookup(state, tree, evaluated_inputs):
    receipt = state["receipts"].get(tree)
    if receipt and receipt["status"] == "PASS" and receipt.get("inputs") == evaluated_inputs:
        return receipt
    return None


# Repository and state I/O ------------------------------------------------

def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True,
                          text=True).stdout.strip()


def shared_root(repo):
    common = Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    return common.parent


def state_path(repo):
    return shared_root(repo) / ".cas/merge-sweeps/journey-background.json"


def load(repo):
    try:
        state = json.loads(state_path(repo).read_text())
        if state.get("schema") == SCHEMA:
            return state
    except (OSError, ValueError):
        pass
    return empty_state()


def save(repo, state):
    path = state_path(repo)
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temp.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n")
    os.replace(temp, path)


@contextlib.contextmanager
def locked(repo):
    path = state_path(repo).with_suffix(".lock")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def dist_tree(repo, rev):
    return git(repo, "rev-parse", f"{rev}:hub-web/dist")


def inputs(repo, rev):
    """Everything besides the bundle that decides what the run evaluated."""
    runner = hashlib.sha256()
    for name in RUNNER_INPUTS:
        runner.update(f"{name}\0{git(repo, 'rev-parse', f'{rev}:{name}')}\n".encode())
    return {"hub_web": git(repo, "rev-parse", f"{rev}:hub-web"),
            "catalog": git(repo, "rev-parse", f"{rev}:docs/qa/journeys.md"),
            "runner": runner.hexdigest()}


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


# Commands ----------------------------------------------------------------

def command_offer(args):
    tree = dist_tree(args.repo, args.tip)
    evaluated = inputs(args.repo, args.tip)
    with locked(args.repo):
        state, outcome = offer(load(args.repo), args.tip, tree, evaluated, args.green, int(time.time()))
        save(args.repo, state)
    print(f"journey-background offer {tree[:12]}: {outcome}")
    return 0


def command_tick(args):
    with locked(args.repo):
        state = load(args.repo)
        now = int(time.time())
        action, detail = decide(state, now, args.host_idle, args.min_interval_secs, alive)
        if action == "wait":
            save(args.repo, state)
            print(f"journey-background tick: {detail}")
            return 0
        root = shared_root(args.repo)
        artifacts = root / ".cas/merge-sweeps/journey-runs" / detail["tree"]
        artifacts.mkdir(parents=True, exist_ok=True)
        log = (artifacts / "background.log").open("a")
        env = {key: value for key, value in os.environ.items() if key not in SCRUBBED}
        child = subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), "run", "--repo", str(args.repo),
             "--tip", detail["tip"], "--tree", detail["tree"], "--artifacts", str(artifacts)],
            stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, env=env,
            start_new_session=True)
        state["running"] = {"tree": detail["tree"], "tip": detail["tip"], "pid": child.pid,
                            "started_epoch": now, "artifacts": str(artifacts),
                            "inputs": detail["inputs"], "skipped": detail.get("skipped", [])}
        state["pending"] = None
        state["last_started_epoch"] = now
        save(args.repo, state)
    print(f"journey-background tick: started {detail['tree'][:12]} at {detail['tip'][:12]} "
          f"(pid {child.pid}; skipped {len(detail.get('skipped', []))} older trees)")
    return 0


def command_run(args):
    """Evaluate one dist tree in a private detached worktree, then record it."""
    root = shared_root(args.repo)
    checkout = Path(args.artifacts) / "checkout"
    status, detail = "FAIL", "not started"
    try:
        if checkout.exists():
            subprocess.run(["git", "-C", str(root), "worktree", "remove", "--force", str(checkout)],
                           capture_output=True)
        git(root, "worktree", "add", "--detach", str(checkout), args.tip)
        subprocess.run(["npm", "ci", "--no-audit", "--no-fund"], cwd=checkout / "hub-web", check=True)
        result = subprocess.run(["bash", str(checkout / "scripts/journey-eval.sh"), args.artifacts,
                                 "--full", "--workers=4"], cwd=checkout)
        receipt = json.loads((Path(args.artifacts) / "journey-receipt.json").read_text())
        ok = (result.returncode == 0 and receipt.get("suite_exit") == 0 and not receipt.get("errors")
              and receipt.get("scope") == "full" and receipt.get("head_sha") == args.tip
              and dist_tree(root, args.tip) == args.tree)
        status = "PASS" if ok else "FAIL"
        detail = f"journey-eval exit {result.returncode}, suite_exit {receipt.get('suite_exit')}"
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        detail = f"background run failed: {error}"
    finally:
        subprocess.run(["git", "-C", str(root), "worktree", "remove", "--force", str(checkout)],
                       capture_output=True)
        with locked(args.repo):
            state = load(args.repo)
            evaluated = (state.get("running") or {}).get("inputs") or inputs(root, args.tip)
            save(args.repo, record_finish(state, args.tree, status, args.tip, args.artifacts,
                                          evaluated, detail, int(time.time())))
    print(f"journey-background run {args.tree[:12]}: {status} ({detail})")
    return 0 if status == "PASS" else 1


def command_reuse(args):
    """Copy a matching background evaluation into the cut's artifact dir."""
    if os.environ.get("JOURNEY_EVAL_FRESH") == "1":
        print("journey-background: JOURNEY_EVAL_FRESH=1; not reusing")
        return 1
    tree = dist_tree(args.repo, "HEAD")
    receipt = lookup(load(args.repo), tree, inputs(args.repo, "HEAD"))
    if not receipt or not Path(receipt["artifacts"]).is_dir():
        print(f"journey-background: no background evaluation of hub-web/dist {tree[:12]} to reuse")
        return 1
    source, target = Path(receipt["artifacts"]), Path(args.artifacts)
    target.mkdir(parents=True, exist_ok=True)
    for name in REUSED_FILES:
        item = source / name
        if item.is_dir():
            shutil.copytree(item, target / name, dirs_exist_ok=True)
        elif item.is_file():
            shutil.copy2(item, target / name)
    (target / "journey-reuse.json").write_text(json.dumps(
        {"tree": tree, "tip": receipt["tip"], "source": str(source),
         "finished_epoch": receipt["finished_epoch"]}, indent=2) + "\n")
    print(f"journey-eval: reused background evaluation of hub-web/dist {tree[:12]} at "
          f"{receipt['tip'][:12]} from {source}; set JOURNEY_EVAL_FRESH=1 to rerun")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="action", required=True)
    for name in ("offer", "tick", "run", "reuse"):
        command = sub.add_parser(name)
        command.add_argument("--repo", type=Path, required=True)
        if name in ("offer", "run"):
            command.add_argument("--tip", required=True)
        if name == "offer":
            command.add_argument("--green", action="store_true")
        if name == "tick":
            command.add_argument("--host-idle", action="store_true")
            command.add_argument("--min-interval-secs", type=int, default=int(
                os.environ.get("CAS_JOURNEY_BACKGROUND_MIN_INTERVAL_SECS", DEFAULT_MIN_INTERVAL)))
        if name == "run":
            command.add_argument("--tree", required=True)
        if name in ("run", "reuse"):
            command.add_argument("--artifacts", required=True)
    args = parser.parse_args()
    try:
        return {"offer": command_offer, "tick": command_tick, "run": command_run,
                "reuse": command_reuse}[args.action](args)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"journey-background {args.action}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
