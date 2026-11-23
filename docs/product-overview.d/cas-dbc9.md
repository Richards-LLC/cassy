## Core Concepts

A task's recorded source branch follows a transfer. If the new assignee's branch already contains every tip of the previous source branch, task start binds it. Otherwise park binds it once that branch contains them, for example after the worker merges the previous branch in. Binding retires any earlier anchor to history and records an audited rebind in the task notes. If a previous tip is missing from the new branch, the recorded source stays unchanged and the supervisor's explicit `rebind_branch` remains the recovery.
