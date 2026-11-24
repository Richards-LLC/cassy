## Factory

- Raw GitHub merges honor the verifier verdict recorded for the pull request's exact head, independently of the QA setting. A supervisor can override a rejection by recording a task decision note in the form `Raw GitHub merge override <dispatch>: <reason>`. The hook logs the reason before allowing the override. Pending reviews cannot be overridden.
