## Core Concepts

A factory worker owns two kinds of branch: `factory/<name>` and `factory/<name>-<task-id>`. The tool guard and the installed pre-commit and pre-push hooks all apply this one rule, from the branch name alone. A branch's owner is never guessed from its name: a refusal names another worker only when that worker is registered. When the branch isn't yours, the refusal prints how to save your edits and switch back. Registration keeps worker and supervisor names from sharing a branch namespace.

`transfer adopt_branch` adopts the delivery a task actually holds, never anything past it. It works when the receiver already sits on one of the task's branches. It also works when the receiver is at a newer train tip with no work of its own, and after a review return has moved the task's anchor into history.
