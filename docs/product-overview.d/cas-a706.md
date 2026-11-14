## Core Concepts

A worker's blocked hold and answered-blocker metadata belong to that worker's assignment. Releasing or resetting a retired assignment clears both before a replacement receives the task, so the replacement starts open without inheriting an old block or timestamp.
