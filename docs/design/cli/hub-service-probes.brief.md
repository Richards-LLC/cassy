# Hub service diagnostic probes

## Reader

The operator checking hub health when systemd or launchd cannot answer.

## Decision

Distinguish a confirmed inactive installed service from an unobserved manager before relying on recovery or changing process ownership.

## Concept

Every read-only manager child has a 500ms deadline. Unknown remains distinct from the confirmed active/inactive result; only the created child is killed and reaped off the diagnostic deadline.

## Hierarchy

Keep the hub verdict first and add the short warning “service manager unknown (timed out)” with one retry command. Existing installed-service bypass errors still refuse an unsupervised hub.

## Machine output

`cas hub status --json` emits service_status “unknown” plus service_warning when its manager probe times out or cannot run. `cas hub service status --json` retains nullable active/enabled and adds optional manager_warning. No timeout becomes false or healthy. Restart refuses unknown activity before changing unit, profile or process ownership. The 500ms bound is per diagnostic child; up to three manager diagnostics plus the existing receipt/health probes can contribute to total command time.

## Critique

New warning and JSON output require the supervisor's actual build and terminal QA. Source-only worker proof does not claim a green user-path receipt.
