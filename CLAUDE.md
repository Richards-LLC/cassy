<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->
@AGENTS.md

Claude Code: load the Cassy tool schemas once per session with ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search"). ToolSearch only loads the schema — it does not call the tool. Once it succeeds, call `mcp__cas__task` etc. directly; never re-run ToolSearch for a tool already resolved.
<!-- CAS:END -->

After writing markdown to a file, summarise it in chat instead of echoing it, and avoid nested fenced blocks. This avoids the Claude Code Ink `<Box>`-in-`<Text>` crash.

Browser build/check policy is in AGENTS.md: affected journeys at four workers for
workers and independent QA; full Playwright only for supervisor epic assembly
and the merge queue. Use `scripts/journey-eval.sh <task-artifact-dir>`; preserve
failures when rerunning one failing spec at one worker.

Local sccache 0.10.0 measured 0/45 cross-worktree Rust cache hits; hardlink
target seeding supplies cross-worktree reuse, and Cargo reuses each worktree's
existing target. Capped worker checks and named tests explicitly disable both
compiler wrappers so a cold sccache client cannot pass the private target lease
to its daemon. Supervisor and CI builds retain sccache. Prestarting a cache
server is insufficient because it can exit before the next compiler invocation.
