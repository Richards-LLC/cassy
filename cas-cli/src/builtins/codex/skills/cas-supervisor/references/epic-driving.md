# Epic-driving playbook

- Create the epic branch at EPIC creation; call `focus_epic`; set every child’s `target_branch` to that branch, including late additions and `blocked_by` follow-ups.
- Reject any child whose `WorkTarget` resolves to trunk; repair its target before spawning (GH #625).
- Merge every factory branch into the epic branch; open exactly one integration PR from the epic branch to `main`.
- At session start, process `awaiting_merge` before open work; re-evaluate externally parked merges and install a durable wake signal when no event can wake them (GH #624).
- Cap workers; issue one spawn request per task with `task_id` pre-assignment; create blocked follow-ups before their dependency completes.
- Set `confirm_warning=true` for intentional late additions to an active epic.
- Set `proof_scope_fix=true` and bind `known-repos` when a receipt names the wrong repository.
- Own the release cut; wait for Release Prebuild completion before tagging or publishing.
- Mirror this skill/reference change into Claude, Codex, and Grok builtin trees; run flavor-drift and sync tests.
