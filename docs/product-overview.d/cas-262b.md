## Core Concepts

Release dependency caches and compiler objects share GitHub's repository storage quota. Main seeds reusable compiler objects; factory, train and queue builds read them without adding branch-specific copies. Dependency cache identity continues to include third-party dependencies, toolchain and build compatibility inputs while excluding workspace version bumps.
