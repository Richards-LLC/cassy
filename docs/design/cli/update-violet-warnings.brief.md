# cas update Violet advisories

| Field | Contract |
| --- | --- |
| First two lines | A Violet warning names the skipped profile or advisory and its cause; builtin sync continues. |
| Scannable | Each warning wraps at whitespace with its retry or verification command on a separate line; path tokens never split. |
| Readable | HOME paths use `~`; over-wide path tokens keep their first segment and leaf around a middle ellipsis, with a one-line `--verbose` hint. |
| Machine output | The existing update JSON document adds `warnings` containing complete absolute warning paths; compact diagnostics stay on stderr, and `--verbose` prints full human paths. |
| Omitted | Credential values, parser error chains and repeated full HOME prefixes stay off compact output; stored advisory paths and trust boundaries remain unchanged. |

A shared-base symlink beneath the active HOME is already trusted. An explicit
profile outside HOME does not authorize a sibling target outside both roots;
the warning correctly describes that refused target. This includes inherited
profile selectors during an isolated-HOME update. The renderer leaves this
boundary intact and names a retry after the operator reviews the skipped path.
