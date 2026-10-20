# SOW Delivery Rules

> Canonical Cassy template for `cas-sow-delivery-report`. Copy it to
> `docs/sow/DELIVERY.md` and fill every double-angle-bracket placeholder with
> the operator. These values are business terms agreed with the client, so
> code cannot supply them. A project may tighten the rules in its own copy. Each SOW may
> override a value in its own `docs/sow/<sow-slug>/SOW.md` front matter.

## People and channels

| Key | Value |
| --- | --- |
| `pm_name` | `<<PM display name>>` |
| `pm_mention` | `<<PM's Slack user ID, written in Slack mention syntax>>` |
| `client_name` | `<<client display name, exactly as the client writes it>>` |
| `client_channel` | `<<#client-channel>>` (`<<CHANNEL_ID>>`): gap questions and the delivery message |
| `internal_channel` | `<<#internal-channel>>` (`<<CHANNEL_ID>>`): developer → PM handoff and PM pings |

## Review cycle and rounds

| Key | Value | Meaning |
| --- | --- | --- |
| `review_cycle_days` | `<<e.g. 14>>` | Longest time from delivery to client sign-off for one round |
| `rounds_included.medium_large` | `<<e.g. 3>>` | Feedback rounds included in a medium or large SOW within the cycle |
| `rounds_included.small` | `<<e.g. 0>>` | Rounds included in a small or one-off SOW. At 0, every new request goes to the PM for billing |
| `pm_ping_after_round` | `<<e.g. 2>>` | After this many rounds on the same feature, ping the PM before starting the next one |
| `approval_hours` | `<<e.g. 2>>` | A change request estimated above this many hours, QA included, needs PM approval before work |

Each SOW records its size (`small` or `medium_large`) in its front matter as
`size:`. Without it, ask the PM; do not assume.

## Environments

| Key | Value |
| --- | --- |
| `staging_url` | `<<https://staging.example.com, or none>>` |
| `production_url` | `<<https://app.example.com>>` |
| `version_endpoint` | `<<path that reports the deployed version, or none>>` |

Use staging for screenshots when it exists, otherwise production.

## QA login (headless capture)

| Key | Value |
| --- | --- |
| `qa_account_label` | `<<which test account, e.g. "QA admin (test account)">>` |
| `qa_credentials_file` | `<<absolute path to a chmod-600 file with email: and password: lines>>` |
| `login_path` | `<<route of the sign-in form, e.g. /login>>` |
| `login_success_url` | `<<anchored regular expression the URL matches only once signed in, e.g. ^https://[^/]+/admin/>>` |
| `login_selectors` | `<<email, password, submit selectors, or default>>` |

Name the file here, never the secret. If the account needs a second factor or
a third-party consent screen, it cannot serve headless capture; ask the
operator for a test account that can.

## Brand

| Key | Value |
| --- | --- |
| `design_md` | `<<path to the client product's DESIGN.md>>` |
| `logo` | `<<path to the SVG logo the report header uses>>` |
| `fonts` | `<<paths to font files to embed, or system>>` |
| `report_author` | `<<name stamped as the PDF author, e.g. the agency>>` |

## Report locations

- SOW documents: `docs/sow/<sow-slug>/SOW.md`
- Gap check and rounds: `docs/sow/<sow-slug>/GAP-CHECK.md`, `ROUNDS.md`
- Reports: `docs/sow/<sow-slug>/reports/`
