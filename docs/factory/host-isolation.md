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

The supervisor coordinates live application and records matched before and
after intervals under comparable factory load. Capture the Hub's effective
unit, PID, cgroup `cpu.pressure`, `/proc/<pid>/schedstat`, host load and binary
identity. Compare changes in run-queue wait over each interval, not absolute
lifetime counters across different PIDs. A quiet baseline cannot establish
starvation or improvement; changing a debug Hub to a release build is also a
separate variable. Workers do not restart the Hub or other factories for this
proof.

## Release-built Hub hotfix design

The proposed managed executable path is `~/.local/lib/cassy/hub/current`,
with immutable release-artifact slots, a recorded previous release for
rollback, and normal install-on-publish advancement that clears an obsolete
hotfix pin. This path is a design proposal, not an installed feature.

Rule-176 requires whole-flow review and operator approval before implementing
its release/install interactions. The design must reuse the existing exact
source release-prebuild artifact and packaged audit evidence, preserve
binary-drift refusal and effective drop-in checks, and account for same-version
hotfix identity, failed readiness, rollback and the updater's post-swap path.
It adds no independent build, workflow, timer, duplicate test stage or release
budget. A `cas qa candidate` debug binary is never a production hotfix source.
