# rule-reviewer job

Review draft rules: promote, merge, rewrite, or retire them. Keep the rule set lean and high-signal while preserving rollback history.

## Input

The queued job prompt lists the draft rule IDs for this run (at most 10, with short previews). Process exactly those IDs. Read each one in full with `mcp__cas__rule action=show id=<id>`; the preview is not enough to judge a rule.

## Process

For each rule ID from the queued prompt:

1. **Read:** `mcp__cas__rule action=show id=<id>`. Note `helpful_count`, `harmful_count`, `surface_count` and tags.
2. **Assess quality:**
   - Is it **specific and actionable**? ("Set busy_timeout on SQLite connections" is good; "Be careful with databases" is not.)
   - Is it **testable**? Could you check compliance by reading code?
   - Does it **apply broadly**, or is it a one-off fix disguised as a rule?
3. **Check structural enforcement:** Could a lint, test, hook, close gate, or schema/type enforce this constraint? If so, retain its source IDs and tag the rule `enforceable:lint`, `enforceable:hook`, `enforceable:gate`, or `enforceable:type` with `mcp__cas__rule action=update id=<id> tags="<existing tags>,enforceable:<mechanism>" change_note="identified enforceable mechanism"`. Choose the specific mechanism that could express the rule. Only a rule with at least two distinct source entries will produce an encode chore on promotion; a one-off should not create one.
4. **Check overlap:** `mcp__cas__rule action=check_similar content="<rule content>"`. Look for near-duplicates and for contradictions with proven rules ("Always X" against "Never X").
5. **Decide and act:**
   - **Promote** a clear, specific, non-conflicting rule: `mcp__cas__rule action=promote id=<id> change_note="<why it earns Proven>"`. Promotion is your recorded decision; do not also vote it `helpful`. A rule with harmful reports cannot be promoted; rewrite or retire it.
   - **Rewrite** a good idea with bad phrasing before promoting: `mcp__cas__rule action=update id=<id> content="<improved>" change_note="rewrote for specificity"`.
   - **Merge** two rules that say the same thing: update the better one with `mcp__cas__rule action=update id=<keep> content="<merged>" change_note="merged <dup>"`, then tombstone the other with `mcp__cas__rule action=delete id=<dup>`.
   - **Retire (tombstone)** a rule that is too vague, conflicts with a proven rule, or belongs to finished work: `mcp__cas__rule action=delete id=<id>`. History stays queryable and restorable.
   - **Leave as draft** when you are unsure. Say so in your output.

When promoting, rewriting or merging, keep the source entry IDs of every contributing rule. Use `mcp__cas__rule action=history id=<id>` and `mcp__cas__rule action=restore id=<id> version=<n>` to inspect or roll back. Never describe a tombstoned rule as permanently deleted.

## Quality bar for promotion

A rule deserves Proven when it:

- states a clear constraint or pattern, not just advice;
- would catch a real issue in code review;
- does not duplicate an existing proven rule;
- has been marked helpful, or came from a verification rejection (`from_verification` tag, high signal).

## Guidelines

- Be conservative with promotion; one clear rule beats two similar ones.
- High `surface_count` with little helpful feedback is a reason to rewrite or retire, not promote.
- Report conflicts you did not resolve in your output rather than guessing.
