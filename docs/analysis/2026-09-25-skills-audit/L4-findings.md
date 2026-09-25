# L4 audit — design and reporting skills (cas-a4d8)

2026-09-25 · warm-marten-55 · EPIC cas-1660 · scored against L1 rubric v1
(`~/.cas/artifacts/cas-63c5/rubric.md`) · baseline `docs/analysis/2026-09-02-builtin-skills-review.md`
· tree `factory/warm-marten-55` @ 4836e56f7 (v3.31.0). Findings only: no repo file edited, no cargo run.

## Verdict

These nine skills are well written sentence by sentence. The prose is plain, there is almost no
shouting (one `IMPORTANT` in design-spec), most steps have done-states, and every bundled script
still runs. The defects are structural:

1. **One shipped exemplar leaks operator-private data to every install.** It is a regression of
   the 2026-09-02 review's de-operator-ise item.
2. **cas-release-report ignores the product's own `cas release report` command.** The actual
   release practice has also drifted away from the skill's contract.
3. **Five skills give conflicting rules about the same page.** They give three form-choice
   tables, four restatements of the ship floor, two token vocabularies, and two heroes for a
   release summary.
4. **The QA gates point to scripts that ship only in cas-src**: `visual-qa.mjs` and
   `terminal-qa.mjs`.
5. **An agent triggering cas-html-reports is told to read about 111 KB (≈27.7 k tokens)
   before it renders anything.** Opening the exemplars as instructed adds up to about 221 KB
   more.

## Per-skill loaded size (bytes; tokens ≈ bytes ÷ 4)

"Mandated path" means SKILL.md plus every file the procedure tells the agent to read on a
normal run. It includes cross-skill reads. Opening the exemplars is excluded.

