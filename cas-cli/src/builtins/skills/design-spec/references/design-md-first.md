# DESIGN.md first

This is the one rule for anything a person reads or sees: an HTML report,
page, chart, deck, release or customer notes, Commander or hub-web UI, CLI or
TUI output, drawing, or generated image. Every craft and report skill links
here; none restates it. It applies whether or not the brief mentions branding.

## 1. Name the target project

The target is the project the output is *for*, which is not always the
checkout you run in. A status report on a client app wears the client's
brand, even when a Cassy worker writes it. When the brief names no other
project, the target is the current project.

## 2. Resolve its DESIGN.md

Use the nearest `DESIGN.md` to the surface: the app root for UI in one package
(for example `hub-web/DESIGN.md`), otherwise the repo root. A delivery profile
such as `docs/sow/DELIVERY.md` may name the file instead, and so may the
`project_<slug>_designmd` memory pointer. Follow its `inherits:` key when it
names another `DESIGN.md`. Take the revision with
`git -C <repo> log -1 --format=%h -- <path>`.

## 3. Use all four parts

- **Tokens:** colours, spacing and radii by the role names in the frontmatter
  (`maps` gives the project variable). No hex or size the file lacks.
- **Type:** its display, body and mono families and its scale.
- **Mark:** its `brand-mark` asset or the logo the file names.
- **Voice:** the tone and vocabulary rules in its Overview and Do's & Don'ts.

Plain-text formats (Slack, a terminal) still take the voice and the semantic
roles. Drafting conventions and the CLI output contract keep their own
mechanics; the project decides type, mark and palette within them.

## 4. Missing or stale

When the file is missing, say so in your reply or task note
(`no DESIGN.md in <project>`). Then run `design-spec` to create one and build
from it. Generic styling is never the fallback. The Petrastella tokens are
design-spec's inheritance base, not a substitute for running it. When the
target cannot take a commit, write the file under the task's artifacts
directory and name that path. When the token source is newer than the file,
follow design-spec's Drift trigger first.

## 5. Record it

Name the DESIGN.md in the close reason or a decision note of the task that
produced the output, as one of these lines:

```text
design: <repo-relative path>@<revision>
design: none: created via design-spec at <path>
```

This is required for a task with a `qa.user_facing_labels` label, a demo
statement, or a deliverable a person reads. Reviewers and QA check the output
against the recorded file.
