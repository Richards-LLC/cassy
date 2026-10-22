# cas update Violet advisories

| Field | Contract |
| --- | --- |
| First two lines | A Violet warning names the skipped profile or advisory and its cause; builtin sync continues. |
| Scannable | Each warning is one losslessly wrapped block with its retry or verification command on a separate line. |
| Readable | HOME paths use `~`; targets outside HOME retain every character, including across a long-token wrap. |
| Machine output | Existing update JSON stays on stdout; Violet diagnostics and their remedies stay on stderr without a new JSON schema. |
| Omitted | Credential values, parser error chains and repeated full HOME prefixes remain absent; stored advisory paths and trust boundaries are unchanged. |

A shared-base symlink beneath the active HOME is already trusted. An explicit
profile outside HOME does not authorize a sibling target outside both roots;
the warning correctly describes that refused target. This includes inherited
profile selectors during an isolated-HOME update. The renderer leaves this
boundary intact and names a retry after the operator reviews the skipped path.
