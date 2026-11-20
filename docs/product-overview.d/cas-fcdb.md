## Core Concepts

- **One `/finish` per harness** - Cassy ships finish as a builtin skill installed once at user level for Claude, Codex and Grok (OpenCode keeps its project copy), so each harness lists it once. `cas update` removes the old personal `/finish` seed from every harness by its exact bytes and prunes a managed project copy; edited copies stay.
