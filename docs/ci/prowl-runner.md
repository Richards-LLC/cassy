# Prowl macOS runner

Operator-approved November 4, 2026. The routing change uses existing admission
jobs and existing checks; see the [whole release flow](release-fast-publication.md#self-hosted-routing).

## Provisioning

The supervisor runs the reviewed [wizard](../../scripts/provision-prowl-actions-runner.sh)
on prowl with Daniel's sudo. Agents must not execute it as a test or register
a runner. It requires macOS ARM64 and an installed, licensed Xcode 26.3 at
`/Applications/Xcode_26.3.app/Contents/Developer`, matching hosted CI. Rust
stable is installed into the CI account's home; Zig still uses the workflow's
existing bootstrap and all compile/build checks remain unchanged.

The wizard creates the hidden, disabled-login, non-admin `cassy-ci` account and
its own group and home (`/var/lib/cassy-ci`). A deny ACL at position zero on
each existing human home prevents read and traversal, including operator
keychains and private files. Existing unrelated ACL entries are retained.
The script verifies the denials as the service user before registration and
again before starting the root-owned launchd service. A failed isolation check
stops provisioning. No operator home, Git credentials or keychain is copied.

Runner version 2.336.0 is pinned with its
[official ARM64 archive](https://github.com/actions/runner/releases/tag/v2.336.0)
SHA-256. The launch daemon `io.petrastella.cassy-ci` runs `runsvc.sh` as
`cassy-ci`, with its own HOME/Cargo/Rustup environment. It starts at boot
without a GUI login. Logs are under `/var/log/cassy-ci-runner`, whose root-owned directory
prevents CI from replacing launchd log paths with symlinks.

On prowl, after copying the reviewed checkout, the supervisor obtains a
short-lived repository registration token and runs:

```bash
export RUNNER_TOKEN="$(gh api --method POST repos/Richards-LLC/cassy/actions/runners/registration-token --jq .token)"
sudo --preserve-env=RUNNER_TOKEN scripts/provision-prowl-actions-runner.sh --install --yes
unset RUNNER_TOKEN
sudo scripts/provision-prowl-actions-runner.sh --verify
```

`--yes` is explicit approval for the printed plan and avoids a second typed
confirmation after sudo. Omit it to review one interactive confirmation.
Tokens are piped to the runner's supported input environment, never written
to a file or included in process arguments. An interrupted install retains
its root-owned receipt at `/var/db/cassy-ci-runner`; inspect and uninstall
that attempt before retrying rather than adopting an existing account.

## Runner inventory permission and trust

Set repository secret `CASSY_RUNNER_READ_TOKEN` to a fine-grained token scoped
to this repository with **Administration: read**, or an equivalently scoped
GitHub App token. GitHub's [runner inventory API](https://docs.github.com/en/rest/actions/self-hosted-runners#list-self-hosted-runners-for-a-repository)
requires this permission; the ordinary workflow token often cannot read it.
Without the secret/permission, routing safely uses hosted Macs. The CI selector job has no checkout; Release Prebuild shares its existing
version-detection checkout. Both selectors execute inline workflow code,
never a script loaded from the checkout, and check the canonical repository
and allowed event/ref in their server-side condition and script. No Mac
consumer step receives the inventory token. A timed-out selector also falls
back through default hosted outputs. A live idle runner named `prowl-cas-mac` must carry every required
label before it can be selected.

Persistent public-repository runners retain the repository's existing
external-contributor approval boundary; a fork can edit its workflow. Keep
external-contributor workflow approvals required and do not approve a PR that
requests these labels. See the existing [trust boundary](self-hosted-runner-pilot.md#trust-boundary).

## Live acceptance receipts

The worker has not registered or run this service. The supervisor records:

| Check | Required evidence |
| --- | --- |
| Account isolation | Successful wizard `--verify`, denied home/keychain reads, non-admin UID and launchd service UID |
| Preferred runner | A trusted merge-group macOS Check and ARM64 prebuild run on `prowl-cas-mac` with unchanged checks |
| Hosted fallback | Stop the dedicated service before selection, then record a trusted hosted Mac run |
| Uninstall | Successful unregister/service/account removal, with other home ACLs preserved |
| Timing | Compare macOS Check elapsed and queued seconds with the hosted baseline below; report end-to-end release time separately |

[Hosted baseline run 41845515686](https://github.com/Richards-LLC/cassy/actions/runs/41845515686/job/126628428314)
succeeded on a GitHub-hosted Mac in **thirteen minutes fifteen seconds**
(09:15:08–09:28:23 AM Eastern, November 4, 2026). Prowl timing and the live
online/offline run links are pending provisioning; no speedup is claimed.

For the fallback control, stop and later restart only this owned service:

```bash
sudo launchctl bootout system/io.petrastella.cassy-ci
sudo launchctl bootstrap system /Library/LaunchDaemons/io.petrastella.cassy-ci.plist
```

Run the hosted control between those commands. Availability is a snapshot;
a runner becoming busy/offline after selection can still queue the job.

## Uninstall

Use a fresh removal token; the same wizard supports partial unregistered
installations without a token. It stops only its launchd service, unregisters
the runner, refuses to lift home isolation while any CI-UID process remains,
removes its exact deny ACL entries, then deletes only its own account, group,
service files, home and receipt. Other users' existing ACLs remain intact.

```bash
export RUNNER_REMOVE_TOKEN="$(gh api --method POST repos/Richards-LLC/cassy/actions/runners/remove-token --jq .token)"
sudo --preserve-env=RUNNER_REMOVE_TOKEN scripts/provision-prowl-actions-runner.sh --uninstall --yes
unset RUNNER_REMOVE_TOKEN
```

Never remove the deny ACLs first or run CI under the operator's login.
