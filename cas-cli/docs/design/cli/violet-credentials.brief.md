# Violet CLI credential diagnostics

| Field | Contract |
| --- | --- |
| First two lines | The existing command failure names `missing_credential`, its variable, and the reason; `cas doctor` is the copyable diagnostic command. |
| Scannable | Diagnostics name the attempted environment variables and private-file path. Native registration failures also name the registration path. |
| Readable | Missing, ambiguous and policy-withheld credentials stop before any network call. Selection ambiguity explicitly asks for `VIOLET_SLACK_TOKEN_ENV`. |
| Machine output | `--json` retains one `ok: false` error document with `code`, `message` and `retryable: false`; no credential values appear in either stream. |
| Omitted | Tokens, header values and file contents never appear. Existing terminal layout and colour handling are retained. |

Only the user machine production proxy marked `# managed`, naming the default
`VIOLET_SLACK_TOKEN_CASSY_PROXY`, may use an explicit selector or recover a
missing default through the generic token, native bearer registration, or one
unambiguous labelled token. Missing selectors and native references fail closed.
Custom references, custom endpoints and project definitions retain their explicit
account. Worker denials govern inherited values and private-file fallback alike.

Nonempty environment values precede the safe private file; empty exports do not
shadow private values. This uses the same resolver as harness launch and update
probes. The CLI additionally asks that resolver for explicitly referenced custom
or legacy private-file variables; harness hydration still injects only registered
canonical credentials.

The canonical selector takes precedence over its installed legacy alias; a
missing selected credential cannot fall through to another account. Selectors
accept only Violet Slack token variable names (including their legacy spelling),
never bypass or unrelated secrets. Worker denials compare canonical credential
identities, covering both spellings in inherited values and private-file loads.
After explicit selection and the exact default, the generic token precedes the
native registration, matching the release adapter; labelled discovery is last.
