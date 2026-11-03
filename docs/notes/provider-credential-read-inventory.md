# Provider credential read inventory

Source review: 2026-10-28. No operator credential file or Keychain secret was
opened during this investigation. This inventory distinguishes file presence,
non-secret metadata, and actual token reads; it does not assert provider approval
for undocumented integrations.

## Claude

| Source surface | Read/use | Verdict after this change |
| --- | --- | --- |
| `cli/limits.rs` collection | Previously deserialized `.credentials.json` or Keychain `security ... -w`, then sent a Bearer request to the subscription usage endpoint | Removed, including token structs, endpoint, auth headers and request adapter. Reads only a typed `.cas-claude-usage.json` metadata snapshot. |
| `account_registry.rs::metadata` | Previously loaded the Claude credential JSON for `subscriptionType` | Removed. Plan is unknown. Identity comes from `.claude.json` account/organization metadata. |
| `accounts.rs::claude_email`, `Provider::login_email`, `active_claude_email` | Reads `oauthAccount.emailAddress` from `.claude.json`; the default profile prefers `~/.claude.json` | Non-credential account metadata. No OAuth access/refresh token field is selected. |
| `account_registry.rs::account`, `set_login`; `accounts.rs::discover_aliases`; `account_links.rs::inspect`, `ensure` | Credential filename existence and account metadata for classification | Presence/metadata only. Native login probe determines actual acceptance. |
| `claude_credentials.rs::keychain_service` | Computes the native service selector from a normalized profile path | No credential read. Used for metadata probes and deletion only. |
| `capability.rs::macos_keychain_unavailable` | Runs `security find-generic-password -s <service>` to classify unavailable Keychain | Metadata only; no `-w` or `-g`. Official `claude auth status --json` handles authentication. |
| `mcp/tools/service/factory_ops.rs::preflight_claude_config_dir_with` | Linux opens credential file for readability; macOS probes Keychain item without requesting its password | Presence/readability only, no content read. Shared selector/preflight normalization belongs to the separate platform lane. |
| `account_lifecycle.rs` rename/new login | Previously read Keychain password and rewrote it under a new profile selector | Secret migration removed. Existing macOS Claude rename refuses before mutation; new login authenticates at its final typed path and retains that path. Cancellation removes the new folder. |
| `account_lifecycle.rs::keychain_delete`, remove/sweep/failed-login cleanup | Deletes the selected Keychain item after folder deletion or cancelled new login | Mutation by service name only; no password read. |
| `accounts.rs::plan`, `apply`; `account_links.rs`; `cli/doctor/accounts.rs` | Link/drift/doctor traverse only allowlisted shared configuration items; whole account retirement uses filesystem rename | No credential content read, digest or copy in the intended shared-item paths. `.credentials.json` is not a shared item; `.claude.json` copied for default-profile migration is non-credential metadata. |
| `cli/account_picker.rs::seed_profile_dir`; `cli/claude.rs` profile setup | Seeds allowlisted configuration and excludes credential/runtime-private entries | No Claude token read/copy. Authentication remains the official binary's job. |
| Claude launch builders in `cli/claude.rs`, `hub/launch_env.rs`, `cas-pty` | Remove provider credential override environment variables | Removal by variable name, no token extraction by these paths. |

`cas status-line --with-session` accepts the official binary's documented
`rate_limits.five_hour` / `seven_day` percentages and reset timestamps. It
persists selected fields only, not raw stdin, with an observation timestamp and
non-secret account identity. Snapshot writes are private and atomic. The profile
path, account identity and five-minute freshness window bound reuse. Expired
windows discard old percentages. Missing, invalid, stale or account-changed data
shows usage unknown with a reason. Neither a snapshot nor a transcript establishes
current login acceptance.

## Codex

| Source surface | Read/use | Verdict |
| --- | --- | --- |
| `accounts.rs::codex_email` | Loads `auth.json`, extracts `tokens.id_token`, decodes its JWT payload for email | Actual token value read locally. No forwarding here. **Not presence-only**; undocumented storage coupling remains. |
| `account_registry.rs::metadata`, `codex_claims` | Loads `auth.json`, reads account/workspace identifiers and decodes ID-token claims for fallback account ID and plan | Actual token value read locally. No forwarding here. **Not presence-only**; undocumented storage coupling remains. |
| `factory_auth_health.rs::codex_account_model_support` | Loads `auth.json` into JSON to inspect `auth_mode`, then reads `models_cache.json` | Reads secret-bearing file contents even though only mode is selected. No token extraction or forwarding in this function. |
| `accounts.rs`, `account_registry.rs`, `account_links.rs` | Filename existence for stored-login evidence | Presence only, apart from the explicitly identified email/identity readers above. |
| `mcp/tools/service/factory_ops.rs` Codex preflight | File presence/readability | No content read at this boundary. |
| `cli/codex.rs`, shared profile seeding/linking/doctor | Excludes `auth.json` from copied/shared configuration; native login/status commands own authentication | No credential copy/read by the seeding/linking boundary. Metadata discovery can still reach the readers above. |
| `cli/limits.rs` | Reads native rollout `rate_limits` observations | Native output, no credential read or subscription API call. |

OpenAI documents managed ChatGPT authentication through Codex app-server,
`account/read` for non-secret account/plan details and `account/rateLimits/read`
for native rate-limit output. Initialize with Cassy's own `clientInfo`; the
integration must identify itself instead of impersonating an official client.
The experimental external-token mode targets host apps that already own their
user authentication lifecycle. Its existence does not establish approval to
extract another application's stored subscription tokens.

Support qualification matters: the app-server page's **Protocol** subsection says
“WebSocket transport is experimental and unsupported.” The CLI reference's
**`codex app-server`** subsection says “This is primarily for development and
debugging and may change without notice.” These qualifications must be assessed
for the chosen transport and deployment; documented methods and client attribution
do not establish unconditional production approval. This review proposes a
credential-free metadata seam, not a production deployment decision. The separate
provider-policy audit owns that verdict.

The three direct Codex storage readers above remain explicitly recorded for a
separate migration to managed app-server account output. This Claude remediation
does not claim that those readers are presence-only or that OpenAI endorses them.
No policy prohibition specific to local ID-token claim decoding was established
by this source review. Provider-approval and macOS live Keychain/login behavior
remain independent review obligations, not outcomes inferred from unit tests.

## Official sources

- [Claude Code statusline](https://code.claude.com/docs/en/statusline): native `rate_limits` fields, used percentage and Unix reset timestamps.
- [Claude CLI reference](https://code.claude.com/docs/en/cli-reference): supported `auth status` JSON; no documented headless subscription-usage query was found in this review. Interactive `/usage` is not used as an automation protocol.
- [Claude legal and compliance](https://code.claude.com/docs/en/legal-and-compliance): native subscription authentication constraints motivating removal of credential intermediation.
- [Codex CLI reference](https://developers.openai.com/codex/cli/reference): the app-server command's development/debugging qualification.
- [Codex authentication](https://developers.openai.com/codex/auth): native ChatGPT and API-key sign-in modes.
- [Codex app-server](https://developers.openai.com/codex/app-server): managed account/auth/rate-limit methods and required client attribution.
- [Sign in with ChatGPT app-server integration](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server): separately authorized host authentication and client identification.

This inventory covers provider authentication storage. Cassy's own Cloud and hub
`auth.json` stores are separate first-party credentials, outside this inventory.
