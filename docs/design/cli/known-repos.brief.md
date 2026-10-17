# Known repositories: machine output

| Field | Command-specific contract |
| --- | --- |
| First two lines | Default human output retains the existing repository or binding count and first row; JSON emits one document immediately. |
| Scannable | Repository rows retain path, health and touch count; binding rows retain selector, root and validated state. |
| Readable | Existing empty-registry guidance and binding-status wording stay unchanged for people. |
| Machine output | `list --json` returns `{repos: [{path, healthy, touch_count}], binding_count}`; `status --json` returns `{bindings: [{selector, repo_root, state}]}` with `valid` or `stale` state, empty arrays when empty, and no human prefix or progress on stdout. The global flag works before or after the command. |
| Omitted | No discovery, repair, registry mutation, credential data, or additional host identity is added; errors keep the existing CLI failure channel. |

The JSON fields describe the same collected facts used by the human renderer.
Paths belong to the invoking operator's local output. QA uses a synthetic HOME,
repositories and bindings, and publishes no private host registry contents.

## Verification

Extend the existing real-command binding/restart regression to cover empty and
populated JSON documents at both accepted flag positions. Preserve its human
rendering, binding conflict and unbind assertions. Candidate CLI/terminal evidence
will be attached before delivery; no real-build result is claimed yet.
