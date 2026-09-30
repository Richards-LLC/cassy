# Review bodies

Delivery PRs, protected-branch handoffs, release PRs and typed worker merge
requests share [pr-body.md](pr-body.md). Summary uses the smallest useful
visual: pseudocode, call tree, file tree, Mermaid or a diff sketch. Evidence
pairs the observed before and after, with a QA bundle or a verify-before-claim
base-vs-change run. Merge Danger displays the declared task risk and door.

Generate a delivery body from the repository root:

```bash
cas worktree pr-body --task cas-example --base origin/main --head HEAD \
  --before 'same command: failing baseline' --after 'same command: passing change' \
  --evidence '/path/to/qa/bundle.json' --output pr-body.md
gh pr create --base main --head factory/example --title 'Describe the change' \
  --body-file pr-body.md
```

The default Summary shows the actual Git file changes between immutable
commits. Replace it with a smaller domain visual when that explains the change
better. Missing observations and declarations remain visibly unprovided; a
commit comparison is not a passing behavior test. Task notes with `qa-bundle:`
or `base-vs-change:` supply evidence references automatically. Merge-request
composition adds this body beside the existing typed envelope and keeps the
worker's original message, QA hold and immutable envelope semantics.

Set `door=one-way` or `door=two-way` in task create/update; an empty update clears
it. Legacy tasks have no declared door. The store records it and task show and
review bodies display it. No merge gate reads door. This is an operator decision
until the reviewer accuracy evaluation passes.

The release stage preserves the selected CHANGELOG section and every PASS/FAIL
gate row. `CAS_RELEASE_TRAIN_TASK_RISK` and `CAS_RELEASE_TRAIN_TASK_DOOR` copy the
release task's declarations into its body; unset values print `not declared`.
The existing gate receipt comment remains available too.

Adapted from Matt Pocock's MIT
[PR skill](https://github.com/mattpocock/skills/tree/main/skills/engineering/pr),
which credits Dex Horthy and HumanLayer's
[show-me skill](https://github.com/humanlayer/skills/blob/main/plugins/show-me/skills/show-me/SKILL.md).
The upstream permission notice is retained in
[PR_TEMPLATE_LICENSE](PR_TEMPLATE_LICENSE).
