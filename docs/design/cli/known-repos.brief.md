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
is retained under the task artifacts directory; the checked-in capture uses only synthetic paths.


## Critique

The human renderer is preserved; this change adds the requested machine form.
The isolated candidate passed both terminal gates (12 runs each, zero failures,
warnings or allowlisted findings) and the physical CLI proof. Raw receipts and
captures are in `cas-7e98/terminal-list` and `cas-7e98/terminal-status` under the
configured task artifacts root; `known-repos.after.txt` normalizes fixture paths.

```text
terminal-qa: PASS known-repos-list · 12 runs · 0 fail · 0 warn · 0 allowed
terminal-qa: PASS known-repos-status · 12 runs · 0 fail · 0 warn · 0 allowed
```

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | Human line one gives the repository or binding count; the first row follows immediately. |
| Fit | 4 | Inventory rows remain scannable at 80 and 120 columns; automation receives one structured document. |
| Craft | 4 | Existing health marks, touch-count alignment and selector-to-root rows are preserved. |
| Theme safety | 5 | Both gates pass four palettes, NO_COLOR, pipes and C locale without findings. |
| Machine contract | 5 | Real CLI proof covers both flag positions, empty/populated registries, valid/stale bindings and unchanged failure/row-retention behavior. |

Scored by tender-phoenix-59 on 2026-10-17 using synthetic fixtures. Actual host
registry contents were neither read nor included in captures.
