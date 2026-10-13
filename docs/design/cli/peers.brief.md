# Peer awareness

| Field | Contract |
| --- | --- |
| First two lines | The count of visible supervisors and live supervisors comes first, followed by the repository scope. |
| Scannable | Each supervisor groups machine, session, epic focus, claims, and live workers with branch and task rows. |
| Readable | Claim titles and locally synced task status explain the held work; claim age is time since the last renewal. |
| Machine output | `coordination action=peers json=true` and `cas peers --json` return one object with `canonical_id`, `peers`, and `detail`; each detail peer contains `supervisor`, `user_id`, `team_id`, `project_id`, `claims_visibility`, `branch`, `claims`, and `workers`. Missing synced titles, statuses, or branches are null; claims visibility is `same_account`, `different_account` or `unknown`. |
| Omitted | Human output truncates long values to 80 cells; JSON preserves full values. Workers are read-only observations and never become message or spawn targets. |
