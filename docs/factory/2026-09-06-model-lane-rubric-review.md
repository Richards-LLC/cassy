# Model lane rubric review — 2026-09-06

Audience: operator (Daniel). Type: decision brief. Author: supervisor session golden-panda-80.
Evidence window: 2026-09-05 16:20Z – 2026-09-06 00:10Z (the cas-80b6 rescue, releases 3.17.1 and 3.17.2).

## Verdict

The rubric routes by reputation, not by measurement. In the one window where we have hard numbers,
the lane that carried the work (Codex Luna at xhigh, "standard") is absent from the risk-bearing
lane ("heavy"), the model with the only recorded stall (Codex Astra) is being promoted into that
lane, and the two judgment lanes now fall back silently to the model they replaced. Nothing in the
rubric names an effort rationale, a cost ceiling, or a promotion rule. Three numbers would fix that,
and one of them already ships in 3.17.2.

## The rubric as directed (2026-09-06 00:10Z, task cas-255e in flight)

| Lane | Primary | Effort | Fallback | Intended use |
|---|---|---|---|---|
| light | Claude Haiku 4.5 | low | Codex Luna / xhigh | mechanical chores |
| standard | Codex GPT-5.6 Luna | xhigh | Claude Opus 5 / high | ordinary implementation |
| taste | Claude Fable 5.1 | medium | Claude Opus 5 / high | public surfaces, prompts, docs, judgment |
| supervisor | Claude Fable 5.1 | medium | Claude Opus 5 / high | factory coordinator |
| heavy | Codex GPT-6 Astra | high | Codex GPT-5.6 Sol / high | implementation with safety risk |

Explicit-only recipes: Codex Sol (former heavy), Codex Terra (suspended 2026-08-27), Qwen 3.8 Max via
OpenCode (receipt-gated). Source: `crates/cas-factory/policy/lane-registry.toml` at epic tip
736bb1fe plus the cas-255e directive.

## What the evidence window shows

Deliveries reviewed by the supervisor between 16:20Z and 00:10Z, by the lane that produced them.
"Send-back" means a review rejection or CI red that required a corrective commit before merge.

| Lane (model / effort) | Workers | Deliveries | Send-backs | CI reds | Merged |
|---|---|---|---|---|---|
| standard (Luna / xhigh) | 8 | 19 | 5 | 1 | 19 |
| heavy (Sol / high) | 1 | 1 | 1 | 0 | 1 (after continuation on Luna) |
| taste (Fable / medium) | 0 as worker; 1 as supervisor | — | — | — | — |
| light (Haiku / low) | 0 | 0 | 0 | 0 | 0 |
| Astra (any) | 0 as worker | 0 | 0 | 0 | 0 |

Send-back detail (standard): cas-62ca ×2 (doctor snapshot row missed; managed-block line budget),
cas-d05f ×2 (fixture escaped the checkout; then a helper contract change that generalised the same
defect), cas-1e85 ×1 (release-note "Was" wording). Send-back detail (heavy): cas-c674 deleted the
supervisor skill's Operating flow section and conflicted with the epic; a Luna worker finished it.

Supervisor stall on record: the previous supervisor held finished workers with green proofs from
about 06:00Z to 15:10Z on 2026-09-05 (nine hours of actionable idle) — the operator attributes this
behaviour to Astra. Source: cas-20a3 note 15:10Z; operator statement 17:27Z.

Release latency in the same window: 3.17.1 from rescue start to published 4 h 57 min, including two
merge-queue failures caused by a test from an earlier lane; 3.17.2 from "cut it now" to published
77 min with one gate and one queue run.

## Where the rubric fails

1. **Heavy is routed on reputation against the data.** Astra has zero worker deliveries in the
   window and one recorded coordination stall. Sol has one delivery and one send-back. Luna has
   nineteen merged deliveries and is not in heavy at all.
2. **Silent fallback to the replaced default.** Opus 5/high was the built-in supervisor default until
   2026-09-05 22:00Z. It is now the automatic backup for both judgment lanes. A bad auth hour changes
   the coordinator's behaviour with nobody deciding it. cas-255e adds a loud receipt; loud is not
   approved.
3. **Effort has no stated rationale.** Luna xhigh only; Fable medium; Astra medium by recipe but
   high in heavy; Sol high; Haiku low. A reader cannot tell cost decisions from quality decisions.
4. **Nothing is measured.** Every routing change this week (Astra→Fable taste, Fable supervisor,
   Opus fallbacks, Astra heavy) came from anecdote. The actionable-idle metric that shipped in 3.17.2
   is the first number the rubric has ever had.
5. **The light lane is decorative.** Zero uses in the window. Every mechanical chore went to Luna
   because Haiku is not trusted with builtin marker tests.
6. **Cross-harness fallback inside a lane.** Standard falls back from Codex to Claude: different
   hooks, skill mirror, and account mid-epic.
7. **Fallback edges are declared but disabled.** Until cas-255e lands, taste and supervisor carry a
   fallback that never fires; the comment says "fail closed". A registry that documents one policy
   and executes another is a review hazard.

## Optimizations, in order of leverage

1. **Measure three numbers per lane, per week**, from data CAS already records:
   send-backs per delivery (task notes carry request_changes), actionable-idle minutes (3.17.2
   metric), and assignment-to-first-push minutes (lease start vs first pushed tip). Print them in the
   generated route table so every reader sees the rubric and its scorecard together.
2. **Promote by trial, not by directive.** Keep Sol as heavy primary; make Astra/high the heavy
   fallback for two weeks; promote only if its send-back rate is at or below Sol's on at least five
   deliveries.
3. **Make fallbacks explicit decisions.** A lane fallback fires only after the supervisor is told
   the primary is unavailable and the receipt names both recipes. For the supervisor lane, prefer
   fail-closed plus an operator alert over a silent model change.
4. **Add an effort column with a reason.** One sentence per lane: why this effort, what it costs
   relative to the lane below, and what evidence would change it.
5. **Retire or re-scope light.** Either route it at Luna/xhigh for tiny chores (which is what
   happens today) or give Haiku a bounded class of work with its own marker tests.
6. **Keep fallbacks inside a harness.** Standard should fall back to another Codex recipe or fail
   closed; cross-harness fallbacks belong to the operator, not the registry.
7. **Give Luna a seat in heavy.** On this window's evidence it is the safest implementation lane we
   have; at minimum it should be the heavy fallback ahead of Sol.

## Decision requested

Keep cas-255e as directed (it is implementing the rubric above), and choose one of:

- A. Ship it as directed and start the three-number scorecard now; revisit in two weeks.
- B. Amend cas-255e so heavy stays on Sol with Astra/high as fallback, and add the scorecard.

The author recommends B.

## Provenance

- Registry: `crates/cas-factory/policy/lane-registry.toml` at epic 736bb1fe64176204b77e46b960ad23fba7d8cbba.
- Delivery and send-back counts: supervisor merge and review notes on epic cas-80b6, 2026-09-05 16:20Z–2026-09-06 00:10Z; task notes on cas-4626, c650, 62ca, bd04, bddf, a49c, 41ae, d05f, 47ea, e159, c674, 9eae1, 72f7, 16ee, a65d, 6e24, 1e85, 826a.
- Stall: cas-20a3 note 15:10Z; operator message 2026-09-05 17:27Z.
- Release timings: `/home/pippenz/.cas/artifacts/release/v3.17.1-epic-80b6-merge/FINAL-HANDOFF.md` and `.../v3.17.2-epic-80b6-merge/FINAL-HANDOFF.md`.
- Directive: operator message 2026-09-06 00:10Z; task cas-255e.
