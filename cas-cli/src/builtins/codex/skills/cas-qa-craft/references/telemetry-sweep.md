# Telemetry sweep contract

`qa.telemetry_sweep` is an optional project-relative executable. It is a
read-only observation hook, not a data export: it must query analytics without
mutating production data and must never print credentials, authorization
headers, raw event payloads, or user identifiers.

## Output

Write one tab-delimited line per finding, with no header and exactly these six
fields:

```text
kind<TAB>subject<TAB>count<TAB>people<TAB>window<TAB>sample
```

`kind` is `NEW`, `RISING`, `HIGH_RATE`, or `BLACKOUT`. `NEW` means a subject
appeared in the window; `RISING` means its count increased against the
script's comparison window; `HIGH_RATE` means events per person crossed the
project's documented rate; and `BLACKOUT` means the expected reporter/person
produced zero events. `count` and `people` are non-negative integers (a
blackout normally has `0` for both). `window` is a short, human-readable
interval and `sample` is one safe, redacted example; tabs and newlines are not
allowed inside fields.

The skill invokes the executable from the project root. A task reporter, when
present, is passed as `--reporter <value>`; the script should scope its query
to that reporter without echoing the value. A script failure or malformed line
is recorded in the ledger honesty section and does not become a fabricated
finding.

## Documentation-only PostHog/HogQL example

This is a reference shape, adapted from the project sweep contract rather than
a drop-in Cassy implementation. It illustrates the query boundary only; keep
the API host, project id, and token in the environment and never echo them.

```bash
#!/usr/bin/env bash
set -euo pipefail

reporter=""
if [[ "${1:-}" == "--reporter" ]]; then
  reporter="${2:?missing reporter value}"
fi

query='SELECT event, count() AS count, uniq(distinct_id) AS people
FROM events
WHERE timestamp >= now() - INTERVAL 7 DAY
GROUP BY event
ORDER BY count DESC'

# POSTHOG_API_KEY and POSTHOG_PROJECT_ID are read by curl but never printed.
# The production script should add a reporter predicate when `$reporter` is
# present, parse the JSON rows, and emit only safe six-field TSV findings:
#   RISING<TAB><subject><TAB><count><TAB><people><TAB><window><TAB><sample>
# A zero-row reporter query emits BLACKOUT with count=0 and people=0.
curl --fail-with-body --silent --show-error \
  -H "Authorization: Bearer ${POSTHOG_API_KEY:?}" \
  "https://app.posthog.com/api/projects/${POSTHOG_PROJECT_ID:?}/query" \
  --data-urlencode "query=${query}" \
  | jq -r '.results[] | ["RISING", .event, .count, .people, "last-7d", "redacted"] | @tsv'
```

## Known noise

Keep explained subjects in the QA ledger instead of re-filing them:

```markdown
| subject | explanation | task id | citation |
| --- | --- | --- | --- |
| checkout | expected migration traffic | cas-1234 | task note or issue URL |
```

The task id must identify the existing explanation. A finding with no matching
row remains a finding and should be evaluated by the normal defect-task rule.
