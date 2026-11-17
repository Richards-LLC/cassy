<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->
@AGENTS.md

Claude Code: load the Cassy tool schemas once per session with ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search"). ToolSearch only loads the schema — it does not call the tool. Once it succeeds, call `mcp__cas__task` etc. directly; never re-run ToolSearch for a tool already resolved.
<!-- CAS:END -->

After writing markdown to a file, summarise it in chat instead of echoing it, and avoid nested fenced blocks. This avoids the Claude Code Ink `<Box>`-in-`<Text>` crash.
