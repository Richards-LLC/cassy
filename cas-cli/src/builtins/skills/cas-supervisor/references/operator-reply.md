# Phone reply contract

On a Claude/Codex supervisor, answer a verified Commander question once in the final pane reply; the mirror carries it to web and phone. Use [explicit operator routing](reference.md#verified-commander-messages) only for unprompted updates or unmirrored delivery.

The same Markdown reaches the pane and Commander, which draws diagrams, colored code, tables and callouts. The operator reads a picture faster than a paragraph, so show content visually:

- Answer first: the first line is the answer in one sentence.
- Give each kind of content its form from the table below; bullets (at most 5, one line each) only when no form fits.
- Bold the decision word or the thing waiting on the operator.
- Keep prose within the `cas-supervisor` pane budget unless asked for detail. Diagrams, code blocks and tables do not count; use at most one of each, and keep a diagram to about 10 nodes for a 390 px phone.
- No file paths or ticket IDs unless asked, and no process narration.

| Content | Form | Write | Terminal pane shows |
|---|---|---|---|
| A flow, dependency, state change or timeline | Diagram | a `mermaid` fence; `flowchart TD` stays narrow on a phone | the Mermaid source as a code block |
| Bad news, a blocker, a risk or a decision | Callout | `> [!WARNING]` for bad news or a blocker, `> [!CAUTION]` for a destructive or irreversible step, `> [!NOTE]` for a decision or context, each body line also starting with `>` | a quote block with the marker as text |
| A command, config or code | Code block | a fence with a language tag; `bash`, `ts`, `js`, `json`, `python` and `rust` are colored, `diff` opens a change view | a code block |
| A comparison of options, workers, gates or before/after | Table | a pipe table with a header and dash rule row; start and end every row with a pipe | the table |
| A screenshot or other picture of a delivery | Image | `![alt](artifact:<id>)` for a published artifact, or `![alt](<path>)` for a PNG, JPEG, WebP or GIF under `[factory] artifacts_root/<project-key>/` (absolute, or relative to that directory), at most 10 MiB; the operator taps it to enlarge | the Markdown as text |

Asked how a merge reaches main while CI is red:

````markdown
Merges reach main only after CI reruns the full suite on the merged tree.

```mermaid
flowchart TD
  A[Worker parks the task] --> B[Supervisor merges to the epic]
  B --> C[CI runs the full suite]
  C -->|green| D[main]
  C -->|red| B
```

```bash
# newest runs on main first
gh run list --branch "main" --limit 3
```

> [!WARNING]
> **Blocked:** CI is red on two browser checks, so nothing reaches main until they pass.

| Gate | Runs | Blocks main |
|---|---|---|
| Worker check | capped check and named tests | no |
| CI on the merged tree | full suite and browser checks | yes |
````

## Showing a UI delivery

Show the operator a visual delivery as images in the reply, not as paths. Use the task's QA screenshots (`<artifacts_root>/<project-key>/<task-id>/…`), one image per size or scheme the operator must judge, each with alt text naming what it shows. Pair before and after when the delivery changes an existing screen. Commander loads each image from this machine, with the operator's session permission. A path outside the project's artifacts directory, a non-image file or a remote URL stays text, so copy evidence into the artifacts directory first. wl-copy cannot carry images.
