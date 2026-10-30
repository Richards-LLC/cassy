# Native catalog projection: controls, interfaces and remaining launch limits

Claude and Codex have promising launch-local recipes that preserve native named
invocation and reduce the complete isolated catalog below 2,500 characters.
Grok can produce the same result when its fixture account configuration contains
an extra discovery root, but its invocation-local config overlay rejects that
discovery setting. The process-local filesystem alternative cannot run in this
worker's namespace. No production launch or real account is changed; the feature
acceptance criteria remain unmet.
Task-specific selection and lifecycle integration are not implemented in
production; the prototype selects one skill by audience.

This updates the initial project-copy investigation. The earlier finding that
no Claude skill-only control had been found was bounded and incomplete:
`--settings skillOverrides` is a supported control and works in the native probe.
The initial 13-case evidence remains available as `art-2fc71fc5`.

## Native fixtures and complete catalog measurements

[The executable probe](../../scripts/probe-skill-catalog.py) reads the repository's
embedded public builtin inventory, including relative resources, into disposable
profiles. It runs the installed native CLI against a local scripted HTTP endpoint
that returns only a final message. Native expansion of a unique body marker
proves invocation; the stub neither reads files nor executes tools. Original and
project body markers distinguish which copy ran. The `/init` control checks the
bundled command's expanded user-message body separately.

The following measurements cover complete native skills sections, including
native bundled entries and section wrappers, without path normalization. They
exclude unrelated system instructions and the explicitly invoked skill body.
Claude includes its native bundled descriptions in the baseline; Codex includes
its four system skills in both baseline and treatment. Fixtures contain a Git
repository and the current shipped inventories: 50 Claude entries, 49 Codex and
49 Grok entries. Existing explicit-only policies remain part of the baseline.

| Native CLI | Complete baseline, characters | Worker treatment | Standalone reviewer treatment | Hidden named invocation |
| --- | ---: | ---: | ---: | --- |
| Claude Code 2.1.312 | 14,772 | 224 | 257 | Original public body loaded; hidden bundled `/init` also expands |
| Codex 0.165.0 | 12,078 | 2,230 | 2,265 | Project public body loaded |
| Grok 1.0.70, f50904eaee6c | 12,450 | 291 | 326 | Project public body loaded, using the disk-config comparator |

These are **standalone CLI audience fixtures**, not native task-verifier children
and not production account launches. Their JSON records `native_child=false`.
Character and UTF-8 byte measurements are separate. Real account paths, installed
plugins, additional skills and native versions can change the budget. No passing
fixture is permission to enable a production default.

### What each treatment actually does

- **Claude:** invocation-local `--settings` sets unselected public and native
  bundled names to `user-invocable-only`. Account/profile selectors are retained;
  no skill copies or private config reads are needed for this comparator.
- **Codex:** all public skills/resources are copied into the fixture project's
  discovery root. Unselected copies use `allow_implicit_invocation=false`;
  per-invocation `skills.config` disables the original paths. The copies remain
  explicitly invocable. The selected account home is retained.
- **Grok disk comparator:** the fixture account's generated `config.toml`
  contains `[skills] paths=[projected_public_root]`. The extra root wins over
  the original public copies and contains explicit-only metadata. This is not a
  deployable recipe under the prohibition on shared/private config changes.

Original **skill/resource** files remain unchanged. That assertion does not
cover every account-state file: Grok reformats its generated public config in
these native runs. The probe separately checks that the generated fixture
settings survive semantically; all observed audience comparator runs preserve
those settings. It never discovers, reads or copies a real private profile.

## Negative controls and concrete limitations

1. **Disabled is not explicit-only.** Codex `skills.config enabled=false`,
   without replacement copies, suppresses the catalog but prevents native named
   expansion. Grok `[skills] disabled` yields a 288-character worker catalog,
   but `/cas-technical-drawing` does not expand. Neither satisfies invocation
   preservation.
