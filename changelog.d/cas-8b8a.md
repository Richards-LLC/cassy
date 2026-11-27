### Fixed — Factory scorecard

- **Recorded nightly windows** — `cas scorecard --since <RFC3339> --until <RFC3339>` reports local rows in the half-open window without fetching Cloud or GitHub. It includes refusals per observed merge and by recorded actor (with unknown explicit), supervisor wake latency by harness, proven agent issue filings, unattributed filings and observed closure receipts. Missing close timestamps remain unknown.
- **Reliable turn and refusal sensors** — Native turns keep their identity across agent re-registration; the scanner records supervisor wake starts immediately. MCP refusal facts retain uncoded errors and authenticated actor, role, harness, tool and action.
