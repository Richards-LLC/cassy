# Native capacity observations and profile setup

- First two lines: capacity is an observation, with its Eastern time beside the percentage; a previous-day observation also names its date.
- Scannable: preserve the existing account and reset rows, adding only `as of`; keep the same-day label short.
- Readable: a profile setup warning names its settings file, the parse/read/write cause, and `cas hook configure` as the retry after repair.
- Machine output: native timestamps remain RFC 3339 and usage JSON fields stay unchanged; warnings go to stderr while hook setup continues for other profiles and the project.
- Omitted: no usage endpoint request, credential-content read, automatic refresh or account switch; the batch candidate owns the real two-profile walkthrough.

Was: the hub omitted observation age, previous-day readings looked like today's clock time, and one bad profile stopped project hook setup.
Now: shared observation labels carry time and, when needed, an Eastern date; profile migration failures warn while healthy profiles and project hooks continue.

## Verification

Named Rust regressions cover the launch row, an Eastern day boundary, and malformed-profile continuation. The batch supervisor owns the real-build terminal capture and HUB-J13 native-capacity walkthrough; they are not claimed by fixture evidence.
