## Core Concepts

### Codex supervisor delivery and continuity

Codex supervisors receive queued messages at turn start within the existing
10-row / 5,000-character budget. Overflow remains pending for the next turn;
new messages also arrive after Cassy MCP calls. Idle wake decisions read the
supervisor's own account and rollout, including recipient ownership for an
unbound native session, and preserve the in-flight tool veto.

At an idle boundary, context pressure saves lane facts and gives the supervisor
one turn to add judgement before restarting its pane. Workers and the factory
continue running; the replacement receives the saved handoff. The threshold is
the lower of 80% of the reported window and the configured input cap, which
defaults to 300,000 tokens; zero disables this automatic reset.
