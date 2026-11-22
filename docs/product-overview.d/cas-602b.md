## Core Concepts

A parked worker receives an informational, quiet receipt when its approved PR
merges. The receipt records the task and merge commit without asking for another
close. Cassy's merge/verdict path owns finalization; missing approval and timeout
recovery remain addressed to the supervisor who owns that delivery.
