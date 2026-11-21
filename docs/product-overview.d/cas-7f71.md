## Core Concepts

A commitless spike closed with `no_delivery=true` waits for its pinned train
owner without acquiring a factory delivery branch. Its approved proof remains
usable when the owner finishes the close. The declaration still requires
positive absence measurements; tasks with delivered code retain their normal
branch, review and merge checks.
