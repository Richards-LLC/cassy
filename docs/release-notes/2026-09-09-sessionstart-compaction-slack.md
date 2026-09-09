# Slack draft — SessionStart compaction keeps ambient recall (main merge, PR #783)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: when Cassy's start-up briefing grew too long, the first thing it dropped was your own recalled memory, silently. Now: it trims static lists first, keeps your recall and inbox, and says on screen what it trimmed.

Reply:
• Recall survives — Was: once the start-up briefing passed its size limit, the section holding your recalled memories was the first to be cut, and nothing said so. Now: static lists are trimmed first, recall and inbox are kept whole, and a one-line note in the briefing names whatever was trimmed.
• Room to grow — Was: the briefing sat within about a hundred bytes of the limit, so any small wording change could trigger the cut. Now: the guidance text is over a kilobyte shorter, leaving real headroom.
• Early warning — Was: nobody knew headroom was gone until recall vanished. Now: `cas doctor` shows a "SessionStart budget" row and flags when headroom drops below a safe margin.

## Dev thread

Top-level:
Live on production — Dev — Was: SessionStart compaction picked sections by byte saving, so the ambient recall section went first when supervisor guidance crossed 9,216 B. Now: semantic compaction priorities (static → context → ambient → factory inbox), an in-payload compaction marker, a production-shape regression, a >1 KB guidance trim, and a doctor row with a 512 B headroom floor (PR #783).

Reply:
• Compaction order — Was: `SessionContextAssembler` degraded whichever section saved the most bytes, so the ambient recall card was cut first and only a stderr line said so. Now: segments carry a label and priority (static listings, then context, then ambient recall, then factory inbox); order is priority, then byte saving, then assembly order; a compacted segment renders `[SessionStart compacted: <section>]` in the payload.
• Regression — Was: only the parity test noticed a payload over budget. Now: a production-shape test builds real supervisor guidance, pushes it past 9,216 B, and asserts the Available Skills listing compacts while the recall card stays whole.
• Budget headroom — Was: 117 B of headroom. Now: guidance bodies trimmed from ~6,750 B to 5,708 / 5,706 / 5,703 B (breadcrumbs and the issue component registry keys retained); parity payload measured 7,827 B before the registry restore.
• Doctor row — Was: no observable headroom check. Now: `SessionStart budget` reports guidance size against the 8,192 B protected ceiling and fails under 512 B headroom; a builtins self-test enforces the same floor.

Proof: session_budget 12/12; doctor 133/133; guidance self-test 5/5; doctor snapshot 2/2; guardrails + drift + agent contract + issue_intake_directive 36/36; parity 6/6; terminal-qa PASS 12 runs. PR #783 (replaced #782 after the merge queue caught the collapsed registry keys).

## POSTED
Posted 2026-09-09 15:48Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788968890.531419 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788968890531419 (reply ts 1788968902.183849)
- Dev top-level ts 1788968904.356749 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788968904356749 (reply ts 1788968918.741069)
