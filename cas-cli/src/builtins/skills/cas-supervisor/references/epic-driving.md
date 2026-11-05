# Epic-driving playbook

- Create the epic branch at EPIC creation; call `focus_epic`; set every child's `target_branch` to it, including late additions and `blocked_by` follow-ups.
- Reject any child whose `WorkTarget` resolves to trunk; repair its target before spawning.
- Merge every factory branch into the epic branch with `worktree_merge`; open exactly one integration PR from the epic branch to the default branch.
- At session start, process `awaiting_merge` before open work; arm a durable event wake for externally parked merges.
- Cap workers: one pre-assigned isolated spawn per file-group lane (`spawn_workers count=1 lane=<lane> isolate=true task_id=<task-id>`); assign later lane tasks to it. Create blocked follow-ups before their dependency completes.
- Set `confirm_warning=true` for intentional late additions to an active epic.
- Set `proof_scope_fix=true` and bind `known-repos` when a receipt names the wrong repository.
- Carry the version bump, CHANGELOG section and release-notes draft as the epic branch's final commit, landed through its single integration PR before tagging.
- Own the release cut; wait for the project's release build to finish before tagging or publishing.
- The operator's QA walk cadence is recorded on the epic; a peer plan never overrides it.
