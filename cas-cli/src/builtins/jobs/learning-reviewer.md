# learning-reviewer job

Review accumulated learnings and promote valuable ones to rules or skills.

## Input

The queued job prompt supplies the complete list of unreviewed learning IDs for this run. Process exactly those IDs. Do not discover a different set by listing the store, and do not silently skip an ID.

Your job is incomplete until you call `mcp__cas__memory action=mark_reviewed id=<id>` for every learning ID from the queued prompt. The "reviewed" tag is not enough; only `mark_reviewed` removes a learning from the unreviewed list.

## Process

List the existing skills once, before the loop: `mcp__cas__skill action=list_all`.

For each learning ID from the queued prompt:

1. **Read:** `mcp__cas__memory action=get id=<id>`.
2. **Assess quality.** Is the learning specific and actionable, or vague and generic?
   - Good: "SQLite busy_timeout must be set on every new connection in multi-agent mode to prevent SQLITE_BUSY errors."
   - Bad: "Be careful with database connections."
3. **Check existing coverage:** `mcp__cas__rule action=check_similar content="<learning content>"`, and the skill list from above.
4. **Look for structural enforcement before writing prose.** Could a lint, test, hook, close gate, or schema/type make the failure impossible or detect it automatically? Prefer a check or a canonical helper over another prose rule. If the lesson has recurred in at least two distinct source entries, record the source IDs and route an enforceable candidate to a draft rule tagged `enforceable:lint`, `enforceable:hook`, `enforceable:gate`, or `enforceable:type` so promotion can file an implementation chore. A one-off stays a learning unless it independently warrants a rule or skill.
5. **Decide:**
   - **Rule:** a behavioural constraint ("always X", "never Y") that applies broadly, in 1–3 sentences.
   - **Skill:** a multi-step procedure, code template, or domain workflow.
   - **Strengthen existing:** a similar rule exists and the learning adds nuance. Update that rule instead of creating a new one.
   - **Keep as learning:** project-specific, a one-time fix, already covered, or too vague.
6. **Create or update:**
   - New rule: `mcp__cas__rule action=create content="..." tags="from_learning,enforceable:lint" source_ids="<learning ID(s)>"`. Use the applicable mechanism tag only when the preceding check found a concrete mechanism; otherwise use `tags="from_learning"`. Retain every distinct source ID.
   - Update a rule: `mcp__cas__rule action=update id=<existing> content="<improved>" change_note="strengthened from <learning ID>"`.
   - New skill: `mcp__cas__skill action=create name="..." summary="..." description="..." invocation="<how to invoke it, e.g. /skill-name>" scope=project draft=true tags="from_learning" source_ids="<learning ID(s)>"`. A skill from a learning starts as a project-scoped draft; never publish it globally from this job.
   - Always pass the originating learning ID(s) in `source_ids`, comma-separated.
7. **Mark reviewed:** `mcp__cas__memory action=mark_reviewed id=<id>`.

## Decision guide

| Signal | Outcome |
|--------|---------|
| "Always X" / "Never Y" | Rule |
| Repeated mistake (seen in several tasks) | Rule |
| Multi-step procedure, code template, debugging workflow | Skill (project draft) |
| One-time bug fix, context about one file | Keep |
| Vague observation | Keep, or archive if it has no value |
| Similar rule already exists | Update that rule |

## Guidelines

- Be selective: quality over quantity. Batch similar learnings into one rule.
- If it takes more than 3 sentences, it is probably a skill, not a rule.
- Archive low-value learnings: `mcp__cas__memory action=archive id=<id>`.
- Mark every learning reviewed, including those you do not promote.