2. **Project roots alone are insufficient.** The initial synthetic project-copy
   controls still advertised personal skills in Claude and Grok. A further Grok
   control with a real Git fixture and the complete inventory still advertises
   12,450 characters and invokes the original body. Merely changing metadata in
   a lower-priority project copy is not a byte-saving implementation.
3. **Grok's invocation-local overlay cannot add a discovery root.** Its installed
   embedded configuration guide describes `GROK_CONFIG`/`GROK_CONFIG_PATH` as a
   merged, allowlisted soft-settings layer. Discovery sources are excluded. The
   native `env-overlay` control supplies only `[skills] paths` in an additional
   file while retaining the original fixture profile. The complete catalog
   remains 12,450 characters and the original body runs. The successful
   disk-config comparator must not be relabeled a successful per-launch overlay.
4. **A process-local mount view is unavailable in this worker.** The experiment
   keeps original selectors and mounts only generated public skills over the
   fixture skill path. `bwrap` fails before any native request: no permission to
   create a namespace. The result is `status=unsupported`, script exit 3, native
   exit 1, zero model requests. No bypass or privilege change was attempted.
5. **Credential-file symlinks do not establish refresh preservation.** A fixture
   with empty route canaries performs atomic replacement of an overlay symlink;
   the link becomes an independent file while the original inode stays intact.
   This demonstrates the routing hazard without reading or copying credential
   contents. It is not evidence that a particular native OAuth backend uses
   that write sequence, nor proof of native refresh correctness.

## Three independent interface designs

Three read-only reviewers explored distinct interfaces before a consequential
production seam was chosen. Their recommendations and exact caller findings are
recorded in the task decision notes; none received edit or lifecycle ownership.

### A. Prepared whole-profile launch

`FixtureProfiles.prepare(request, continuation) -> PreparedFixtureLaunch` owns
profile materialization, effective-profile trust, launch construction and
persistent-state reuse. The request carries the original selected identity,
independent Claude secure-storage state and a fresh/resume receipt. Errors must
reject credential transport that has not been proved safe.

This has depth: deleting it moves identity, trust ordering, source precedence,
state lifetime and cleanup into every caller. It is rejected as the universal
implementation because file copies violate the credential constraint and file
links do not prove atomic refresh semantics. A new profile root also changes
native trust, sessions, plugins and caches; a synthetic API-key run cannot prove
those account invariants.

### B. Capability-driven catalog view

`CatalogViews.prepare(public_inventory, audience, task, generation)` followed by
`apply(prepared, worker_launch)` hides native metadata, immutable public resource
materialization and capability-specific launch operations. It accepts no private
profile contents. A receipt pins the catalog generation through recycle/resume.
The original account and secure-storage selectors stay authoritative.

This is the smallest coherent recommendation for further investigation: use
native controls where demonstrated, and return an explicit unsupported result
where an adapter cannot preserve the requested semantics. Claude's settings
control makes a whole-profile overlay unnecessary there. Codex can retain its
existing account home. Grok still requires a supported discovery seam or a
separately proved process view. Deleting the module would duplicate precedence,
metadata, resources, generation lifetime and budget checks across the backends.

The caller must know the audience/task selection, original identity, authorized
public inventory, generation lifetime and capability failure. Private adapters
own native command/config details. Supervisor launch types must be excluded.
`UnsupportedDiscovery`, `UnsupportedView`, `UnsafeResource`,
`NativeChildUnsupported` and `GenerationMismatch` must fail explicitly rather
than silently launch the full catalog and claim success.

### C. Common prepared-worker operation

`WorkerProfiles.prepare(WorkerLaunchConfig, Fresh|Resume) -> PreparedWorker`
makes the ordinary initial/dynamic/recycled caller simple and carries effective
session locations back to the pane. It correctly owns trust/config ordering.
It is rejected as the sole cross-role seam: native reviewers launched by a
supervisor are not worker panes. A worker-only operation cannot establish their
profile or catalog inheritance.