| Skill | SKILL.md (per-invoke) | Mandated path | Whole dir (on-demand ceiling) | Description chars (always) |
|---|---:|---:|---:|---:|
| cas-html-reports | 9,005 | **110,728 (≈27.7 k tok)** | 288,881 | 189 |
| cas-ui-craft | 5,585 | 59,971 (≈15.0 k) | 144,704 | **428** |
| cas-dataviz | 8,465 | 40,249 (≈10.1 k) | 40,801 | 307 |
| cas-technical-drawing | 4,660 | 14,228 (≈3.6 k) | 144,019 (draft.mjs 123 KB: run it, don't read it) | 398 |
| cas-release-report | 3,688 | 16,880 (≈4.2 k) | 50,649 | 152 |
| design-spec | 8,629 | 10,564; greenfield 44,487 | 42,552 | 131 |
| cas-image-generate | 3,888 | 13,849; +SVG route 25,956 | 37,161 | 274 |
| cas-cli-craft | 5,188 | 16,398 (≈4.1 k) | 31,574 | **514** |
| cas-frontend-engineering | 7,835 | 7,835 + project DESIGN.md | 7,835 | 168 |
| **Total** | 56,943 | — | **788,176** | 2,561 |

- All files are `include_str!`'d three times: Claude, Codex, and Grok trees. That puts about
  2.36 MB of these nine skills in the binary.
- The mirrors are identical except for the intended tool prefix in `design-spec/SKILL.md:56`
  (`mcp__cs__` for Codex, `cas__` for Grok). Axis 7 is clean.
- Every file under the nine directories is registered once per flavour in `builtins.rs`: 0
  unregistered, 0 double-registered.
- Two cross-lane P0s apply here and are not re-reported: non-SKILL.md files never refresh after
  first install (`sync_builtin_detailed`), and Grok resolves `.claude/skills`. The first one
  means F1's leak and every script fix below stay stale in existing installs until that P0 lands.

## Ownership-boundary map

| Concern | Claimed owner (where) | Also restated or contradicted in | Status |
|---|---|---|---|
| Concept brief template | ui-craft `references/concept-brief.md` | html-reports SKILL:27-34; dataviz SKILL:19; cli-craft has its own (fine, different fields) | overlap, consistent |
| Form choice (reader task → form) | ui-craft `form-vocabulary.md` (ui-craft SKILL:27) | dataviz SKILL:21-31; html-reports `presentation-rules.md:17-30` + `:68-79` | **3 tables, conflicting** (F4) |
| Chart construction / number formatting / palette validation | dataviz (ui-craft SKILL:76) | html-reports `presentation-rules.md` (tables, numbers in prose, legends, scales) | duplicated |
| Invariant technical contract | html-reports `technical-contract.md` | html-reports SKILL:90-106; ui-craft SKILL:31-39; review-checklist §3–§9 | restated 3× (F11) |
| Critique rubric and ship floor | ui-craft `critique-rubric.md` | html-reports SKILL:39-46; ui-craft SKILL:40-47; dataviz SKILL:50; `quality-checklist.md:11`; review-checklist §0 | floor restated 5× (F11) |
| Visual-QA receipt | nobody ships it (repo `scripts/visual-qa.mjs`) | ui-craft SKILL:37; critique-rubric:22-32; html-reports SKILL:45, review-checklist §12; dataviz SKILL:50 | **gate without a shipped tool** (F5) |
| Token vocabulary | design-spec `design-tokens.json` (Petrastella roles `ink/verdict/evidence…`) | design-spec SKILL:71 DESIGN.md frontmatter roles `text/primary/accent…`; release-report `default-tokens.json` (copy) | **two schemas, no mapping** (F6) |
| Release summary report | release-report (description) | html-reports SKILL:61 + `report-types.md:37,126` ("Status / release summary", dumbbell hero) | **both fire, different heroes** (F3) |
| Interactive vs static charts | harness-bundled `dataviz` (Claude) | cas-dataviz description + SKILL:9,54-56 | boundary stated one-sided (F16) |
| HTML surfaces vs terminal | ui-craft vs cli-craft | both descriptions name the other | clean |
| Implementation vs design | frontend-engineering (brief → code) vs ui-craft (design and critique) | frontend SKILL:10-11 | clean; ui-craft never points to frontend-engineering (P3) |
| Image assets | image-generate | ui-craft/html-reports "inline SVG / data URI" | clean |

## Findings (severity-ranked)

Surface: `always` means description, `per-invoke` means SKILL.md body, `on-demand` means
references and scripts. Δ tokens ≈ bytes ÷ 4; a negative Δ is a saving.

### P0: misleads an agent today

| # | Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|---|
| F1 | P0 (regression of 09-02 P1 #8) | on-demand; shipped to every install ×3 flavours | `cas-html-reports/references/examples/before-after/rubric-review-before.html:84,161,177,265,273-275`; `rubric-review-after.html:401,441,665` | The before/after exemplar is a real internal operator report. It ships private operator data into every downstream project. It also breaks the skill's own external-deliverable rule (`technical-contract.md:135-136`). | The two files contain the e-mails `pippenz@gmail.com`, `support@gabber.studio` and `daniel@petrastella.io`. They also contain `~/.codex-support@gabber.studio/sessions/...` rollout paths, 150–165 `$` cost figures (e.g. "$315.31 at Astra list", "$639.23"), 12 `cas-xxxx` ids per file, and worker names (`golden-panda-80`, `vivid-kestrel-88`). The other 7 html/ui-craft exemplars have 0 hits. | Delete both HTML files plus `rubric-review.brief.md`/`.why.md`. Point SKILL.md:127-128 at `cas-ui-craft/references/exemplars/before-after.html`, which already shows the same lesson with synthetic data. Add these files to the retired-vocabulary/operator-data lint the 09-02 review proposed. Because of the cross-lane refresh P0, installed copies must be pruned explicitly. | −42.9 k on-demand; −170 KB ×3 binary |
| F2 | P0 | per-invoke | `cas-release-report/SKILL.md:9-53` (whole procedure), `:27-32` | The skill never mentions `cas release report <version> [--pdf] [--refresh-sources]`. That command gathers the sources, assembles the Markdown, renders through this skill's own `render.py`/`template.html`, and optionally renders the PDF. The skill instead prescribes a hand-written Markdown file, a hand-run renderer, and PDF scripts pasted from `pdf.md`. | `cas release report --help` (cas 3.31.0); `cas-cli/src/cli/release_report.rs:44-61,1623,1789-1835`. Practice has drifted from the contract: **22 of 29** `docs/release-reports/*` sources since v3.22.1 have no `<v>.brief.md` and no `<v>.visual-qa.md`, which skill steps 3, 8 and 9 require. Only v3.25.4, v3.25.6 and v3.25.7 comply. | Make step 1 "Run `cas release report <v> --pdf` from the project root. It writes `<out>/<v>.md/.html/.pdf`." Keep steps 3-4 (brief, identity) and 8 (rubric + QA) as the craft layer on top of it. Otherwise, decide explicitly that CLI-generated reports are exempt from the brief and QA, and say so in the skill. Either way, the skill and the command must stop disagreeing. | ≈ −300 per invoke (steps 5/7 shrink) |

### P1: routing, format, or contract defects

| # | Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|---|
| F3 | P1 | always + per-invoke | `cas-html-reports/SKILL.md:3,61`; `references/report-types.md:37,126-138`; `cas-release-report/SKILL.md:3` | A release report triggers both skills, and each prescribes a different hero and file layout. html-reports wants `docs/<area>/YYYY-MM-DD-<topic>.md` with a was→now dumbbell hero. release-report wants `docs/release-reports/<v>.md` with closure-stitch dots. Neither names the other. | html-reports lists "status and release summaries" as a canonical case. report-types type 6 names the dumbbell hero. release-report step 4 names the "closure stitches made of equal-area issue dots". | html-reports description: add "…not published version releases (cas-release-report)". report-types §6: add "A published version release uses `cas-release-report`." | +12 always; 0 |
| F4 | P1 | per-invoke + on-demand | `cas-dataviz/SKILL.md:24-29` vs `cas-ui-craft/references/form-vocabulary.md:19,31,34` vs `cas-html-reports/references/presentation-rules.md:73,79,104-110`, `report-types.md:16,214`, `review-checklist.md:51` | The three form-choice sources contradict each other, and agents obey whichever they read last. **Pie**: dataviz only avoids "pie for close values" and "more than six pie slices", which implies small pies are fine; form-vocabulary says pies are excluded by the design language. **KPI cards**: presentation-rules:79 says a single number becomes "a KPI card", :104-110 specs 3–5 cards, report-types:214 makes "KPI cards 3–5" a **required** section of the executive brief, and review-checklist:51 expects "the hero, KPI cards, and one chart" on one screen; form-vocabulary:19 says "stat strip … no boxes", its anti-defaults (:31) reject card rows, and the critique rubric scores competing cards down. | Quoted lines. | Make `form-vocabulary.md` the only form table. Replace dataviz :21-31 and presentation-rules :17-30/:68-79 with a pointer plus any genuinely extra rows (heatmap, distribution, scatter) moved into form-vocabulary. Rename "KPI cards" to "stat strip" in presentation-rules §KPI cards, report-types:16,214, and review-checklist:51. Change the dataviz "Avoid" cells to "pie (excluded by the design language)". | ≈ −900 per html-reports run; −350 per dataviz invoke |
| F5 | P1 | per-invoke | `cas-ui-craft/SKILL.md:37-39`; `references/critique-rubric.md:22-32`; `cas-html-reports/SKILL.md:45`, `review-checklist.md:112-118`; `cas-dataviz/SKILL.md:50`; `cas-cli-craft/SKILL.md:39-45`, `references/critique-rubric.md:33`; description `cas-cli-craft/SKILL.md:3` | The mechanical gate relies on `node scripts/visual-qa.mjs` and `node scripts/terminal-qa.mjs`, which exist only in the cas-src repo (`scripts/`, 38 KB and 43 KB) and are not shipped with any skill. The fallback is inconsistent. ui-craft allows checking by eye (:39); critique-rubric:28-32 and review-checklist §12 **require** a `--strict` PASS receipt under `artifacts_root/<task-id>/` with no fallback; cli-craft has no fallback at all, and its description promises "a terminal-qa PASS receipt". Downstream projects therefore cannot meet the public-surface floor as written. Second defect: `scripts/visual-qa.mjs` is project-relative, but `scripts/validate_palette.js` (dataviz :47) is skill-relative, and both are written the same way. | `ls scripts/` in cas-src shows `visual-qa.mjs` and `terminal-qa.mjs`; neither is in any `builtins.rs` entry. `visual-qa.mjs:562` defaults its receipts to `docs/factory/data/visual-qa` in cwd, not `artifacts_root`. `visual-qa.mjs:14` uses 390×800 while the skills say 390×844. Playwright is required (`:394`) and never mentioned in the skills. | Ship both scripts as `cas-ui-craft/scripts/visual-qa.mjs` and `cas-cli-craft/scripts/terminal-qa.mjs`, as release-report does with render.py. Write every script invocation as `node <skill-dir>/scripts/…`. State the fallback once in critique-rubric: no Playwright/Chromium → the four manual renders, noted in the evidence. Pass `--artifact-dir` in the documented command. | 0 per invoke; +81 KB ×3 binary |
| F6 | P1 | per-invoke | `design-spec/SKILL.md:66-76` vs `cas-ui-craft/SKILL.md:18-22`, `cas-html-reports/references/technical-contract.md:57-63`, `cas-release-report/SKILL.md:19-21` | There are two token vocabularies and no mapping between them. The DESIGN.md frontmatter uses roles `bg, surface, surface-raised, border, text, text-muted, primary, accent, success…`. The consumers declare `:root` "by the names in that source": Petrastella `surface-hero, line, ink, ink-muted, verdict, verdict-soft, evidence, action, good…`. An agent rendering a report for a project with a DESIGN.md cannot tell which surface is `verdict` or `evidence`. release-report adds a third route: "map project roles to the renderer's token schema", and `render.py:154` probes `design-tokens.json` and `docs/design/design-tokens.json`. | Quoted lines. | Give design-spec one frontmatter schema: Petrastella role names, with a `maps:` note when the project's own token names differ. Consumers then read DESIGN.md roles verbatim. | ≈0 |
| F7 | P1 (currency) | per-invoke | `design-spec/SKILL.md:64-87` | The frontmatter diverges from the public DESIGN.md format (google-labs-code/design.md `docs/spec.md`, version "alpha"). The spec uses `rounded` where the skill uses `radius`. Its `colors` are `<token>: <CSS color>`, while the skill uses "token name AND resolved value". Its `typography` is token→{fontFamily, fontSize…}, while the skill uses `families`/`scale`. The spec also has `components`, `omitted`, `version`, and `name`. The 8-section order is identical, so the skill clearly targets that format but misses its schema. There is also an official validator, `npx @google/design.md lint DESIGN.md` (npm 0.4.0, 2026-07-27), which includes WCAG contrast checks. The skill claims no check exists (:107,113). | exa: github.com/google-labs-code/design.md spec.md §Schema and §Section Order; `npm view @google/design.md` → 0.4.0. | Adopt the spec's key names and keep `source`/`inherits` as extra keys the spec tolerates. Add a done-state: "`npx @google/design.md lint DESIGN.md` is clean, or each warning is listed in Overview". Replace "comparing commit dates by hand is the only check" with lint + `design.md diff`. | +60 per invoke |
| F8 | P1 | on-demand (script) | `cas-image-generate/scripts/generate-image.sh:146`; `references/asset-playbook.md:10-11,29-32`; `output-checklist.md:31-36` | The playbook asks for 16:9, 2K, A4 portrait, and 1200×630. The helper cannot request any of these: the payload is `{contents:[{parts}]}` with no `generationConfig.imageConfig`. The playbook works around it with "dimensions belong in the prompt", which the models do not honour reliably, so output defaults to square 1K. | Current Gemini docs (ai.google.dev image-generation, 2026-09-23) document `generationConfig.imageConfig.{aspectRatio, imageSize}` on `generateContent` for `gemini-3.1-flash-image` / `gemini-3-pro-image` (1K/2K/4K). | Add `--aspect 16:9` and `--size 1K|2K|4K` flags that emit `generationConfig:{responseModalities:["IMAGE"],imageConfig:{…}}`. Put the flag in the playbook's "Suggested output" column and remove the "dimensions belong in the prompt" sentence. | +40 per invoke |
| F9 | P1 | always | `cas-cli-craft/SKILL.md:3` (514 chars), `cas-ui-craft/SKILL.md:3` (428 chars) | Both descriptions exceed the rubric's 400-char always-loaded ceiling. Both spend half their length on an "Owns …" inventory that matters only after the skill loads. | char counts via awk | cli-craft: "Use when designing or critiquing what a CLI or TUI prints for a person — status screens, doctor reports, tables, progress, errors, receipts, the human side of `--json`. HTML surfaces belong to cas-ui-craft." (~210). ui-craft: "Use when designing, rendering, or critiquing a human-facing HTML surface — report, dashboard, product or landing page, README hero, slide, app screen — before first render and before merge. Report contract: cas-html-reports; figures: cas-dataviz." (~250) | −75 always (×every turn, ×3 harnesses) |

### P2: efficiency and structure

| # | Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|---|
| F10 | P2 (highest multiplier) | on-demand, read on every render | `design-spec/references/design-tokens.json` (22,629 B) read via html-reports SKILL:104-105, ui-craft SKILL:18-22, dataviz SKILL:46, design-spec SKILL:36 | Every report, UI render, and figure reads a 22.6 KB DTCG JSON to copy tokens into `:root`. 3.7 KB of it is `$description` prose, and the language doc (11.3 KB) repeats the same values in tables. | A generated CSS block of color roles for both schemes plus font families is 1,247 B; with the type, space, and radius scales it is ~3 KB. | Ship `design-spec/references/tokens.css` (`:root{}` + `@media (prefers-color-scheme:dark)`), generated from the JSON and pinned by the existing parity test at `builtins.rs:5159-5264`. Consumers paste it; the JSON stays for tooling and validation. | ≈ −4.9 k per render (html-reports, ui-craft, dataviz, greenfield design-spec) |
| F11 | P2 (09-02 item unfixed) | per-invoke | `cas-html-reports/SKILL.md:90-114` | The body restates `technical-contract.md` §1-7 and presentation-rules. The 09-02 review flagged this (":86-107 restates technical-contract") and it has not been fixed. The ship floor and render matrix are restated in 5 places (see boundary map). | Side-by-side read. | Replace :90-114 with two lines: "Obey `references/technical-contract.md` (one file, no network, JS-off complete, print, provenance, tokens) and `references/presentation-rules.md`." The floor lives only in critique-rubric; the others say "meets the cas-ui-craft floor". | −430 per invoke; −600 across dataviz/review-checklist |
| F12 | P2 (09-02 item unfixed) | on-demand | `cas-html-reports/references/review-checklist.md:120-124` (+ `:4`, `:48`, `:51`) | The two-minute version still comes last (flagged 09-02). ":4" says "Twelve dimensions" but 13 sections (0-12) follow. ":48" says 360 px while every other file says 390. ":51" repeats F4's KPI cards. | File read. | Move the two-minute version to the top, fix the count and the width, and fold §12 into §0. | −150 |
| F13 | P2 | per-invoke | `cas-html-reports/SKILL.md:119` + `references/examples/*` and `cas-ui-craft/references/exemplars/*` | "Open both; read the HTML source" sends the agent through 221 KB of html-reports exemplars (≈55 k tokens). Two exemplars duplicate ui-craft's: an annotated-timeline incident (`investigation-annotated-timeline.html` vs ui-craft `report.html`) and a before/after (F1 vs ui-craft `before-after.html`). | Sizes above. | After F1, keep one exemplar per form across both skills. Tell the agent to read the `.why.md` sidecar and open the HTML only for the form it chose. | up to −40 k per run that follows the instruction |
| F14 | P2 | on-demand | `cas-release-report/references/pdf.md:9-44,54-78` | Two complete programs are pasted into a reference for the agent to "save as" and run on each release. That goes against the rubric (Axis 3: scripts are executed, not read). They also duplicate the CLI's `--pdf` path (F2). `check-pdf.py` uses `import fitz`, which PyMuPDF now documents as a legacy name that collides with the unrelated PyPI `fitz` package. | pymupdf.readthedocs.io: "Use `import pymupdf` instead of `import fitz`". | Ship `scripts/render-pdf.mjs` and `scripts/check-pdf.py` (with `import pymupdf`), or reuse the CLI. The reference keeps only the inspection checklist (:80-88). | −900 per invoke |
| F15 | P2 (09-02 item unfixed) | on-demand | `cas-image-generate/references/providers.md:44-105` | 60 lines of curl for four providers the skill forbids calling (flagged 09-02, still present). It also omits the new cheaper tier, Nano Banana 2 Lite (`gemini-3.1-flash-lite-image`), which Google now lists as the recommended successor to `gemini-2.5-flash-image`. | ai.google.dev image-generation page, 2026-09-23. | Collapse the unwired providers to a 5-row table (name, key env var, why unwired). Evaluate NB2 Lite as the `draft` tier, or name it as an option. | −1.2 k on-demand |
| F16 | P2 | per-invoke | `cas-dataviz/SKILL.md:9,54-56` | The bundled Claude `dataviz` skill claims "ANY chart … inline SVG … HTML" and asks to be read "BEFORE the first line of chart code", so on Claude both skills load for a report figure. cas-dataviz explains its "inversions" twice (:9 and :54-56, plus `design-review.md`) but never gives the rule that settles a conflict. | The bundled dataviz description in this session's skill list. | One line at the top: "When both load, this skill governs committed Cassy artifacts; the bundled skill governs live or interactive charts." Delete :56 and move `design-review.md` rationale out of the shipped set, since it is design history. | −250 per invoke; −900 on-demand |
| F17 | P2 | per-invoke | `cas-dataviz/SKILL.md:11-37` | The first actionable step is at :41, after 30 lines of stance and tables. Rubric Axis 5 wants it within ~20 lines. There is no `Done when`. | File read. | Move Procedure to the top. End with "Done when checklist items 0-11 are yes and the brief carries the critique." | 0 |
| F18 | P2 | on-demand | `cas-cli-craft/references/exemplars/before-after.md:17`, `long-running.md:43`; `cas-dataviz/examples/send-backs-dot-strip.html` (4 `cas-xxxx` ids); `cas-cli-craft/references/exemplars/status-screen.md` (1 id) | Operator project names (`gabber-studio`, an operator client) and internal task ids appear in shipped exemplars. This is milder than F1. | grep counts. | Rename to a synthetic project (`acme-web`) and synthetic ids. | 0 |
| F19 | P2 | on-demand | `report-types.md` (304 lines), `svg-web-assets.md` (261), `petrastella-design-language.md` (183), `technical-contract.md` (137), `review-checklist.md` (124), `presentation-rules.md` (117), `providers.md` (105) | References over 100 lines have no contents list (Axis 3). | `wc -l`; the first 15 lines have no TOC. | Add a 3-8 line contents list to each, or split report-types into one file per type. The agent reads only its cell (≈2 KB instead of 20 KB). | report-types split: −4.5 k per html-reports run |
| F20 | P2 (cross-lane, note only) | always | frontmatter of all 9 skills | Top-level `managed_by: cas` (Axis 1: prefer `metadata: {managed_by: cas}`). | — | Handled by the L1/global fix. | 0 |

### P3: polish

| # | Surface | file:line | Defect | Fix |
|---|---|---|---|---|
| F21 | on-demand | `cas-technical-drawing/scripts/draft.mjs:10-11` | The header comment documents `--no-grid`; the actual usage (`:2195`) is `--grid`. The skill uses `--grid` correctly. | Fix the comment. |
| F22 | per-invoke | `cas-technical-drawing/SKILL.md:14,26` | Step 1 gives the full `node <skills-dir>/…/draft.mjs` path; steps 2 and 4 shorten it to `draft.mjs`. Nothing says "run, don't read" for a 123 KB script. | Use the full path each time, plus "Run it; `node draft.mjs --help` lists flags. Do not read the source." |
| F23 | per-invoke | `design-spec/SKILL.md:11` | The only `IMPORTANT:` in the nine skills. | "Use repo-relative paths (e.g. …) so the file stays valid in every checkout." |
| F24 | on-demand | `cas-image-generate/references/svg-web-assets.md:64,91` | The worked examples hard-code Tailwind blue/slate (`#2563eb`, `#f8fafc`). Models copy examples, and the house default forbids a blue-grey default. | Use `var(--verdict)`/`var(--bg)` with Petrastella fallbacks. |
| F25 | on-demand | `cas-image-generate/scripts/generate-image.sh:147` | Uses `v1beta`; current docs show `v1/models/gemini-3.1-flash-image:generateContent`. It still works. | Move to `v1` when F8 lands. |
| F26 | per-invoke | `cas-frontend-engineering/SKILL.md:67-96` | The Playwright table (≈2.3 KB) is needed only at step 7, and the "Target 1.63" pin will age. All APIs are verified current in 1.63.0: stories `mount`, `.webp` baselines, `toHaveCSS({pseudo})`, `getByRole({description})`. | Move to `references/playwright-acceptance.md`; say "1.63+ (stories model)". Δ −580 per invoke. |
| F27 | per-invoke | `cas-ui-craft/SKILL.md:73-78` | The scope boundary does not name `cas-frontend-engineering` as the implementation owner for application screens. | Add one clause. |
| F28 | on-demand | `cas-release-report/references/default-tokens.json` | A byte-identical copy of the Petrastella color and type tokens (verified: 0 differing roles). Unlike the design-spec pair, no parity test pins it. | Add it to the parity test or have `render.py` read the design-spec copy. |

## Items from the 2026-09-02 review: status

| 09-02 item | Status now |
|---|---|
| design-spec removed persona (P0 #15), drift-signal promise (P1 #11), doc-hygiene shared ref (P2) | **fixed** (`:103-113`) |
| cas-dataviz description collides with bundled `dataviz` (P1 #1); H7 precedent; absolute operator path in design-review.md | **fixed**; conflict rule still missing (F16) |
| cas-image-generate description provider-first (P1 #2); dossier link | **fixed** |
| "Imagen retired 2026-08-17" unverifiable | **verified true** (ai.google.dev changelog: Imagen 4 shut down 2026-08-17) |
| cas-image-generate 60 lines of unwired curl | **unfixed** (F15) |
| cas-html-reports stance before first step | **fixed** (workflow at :21) |
| cas-html-reports body restates technical-contract | **unfixed** (F11) |
| review-checklist two-minute version last | **unfixed** (F12) |
| report-types listed under Worked examples | **fixed** |
| De-operator-ise shipped builtins (P1 #8) | **regressed** (F1, F18) |

## Scripts: read-only execution receipts

| Script | Command | Result |
|---|---|---|
| `cas-dataviz/scripts/validate_palette.js` | `node … "#4C5CA8,#9D433B,#0095A0,#693F88,#5C6C00" --surface "#FFFFFF"` and the dark set on `#191C24` (node v24.19.0) | ALL CHECKS PASS, exit 0; the shipped series tokens validate as the tokens file claims |
| `cas-technical-drawing/scripts/draft.mjs` | `node … check examples/shelf-box.json` (in-memory; `check` writes nothing, `:2215-2226`) | ALL CHECKS PASS · 69 findings, 0.1 s; `rsvg-convert` present |
| `cas-release-report/scripts/render.py` | `python3 -B … --help`; `ast.parse` (Python 3.14.4) | parses; CLI flags match SKILL.md :30-35 (`--tokens`, `--emphasis`, plus undocumented `--pdf-href`, `--footer`) |
| `cas-image-generate/scripts/generate-image.sh` | `--help`; `--dry-run` with no key | usage prints; the missing-key path exits 2 before network access, as documented |
| repo `scripts/visual-qa.mjs` | read only | accepts paths (`:564` converts them to file URLs); needs Playwright; writes receipts to `docs/factory/data/visual-qa` by default |

## Currency research (exa-search, 2026-09-25)

- **Gemini image models**: `gemini-3.1-flash-image` (NB2) and `gemini-3-pro-image` (NB Pro) are
  current. `gemini-3.1-flash-lite-image` (NB2 Lite) is new. `imageConfig.aspectRatio` and
  `imageSize` are supported (F8, F15).
- **Imagen 4**: shut down 2026-08-17. The skill is correct.
- **Playwright**: `@playwright/test` 1.63.0 on npm. The stories/gallery `mount` fixture (1.62+),
  WebP screenshots, `toHaveCSS` `pseudo`, `getByRole` `description`, and the deprecation of
  experimental-ct packages all match cas-frontend-engineering exactly.
- **DESIGN.md**: google-labs-code/design.md spec (alpha) plus the `@google/design.md` CLI
  (lint/diff, 0.4.0) (F7).
- **PyMuPDF**: `import pymupdf` is recommended over the legacy name `fitz` (F14).
- **Agent Skills guidance**: taken from the L1 rubric (Axis 1-5). No independent re-research.

## Search manifest

| Command | Hits |
|---|---|
| `find <9 skill dirs> -type f` | 54 files, 788,176 B |
| `diff -rq skills/<s> {codex,grok}/skills/<s>` | 0 diffs ×8 skills; 1 intended (design-spec:56 prefix) |
| `grep -c '"<path>"' builtins.rs` per file ×3 flavours | every file 1/1/1 |
| `grep -o -E 'pippenz\|gabber\|@gmail…'` over exemplars | before/after html 4 identities each; cli-craft 2 files; others 0 |
| `grep -o 'cas-[0-9a-f]{4}'` over exemplars | rubric-review ×12 each; dot-strip 4; status-screen 1; others 0 |
| `grep -E 'IMPORTANT\|CRITICAL\|MUST\|NEVER\|ALWAYS'` over bodies and refs | 1 (design-spec:11) |
| `grep -rn 'release report'` over skills | 0 in cas-release-report (F2) |
| `grep -rln 'default-tokens' src tests` | builtins.rs, release_report.rs, drift test; no parity assertion (F28) |
| `ls scripts/ \| grep qa` (cas-src root) | visual-qa.mjs, terminal-qa.mjs (not shipped, F5) |
| `ls docs/release-reports/*.brief.md` vs report sources | 7 briefs / 29 reports; 22 without (F2) |
| `grep -n 'pie\|KPI card\|stat strip'` across 4 files | 9 conflicting lines (F4) |
| exa-search queries | 6 (Gemini image API ×2, Imagen deprecation, Playwright release notes, DESIGN.md spec ×2, PyMuPDF) |
