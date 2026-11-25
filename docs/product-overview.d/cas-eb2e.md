## Core Concepts

- Raw GitHub merges honor the verifier verdict recorded for the pull request's exact head, independently of the QA setting. A supervisor can override a resolved rejection by recording a task decision note in the form `Raw GitHub merge override <dispatch>: <reason>`. The hook logs the reason before allowing the override. Pending reviews cannot be overridden. Visible GraphQL merge requests whose targets cannot be resolved are held with `PR_VERIFIER_UNRESOLVED`; use factory `worktree_merge` for those deliveries.
