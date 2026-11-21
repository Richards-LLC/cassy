### Slow task response recovery

When a committed task operation answers before its follow-up finishes, the MCP response identifies the action and task and includes a bounded notes request. Start responses confirm assignment acceptance and direct callers to START CONTEXT and close checks once follow-up finishes. Close responses continue to describe close follow-up.

A task show/get or notes read that reaches its response deadline includes pagination guidance and a structured recovery request with full=false, offset and limit. Notes writes retain mutation uncertainty/commit semantics and do not receive read-only retry advice.
