## Core Concepts

- **Parent-plan artifacts in child verification** - A child task's pre-merge verifier receives the artifacts its parent epic cites. Each cited `art-` ID resolves against the task that owns it within the project's scope, so an epic can mention another task's artifact. A mention that no task in scope owns reaches the verifier as missing context and never refuses the merge. A resolved artifact must still be available and unrevoked, and its bytes must match before the verifier reads them.