A raw `launch_delta` is also rejected. Today
`crates/cas-mux/src/pane/mod.rs:478` prepares trust before building the worker
configuration, and `:494` applies continuation before spawning. A late profile
environment patch would redirect discovery after trust preparation and discard
important state ownership. Catalog-only native overrides avoid that split;
whole-profile changes would require the deeper prepared operation.

## Invariants and proof boundaries

| Invariant | Current evidence | Remaining work before activation |
| --- | --- | --- |
| No credential contents or real profile access | Fixtures read public repo inventory and generated files only | Retain this boundary in any adapter |
| Native named invocation | Body expansion observed on all three audience comparators; Claude bundled `/init` control | Plugin/alias collisions and real native reviewer context |
| Complete native catalog size | Exact isolated native sections measured above | Production inputs and actual child launches |
| Selected account and secure-storage identity | Candidate recipes retain original root selectors; no production code changed | Preserve Claude unset/empty/set semantics; native auth/refresh compatibility remains unverified |
| Trust and hooks | Existing source ordering traced; fixtures do not install real CAS hooks | Use real preparation/canary path in an isolated integrated launch |
| Writable history, recycle and resume | No account-root replacement in native-control recipes | Native lifecycle and pinned-generation persistence not exercised here |
| Native reviewer inheritance | Current worker and role-file seams traced | Separate child evidence; standalone reviewer rows are not this proof |

Claude's independent storage state is implemented at
`crates/cas-pty/src/pty.rs:669` and applied at
`crates/cas-mux/src/backend/claude.rs:47`; trust targets the selected profile at
`:85`. A profile overlay must not turn the generated directory into the
credential selector. Codex also forwards its selected account directory to MCP
in `crates/cas-mux/src/backend/codex.rs:66`.

Native verifier definitions do not currently pass through the pane constructor:
`cas-cli/src/builtins.rs:65` generates the Claude/Grok role, and
`cas-cli/src/builtins/codex/agents/task-verifier.toml:3` defines the Codex role.
Their current definitions expose no independent profile selector. This is an
integration/proof gap, not a claim that every future native role mechanism is
impossible.

Grok's event lookup uses the daemon's `GROK_HOME` at
`crates/cas-mux/src/pane/mod.rs:1932`; a child-only whole-profile redirect would
also need effective session-root transport. Existing CAS continuation at
`crates/cas-pty/src/pty.rs:1355` handles Claude/Codex and rejects other harnesses.
A new catalog mechanism must preserve that existing behavior rather than claim
new Grok resume support from an environment change.

## Sources and reproduction

- [Codex skills](https://developers.openai.com/codex/skills): explicit-only policy
  and per-path enable/disable controls.
- [Claude visibility settings](https://code.claude.com/docs/en/skills#override-skill-visibility-from-settings):
  launch settings, visibility states and plugin exclusions.
- [Claude local code-review control](https://code.claude.com/docs/en/code-review-local#let-claude-start-the-review):
  bundled-command explicit invocation with visibility overrides.
- [Claude skill precedence](https://code.claude.com/docs/en/skills#resolve-skills-that-share-a-name)
  and [profile directory](https://code.claude.com/docs/en/claude-directory).
- [Grok skills](https://docs.x.ai/build/features/skills-plugins-marketplaces)
  and [settings](https://docs.x.ai/build/settings/reference): extra paths,
  disabled names and whole-profile scope. The installed 1.0.70 binary's embedded
  configuration guide documents the overlay allowlist; the native negative
  capture establishes its observed effect independently.

The probe's default synthetic matrix reproduces the original 13 cases with the
separate Codex disabled control. For complete public inventories, use
`--inventory builtin --mode baseline`, then Claude `--mode settings`, Codex
`--mode local-root`, or Grok's disk comparator `--mode extra-root`. Add
`--audience reviewer` for a standalone reviewer selection. Grok's negative
per-launch control is `--mode env-overlay`; its disabled-name control is
`--mode settings`. The process-view capability check is `--mode view` and exits
3 when unsupported. Pass the installed executable via `--executable` and a JSON
path via `--output`; JSON links to the complete native section and wire capture.
All listeners and disposable profiles are removed when a probe returns.
