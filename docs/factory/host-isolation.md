# Hub and factory host isolation

Linux user services keep interactive Hub work separate from factory load.
These settings are relative scheduling and memory protection policies, not a
claim that latency improved on every host.

| Unit | Policy |
| --- | --- |
| `cas-hub.service` | `CPUWeight=1000`, `MemoryLow=1G` |
| `agent.slice` | `CPUWeight=40` |
| `agent-builds.slice` (under `agent.slice`) | `CPUWeight=25`, `MemoryHigh=24G` |
| New `cas-factory-*.service` | `Slice=agent.slice`, `Nice=5` |

`cas hub service install` generates the Hub policy. Normal service refresh
adds missing reservations to older base units while preserving explicit
operator values. A drop-in can override the base unit: inspect the effective
`CPUWeight`, `MemoryLow`, `Slice`, `Nice`, `ControlGroup` and `ExecStart`, not
just the generated file. In particular, an existing `MemoryLow=256M` drop-in
still wins over the new base `MemoryLow=1G`.

The tracked [agent slice](../../ops/systemd/agent.slice) and
[build slice](../../ops/systemd/agent-builds.slice) are baseline templates.
They are not installed automatically. Hosts with a separate configuration
owner must merge these weights into that owner's definitions, preserving
existing memory policy and local overrides. Factory processes launched by
the Hub use the agent slice and inherit positive niceness; the no-systemd
fallback is unchanged. Existing factory services are not moved or restarted
by this source change.

## Applying this on a host

The operator applies this, and the script refuses to run under a factory worker or
other agent role. An operator command in a supervisor pane is allowed. `install`
needs a checkout at this version, because the drop-ins live there; the copy it leaves
in `~/.local/libexec/cassy/` runs the other commands.

```bash
scripts/hub-pin.sh install --dry-run   # print the plan; change nothing
scripts/hub-pin.sh install             # apply; safe to repeat
```

`install` refuses a debug `cas` or a drop-in that overrides `ExecStart`. Otherwise it:

- links `~/.local/lib/cassy/hub/current` to the installed released `cas`
- points the hub unit's `ExecStart` there, keeping the previous unit under
  `~/.local/state/cassy/hub-pin-backup/`
- adds `90-cassy-host-isolation.conf` drop-ins for the Hub and both slices
- reloads systemd, then checks the effective values

The running Hub and running factories keep running. `ExecStart` applies at the
Hub's next restart; add `--restart` to do that now. To undo, restore the kept unit
and delete the drop-ins. `cas hub service install` writes the installed path back
into the unit, so run `install` again afterwards.

## Measuring the effect

The supervisor records matched before and
after intervals under comparable factory load. Capture the Hub's effective
unit, PID, cgroup `cpu.pressure`, `/proc/<pid>/schedstat`, host load and binary
identity. Compare changes in run-queue wait over each interval, not absolute
lifetime counters across different PIDs. A quiet baseline cannot establish
starvation or improvement; changing a debug Hub to a release build is also a
separate variable. Workers do not restart the Hub or other factories for this
proof.

## Release-built Hub hotfix

A hotfix can only come from a successful Release Prebuild run of the exact commit,
with its packaged audit step passed. No build is started, and there is no option
that takes a local file, so a `cas qa candidate` or other debug binary cannot be
pinned. A debug binary that reaches a slot some other way is refused at activation.

```bash
scripts/hub-pin.sh inspect                                  # shows the current target
scripts/hub-pin.sh stage --run-id <run> --sha <full-sha>    # inert
scripts/hub-pin.sh activate --sha <full-sha> --expected-current <target> --dry-run
scripts/hub-pin.sh activate --sha <full-sha> --expected-current <target>
scripts/hub-pin.sh rollback --expected-current <target>
```

`--expected-current` is what `inspect` printed, so a decision made against an older
state is refused. Activation restarts the Hub once and checks that its process runs
the pinned bytes. If it does not, the previous target and receipt come back, the Hub
restarts once more and the command stops. Rollback returns to the released binary
the Hub last ran, or to the immutable copy kept at activation if that binary has
since changed. An artifact that has expired (14 days) cannot be staged; use the
normal release path.

A newer release supersedes a pin. `cas update`, which install-on-publish runs when a
newer version is published, repoints `current` to the new binary and clears the
receipt before it restarts the Hub. With no newer release the pin stays. This reuses
the existing artifacts and updater and adds no build, job, timer or release budget.
Linux systemd only.
