# Total ambient recall: bounded retrieval architecture and vector-ingestion economics

**Date:** 2026-08-08  
**Audience:** CAS retrieval and factory practitioners  
**Report contract:** System / architecture explainer  
**Repository commit measured:** `c9668ef5d8c2e8cd10125b189ab9ad8c26cb7947`  
**Confidence:** High for the measured CAS repository receipt and the architecture boundaries; medium for size projections, which scale today's corpus mix linearly.

## 1. System takeaway

CAS can provide useful ambient recall without ambient prompt bloat by keeping the full corpus outside the model context, embedding one bounded query per event, retrieving namespace-scoped candidates across every surface, and injecting only a deduplicated evidence packet with hard role-specific token ceilings. The current CAS history corpus is cheap in resource units: 2,239 eligible commit/doc units required 71 successful embedding requests, about 1.89 MB of input, about 12.33 MB of total wire traffic, 56 seconds without a failure, and 18.64 MB of LMDB allocation. `cas-embed-v1` has no published client monetary price, so this report does not invent one; it gives resource units and formulas.

## 2. End-to-end system flow or loop

<figure class="system-diagram" aria-describedby="recall-flow-summary">
<svg viewBox="0 0 1080 360" role="img" aria-labelledby="recall-flow-title recall-flow-desc" preserveAspectRatio="xMidYMid meet">
<title id="recall-flow-title">Bounded ambient recall and silent maintenance loop</title>
<desc id="recall-flow-desc">A current prompt, task, role, and repository become one bounded query embedding. Planned namespace-isolated retrieval, fusion, conflict handling, novelty ledger, and token budget produce a compact evidence packet. A separate planned maintenance loop hashes changes, queues scoped units, embeds bounded batches, and switches model generations safely.</desc>
<defs><marker id="flow-arrow" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto"><path d="M0,0 L9,4.5 L0,9z"/></marker></defs>
<g class="current"><rect x="20" y="45" width="180" height="64" rx="8"/><text x="110" y="72" text-anchor="middle">Prompt + task + role</text><text x="110" y="94" text-anchor="middle">CURRENT INPUT</text></g>
<g class="planned"><rect x="265" y="45" width="165" height="64" rx="8"/><text x="348" y="72" text-anchor="middle">Canonical query</text><text x="348" y="94" text-anchor="middle">PLANNED</text></g>
<g class="planned"><rect x="495" y="45" width="165" height="64" rx="8"/><text x="578" y="72" text-anchor="middle">One embedding +</text><text x="578" y="94" text-anchor="middle">scoped fan-out</text></g>
<g class="planned"><rect x="725" y="45" width="150" height="64" rx="8"/><text x="800" y="72" text-anchor="middle">Fuse + ledger</text><text x="800" y="94" text-anchor="middle">+ hard budget</text></g>
<g class="planned"><rect x="935" y="45" width="125" height="64" rx="8"/><text x="998" y="72" text-anchor="middle">Evidence</text><text x="998" y="94" text-anchor="middle">packet</text></g>
<path class="arrow" d="M200,77 H257"/><text x="229" y="64" text-anchor="middle">normalize</text>
<path class="arrow planned-line" d="M430,77 H487"/><text x="459" y="64" text-anchor="middle">embed once</text>
<path class="arrow planned-line" d="M660,77 H717"/><text x="689" y="64" text-anchor="middle">candidates</text>
<path class="arrow planned-line" d="M875,77 H927"/><text x="901" y="64" text-anchor="middle">bounded</text>
<g class="current"><rect x="20" y="230" width="180" height="64" rx="8"/><text x="110" y="257" text-anchor="middle">Content change</text><text x="110" y="279" text-anchor="middle">CURRENT SOURCES</text></g>
<g class="planned"><rect x="265" y="230" width="165" height="64" rx="8"/><text x="348" y="257" text-anchor="middle">Hash + scoped</text><text x="348" y="279" text-anchor="middle">pending queue</text></g>
<g class="current"><rect x="495" y="230" width="165" height="64" rx="8"/><text x="578" y="257" text-anchor="middle">Bounded batches</text><text x="578" y="279" text-anchor="middle">CURRENT CORE</text></g>
<g class="planned"><rect x="725" y="230" width="150" height="64" rx="8"/><text x="800" y="257" text-anchor="middle">Shadow model</text><text x="800" y="279" text-anchor="middle">generation</text></g>
<path class="arrow planned-line" d="M200,262 H257"/><text x="229" y="249" text-anchor="middle">changed hash</text>
<path class="arrow planned-line" d="M430,262 H487"/><text x="459" y="249" text-anchor="middle">due units</text>
<path class="arrow planned-line" d="M660,262 H717"/><text x="689" y="249" text-anchor="middle">validated vectors</text>
<path class="loop planned-line" d="M800,294 C800,340 348,340 348,302"/><text x="578" y="334" text-anchor="middle">retry/backoff, freshness SLO, atomic generation switch</text>
</svg>
<figcaption id="recall-flow-summary"><strong>Current:</strong> authoritative inputs and the bounded embedding core. <strong>Planned (dashed):</strong> one-query orchestration, scoped fusion, ledger/budget injection, hash-aware scheduling, and shadow-generation rebuilds. Full bodies remain outside the packet until explicitly justified.</figcaption>
</figure>

```text
CURRENT EVENT                         PLANNED AMBIENT RECALL (outside prompt)                 BOUNDED MODEL INPUT
prompt + task + role + repo  ->  canonical query text  ->  one query embedding  ->  namespace-isolated fan-out
                                                                                         | lexical
                                                                                         | semantic
                                                                                         | structural/entity
                                                                                         | temporal
                                                                                         v
                                                         provenance + conflict handling -> fused candidates
                                                                                         |
                                                         novelty + session recall ledger -> compact snippets/IDs
                                                                                         |
                                                         dynamic role budget + hard cap  -> evidence packet
                                                                                         |
model response  <-  explicit full-body pull when justified  <-  worker/supervisor context (normally snippets only)

SILENT MAINTENANCE LOOP
content change -> content hash -> scoped pending queue -> idle-preferred bounded batches -> vector/cache generation
      ^                                                                                         |
      +----------- retry/backoff + freshness SLO + model/dimension generation switch ---------+
```

Every arrow is a boundary: raw corpus bodies do not enter the model merely because they were indexed; namespace and scope checks happen before ranking; first-stage results are IDs, compact snippets, scores, and provenance; full bodies require a second, justified pull.

Recommended hard defaults:

| Control | Worker default / ceiling | Supervisor default / ceiling | Hard behavior |
| --- | ---: | ---: | --- |
| Injected recall packet | 1,200 / 2,000 tokens | 1,800 / 3,000 tokens | Truncate by marginal utility, never by arbitrary body slicing |
| Emergency absolute ceiling | 4,000 tokens | 5,000 tokens | Refuse additional injection and expose omitted candidate count |
| First-stage candidates | 48 fused / 8 injected | 72 fused / 12 injected | IDs + snippets + provenance only |
| Query-construction budget | 512 tokens | 768 tokens | Prompt is summarized locally; secrets and quoted bulk are excluded |
| Full-body pulls | 2 per event | 3 per event | Only when a snippet cannot support the task and relevance clears 0.85 |
| Packet target p50 / p95 / p99 | 450 / 1,200 / 2,000 tokens | 650 / 1,800 / 3,000 tokens | Measured per recall event |
| Repeated-injection rate | <= 10% | <= 10% | Unchanged evidence is referenced through the session ledger, not resent |

Any implementation capable of turning an ordinary query into a 300,000-token packet fails the contract by construction.

## 3. Inputs and outputs

### 3.1 Automatic query input

The query builder creates one stable text record from:

1. current user prompt, stripped of quoted bulk and secrets;
2. active task title, description, acceptance criteria, labels, dependencies, and declared paths;
3. role (`worker` or `supervisor`) and its retrieval policy;
4. repository, branch, task/epic IDs, and recently named symbols/entities;
5. unresolved questions and failures from the last few turns; and
6. the session recall ledger, represented as already-seen evidence IDs rather than repeated prose.

One query embedding is shared by all semantic namespaces. Lexical, structural, temporal, and entity channels use the same normalized query record without additional embedding calls.

### 3.2 Retrieval surfaces and outputs

| Surface | Namespace and scope gate | First-stage output | Full-body pull condition |
| --- | --- | --- | --- |
| Scoped memories | `global`, project, team, validity window | ID, title, compressed fact, confidence, validity, provenance | Fact is top-ranked and exact wording/evidence is necessary |
| Persona and pinned guidance | role/harness/project, explicit priority | rule ID, one-line instruction, source and priority | Conflicting instructions or a procedure must be followed exactly |
| Knowledge pages | project/team/origin, locked/current revision | page ID, title, snippet, source paths, revision hash | Page answers the active question and snippet is insufficient |
| Tasks, rules, skills, specs | assignment/epic/project/status | ID, title, state, dependency/priority, short match | Acceptance criteria, procedure, or decision body is directly required |
| Git history and provenance | repository/branch/time/task/session | SHA, subject/snippet, paths/symbols, provenance confidence | Diff or full commit body is required to prove a claim |
| Source-code chunks | repository/revision/path/language | symbol/chunk ID, signature, path/lines, short excerpt | A concrete edit/review needs the implementation body |

The injected packet is structured evidence, not prose soup: `{evidence_id, surface, snippet, why_relevant, provenance, freshness, conflicts, body_available}`. Omitted candidates are counted so a small packet never masquerades as exhaustive recall.

### 3.3 Role-specific behavior

| Need | Worker policy | Supervisor policy |
| --- | --- | --- |
| Task focus | Binding acceptance criteria, path-local rules, relevant prior fixes, symbols and recent commits | Child task state, dependencies, integration risks, cross-task decisions and quality gates |
| Breadth | Narrow: active task and owned layer first | Broad: epic and all active lanes, but snippets before bodies |
| History | Recent changes to touched paths/symbols and failures resembling the current one | Merge conflicts, cross-lane interfaces, regressions, verification and provenance coverage |
| Guidance | Exact applicable rule/skill, highest priority only | Coordination policy, escalation rules, epic decisions and delivery state |
| Suppression | Unrelated epic chatter, stale closed-task narration, unchanged evidence | Function bodies and detailed logs unless they explain a risk or failed gate |

## 4. Components and layers

### 4.1 Query builder

Builds a canonical, redacted query record and one embedding. It emits stable entity and constraint fields alongside free text so filters are not reverse-engineered from an opaque vector.

### 4.2 Namespace-isolated candidate retrieval

Each surface is queried inside an allowed namespace. Authorization and project/team scope are part of the lookup key or SQL predicate, never a post-retrieval filter. Each surface returns up to 20 lexical, 20 semantic, and 20 structural/entity candidates where those channels exist. Capability-absent semantic retrieval contributes no zero vectors; its weight is redistributed to live channels.

### 4.3 Fusion and conflict handling

Use reciprocal-rank fusion as the deterministic baseline, then add bounded feature bonuses:

- exact task/path/symbol/entity match;
- approved or binding source status;
- temporal fitness and explicit validity windows;
- provenance confidence and direct observation; and
- role fitness.

Semantic similarity must not erase an exact lexical or structural match. Source precedence is explicit: current binding task/approved spec > current project rule or decision > current knowledge/memory > historical inference. Conflicting current sources survive as a paired warning; the system never silently averages them into a false consensus. Stale or superseded evidence may explain history but is labeled and cannot outrank a current binding source.

### 4.4 Novelty, deduplication, and recall ledger

Deduplicate by canonical evidence ID + revision hash, then near-deduplicate snippets by content hash. The session ledger records what revision was already injected, why, and at which turn. Later events receive only new evidence, changed revisions, new conflicts, or a short reference such as “task AC already in context.” The ledger is session-scoped, bounded, and disposable; it is not a new source of truth.

### 4.5 Dynamic budgeter and body puller

Allocate the packet in this order: binding constraints; direct task evidence; conflict/staleness warnings; high-utility background; diversity reserve. Stop when marginal utility falls below 0.15 or the role ceiling is reached. Full bodies are a separate retrieval operation. A large body is excerpted around the matched span with provenance rather than injected whole.

### 4.6 Silent maintenance scheduler

Every embeddable row carries a content hash, model generation, scope namespace, pending state, attempts, next-attempt time, and last error. Writes enqueue only when embedded text changes. The scheduler prefers idle periods but obeys a max-staleness ceiling.

| Surface | Target freshness | Maximum staleness | Trigger |
| --- | ---: | ---: | --- |
| Tasks, rules, pinned/persona guidance | synchronous or < 5 s | 5 min repair pass | write event |
| Memories and knowledge pages | p95 <= 5 min | 15 min | content-hash change |
| Git history and source chunks | p95 <= 10 min | 30 min while active | HEAD/revision change plus periodic repair |
| GitHub/remote docs | p95 <= 30 min | 2 h | cursor poll and changed-text hash |

Recommended batch budget is the current 32 inputs/request and 512 units/tick, but cap a tick at 60 requests (50% of the published 120 requests/60 s service limit) so interactive query embeddings retain headroom. Retry transient network, 429, 5xx, and timeout failures at 1 min, 5 min, 15 min, 1 h, then 6 h with jitter. Honor `Retry-After`. Treat authentication and malformed payloads as blocked/operator errors, and 404/501 as capability absent. A failed batch advances no queue item; successfully cached siblings remain complete.

Model or dimension changes use a shadow generation: keep the old complete generation serving, build the new namespace in bounded batches, validate dimensions/non-zero vectors and coverage, then atomically switch when pending is zero and evaluation gates pass. Only then garbage-collect the old generation. This avoids the current cache-wipe window.

## 5. Data lifecycle

### 5.1 Creation and validation

Source writes calculate canonical embedded text and its hash. Empty text and generated merge subjects are explicitly excluded. Inputs are validated for scope, size, encoding, and secret policy before queueing. Vectors are rejected if zero, malformed, or dimension-mismatched.

### 5.2 Storage and retrieval

Structured source data remains authoritative in SQLite/files/git. Vectors are a rebuildable local cache keyed by namespace and source ID. Candidate retrieval reads compact metadata first; body storage is never copied into the prompt by default. Provenance travels with every candidate and packet item.

### 5.3 Retention and deletion

Deleting or de-scoping a source synchronously tombstones its lexical, structural, and vector records. Team/project reassignment cannot leave a vector visible in the former namespace. Session ledgers expire with the session. Old model generations are removed only after the replacement is complete and serving.

### 5.4 Fresh CAS benchmark

The benchmark used a newly initialized isolated CAS root inside this worktree, the installed `cas 2.53.0` client, current repository commit `c9668ef5`, CHANGELOG-only docs (no network fetch), and the already-configured embedding capability. The live project database and vector cache were not read or mutated for benchmark writes.

| Metric | Prior M7 receipt (2026-08-08) | Fresh measurement | Change |
| --- | ---: | ---: | ---: |
| Indexed commits | 2,477 | 2,577 | +100 (+4.0%) |
| Indexed docs | 53 | 54 | +1 (+1.9%) |
| Generated merge messages skipped | 358 | 392 | +34 (+9.5%) |
| Eligible embedded units | 2,172 | 2,239 | +67 (+3.1%) |
| Successful embedding requests | 71 | 71 | 0 |
| Input bytes | not recorded | 1,892,492 | measured fresh |
| No-failure embedding time | 56 s | about 56 s | corroborated by prior receipt and fresh retry math |
| Failure-inclusive time | not recorded | 85.09 s | one real 503 + retry |
| Raw vector bytes (2,239 x 1,024 x f32) | about 8.90 MB | 9,170,944 | +3.1% units |
| LMDB allocation | about 19 MB | 18,636,800 | 2.032x raw |
| SQLite structural store | not recorded | 10,432,512 | fresh isolated root |
| Downstream TLS bytes | not recorded | 10,375,671 | syscall-counted across initial + retry |
| Modeled full upstream TLS bytes | not recorded | about 1,954,434 | measured 1.033x on retry applied to all input |
| Total wire bytes | not recorded | about 12,330,000 | downstream measured + upstream modeled |

The 2,239 eligible units split into 2,185 commits and 54 docs. The provider returned HTTP 503 on attempted request 64 after 63 successful batches. The drain left 223 units pending and preserved all 2,016 successful vectors. A second invocation made 8 requests, embedded the remaining 223 units in 6.85 s, and drained pending to zero. Total attempted requests were 72 for 71 successful batches. The failure cost was one extra request plus about 29 s compared with the 56 s prior no-failure receipt; no successful work was repeated.

The endpoint does not expose billed token counts. Measured bytes are therefore the authoritative input unit. A planning-only token range of 3–5 UTF-8 bytes/token puts the fresh corpus at roughly 378,000–631,000 tokens; this range is not a billing receipt.

### 5.5 Representative project projections

Projection assumptions: current eligible-commit ratio (84.79%), 788 input bytes/eligible commit, 3,160 input bytes/doc, 1,024 f32 dimensions, measured 2.032 LMDB amplification, 4,634 downstream TLS bytes/unit, 1.033 upstream TLS bytes/input byte, 0.789 s/request, and linear SQLite scaling. “Small” is 500 commits + 10 docs; “medium” is the measured CAS repository; “large” is 100,000 commits + 2,000 docs. These are capacity scenarios, not claims about a population distribution.

| Scenario | Total commits / docs | Eligible units | Input bytes | Requests | Embed time | Raw vectors | LMDB | Total wire | SQLite + LMDB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Small (estimate) | 500 / 10 | 434 | 365,726 | 15 | 11.8 s | 1.78 MB | 3.61 MB | 2.39 MB | about 5.64 MB |
| Medium (measured) | 2,577 / 54 | 2,239 | 1,892,492 | 71 | 56.0 s | 9.17 MB | 18.64 MB | 12.33 MB | 29.07 MB |
| Large (estimate) | 100,000 / 2,000 | 86,789 | 73,136,549 | 2,713 | 35.7 min | 355.49 MB | 722.41 MB | 477.71 MB | about 1.13 GB |

At the 120 request/minute service ceiling, the large backfill has a theoretical floor of 22.6 minutes; measured request latency produces the higher 35.7-minute estimate. Structural indexing measured 2.33 s for medium and projects to about 90 s for large, so embeddings dominate elapsed time.

### 5.6 Incremental monthly economics

The activity scenarios are explicit: small 50 commits + 2 changed docs/month; medium 500 + 20; large 5,000 + 200. Unchanged docs cost nothing because content hashes do not re-arm the queue.

| Scenario | Eligible new units/month | Input bytes/month | Requests/month | Embed time/month | Added LMDB/month | Total wire/month | Approx. total local growth/month |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Small | 44 | 39,417 | 3 | 2.4 s | 0.37 MB | 0.24 MB | 0.57 MB |
| Medium | 444 | 397,324 | 15 | 11.8 s | 3.70 MB | 2.47 MB | 5.72 MB |
| Large | 4,439 | 3,972,449 | 140 | 110.4 s | 36.95 MB | 24.67 MB | 57.19 MB |

A model/dimension rebuild costs the vector portion of initial backfill again: the same eligible units, input, requests, wire traffic, and new-generation LMDB allocation. Structural SQLite indexing is retained. During a safe shadow rebuild, peak vector storage is approximately old LMDB + new LMDB: 7.22 MB small, 37.27 MB medium, and 1.44 GB large, before the old generation is collected.

`cas-embed-v1` monetary pricing is unavailable in the repository contracts and client receipts. Let `P` be provider dollars per million input tokens, `Q` any per-request charge, `T` the provider-reported input-token count, and `R` requests:

```text
initial or rebuild dollars = (T / 1,000,000) * P + R * Q
incremental monthly dollars = (T_month / 1,000,000) * P + R_month * Q
one query-embedding dollars = (T_query / 1,000,000) * P + Q
monthly query dollars = events_month * one_query_embedding_dollars
```

Until the provider publishes `P`, `Q`, and billed `T`, CAS should report measured bytes, units, requests, elapsed time, wire bytes, and storage rather than a dollar figure. A typical 512-byte canonical query is roughly 102–171 planning tokens at 3–5 bytes/token, produces one 4,096-byte raw query vector, and should be cached by query hash within the session.

## 6. Annotated walkthrough

Representative worker event: “Fix the stale history doctor warning in task cas-X; tests fail in `history::status`.”

1. **Construct:** The builder combines that prompt with cas-X acceptance criteria, worker role, repository/branch, declared files, and the last failure. It excludes already-injected unchanged task prose using the ledger.
2. **Embed once:** The canonical record is embedded once. The vector is shared across scoped memory, knowledge, task, history, and source-code semantic namespaces.
3. **Fan out:** Lexical retrieval finds the exact doctor/status terms; structural retrieval finds `history::status` and touched paths; semantic retrieval finds a prior stale-watermark fix; task retrieval finds the binding AC.
4. **Fuse:** Exact task and symbol matches lead. A superseded decision is retained only as labeled historical context. An unrelated high-cosine memory from another project never enters the candidate set because namespace filtering preceded search.
5. **Budget:** The worker receives the AC snippet, applicable rule, symbol/path snippet, one prior-fix commit, and a freshness warning—about 600 tokens, below the 1,200-token default.
6. **Pull:** If the worker needs the prior patch, it explicitly pulls that commit diff. The diff was not injected speculatively.
7. **Ledger:** Evidence IDs and revisions are recorded. The next turn gets only a changed test result or new conflict, not the same five snippets again.
8. **Maintain:** A later commit changes the source chunk and git history. Content hashes enqueue only those changed units; the scheduler embeds them within the active-project freshness SLO.

## 7. Invariants and failure boundaries

| Invariant / boundary | Required behavior | Observable proof |
| --- | --- | --- |
| No ambient prompt bloat | Default and hard packet ceilings always apply; bodies are second-stage | p50/p95/p99 packet tokens and omitted-candidate count |
| Scope before similarity | Project/team/role namespace is part of retrieval | Cross-project and cross-team negative tests return zero forbidden IDs |
| One query embedding | Semantic surfaces share one event vector | request counter equals semantic recall events on cache misses |
| Capability absent | Lexical/structural retrieval continues; no fake/zero vector or empty LMDB | status declares semantic absent and redistributed weights |
| Freshness is explicit | Every item carries revision, observed time, validity/supersession | stale-injection rate and per-surface queue age |
| Conflicts are not flattened | Contradictory current sources are paired and labeled | conflict fixtures preserve both sources and precedence reason |
| Failed batch advances nothing | Successful earlier batches remain; failed units stay pending | fault injection + queue receipt |
| Model spaces never mix | Generation/model/dim are part of cache identity | dimension/model mismatch forces shadow rebuild, not mixed search |
| Deletion is prompt-visible immediately | Tombstone all channels synchronously | deleted-source canary is unretrievable before next model event |
| Ledger is an optimization only | Losing it may repeat context but cannot lose source truth | restart test reconstructs recall from authoritative stores |

The architecture does not guarantee exhaustive recall, truth of source content, or zero latency when the cloud embedding capability is cold/unavailable. It guarantees bounded context, explicit provenance and omissions, safe degradation, and measurable quality.

### 7.1 Evaluation set and gates

Create a versioned, labeled 160-event set: 80 worker and 80 supervisor events; 20 primary-relevance events for each of the six retrieval surfaces plus 20 conflict/staleness cases and 20 “no injection warranted” negatives. Each event labels relevant evidence IDs, preferred top evidence, forbidden cross-scope IDs, stale/superseded IDs, and an ideal maximum packet. Freeze train/tuning and held-out partitions by repository revision.

| Metric | Worker gate | Supervisor gate | Measurement |
| --- | ---: | ---: | --- |
| Recall@8 / Recall@12 | >= 0.85 | >= 0.90 | labeled relevant IDs retrieved in role candidate cap |
| nDCG@8 / nDCG@12 | >= 0.75 | >= 0.80 | graded relevance and binding-source preference |
| MRR | >= 0.70 | >= 0.75 | first preferred evidence rank |
| Useful-context rate | >= 0.75 | >= 0.75 | human/evaluator marks injected item useful; denominator is all injected items |
| Harmful or stale injection rate | <= 1.0% | <= 1.0% | harmful, superseded-as-current, or scope-wrong items / injected items |
| Cross-scope leakage | 0 | 0 | forbidden IDs returned or injected |
| Cold latency p95 | <= 1.5 s | <= 1.5 s | includes one remote query embedding |
| Warm latency p95 | <= 250 ms | <= 300 ms | cached query embedding, local retrieval/fusion only |
| Packet tokens p50 / p95 / p99 | <= 450 / 1,200 / 2,000 | <= 650 / 1,800 / 3,000 | tokenizer at final injection boundary |
| Repeated-injection rate | <= 10% | <= 10% | unchanged evidence revision injected again in session |

Compare four ablations on the identical set: lexical-only; lexical + semantic; lexical + semantic + structural/temporal/entity fusion; and full fusion + ledger/budgeter. Ship only if the full system improves recall/nDCG without breaching harmful-injection, leakage, latency, or prompt-budget gates. Record p50/p95/p99, not means alone.

## 8. Component map

<figure class="system-diagram" aria-describedby="component-map-summary">
<svg viewBox="0 0 1080 430" role="img" aria-labelledby="component-map-title component-map-desc" preserveAspectRatio="xMidYMid meet">
<title id="component-map-title">Ambient recall component ownership map</title>
<desc id="component-map-desc">Current authoritative source stores feed current and planned indexes. A planned recall orchestrator produces bounded evidence for planned worker and supervisor role adapters. Planned observability measures freshness, cost, quality, token size, and leakage across every layer.</desc>
<defs><marker id="map-arrow" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto"><path d="M0,0 L9,4.5 L0,9z"/></marker></defs>
<g class="layer current"><rect x="25" y="25" width="1030" height="76" rx="8"/><text x="45" y="51">AUTHORITATIVE SOURCES — CURRENT</text><text x="45" y="79">memories/tasks/rules/skills · knowledge pages · git/provenance · source tree</text></g>
<g class="layer mixed"><rect x="25" y="132" width="1030" height="76" rx="8"/><text x="45" y="158">INDEX &amp; MAINTENANCE — CURRENT CORE + PLANNED UNIFICATION</text><text x="45" y="186">lexical · structural/entity · namespaced vector generations · pending queues/ledgers</text></g>
<g class="layer planned"><rect x="25" y="239" width="1030" height="76" rx="8"/><text x="45" y="265">RECALL ORCHESTRATOR — PLANNED</text><text x="45" y="293">query builder → scoped fan-out → fusion/conflicts → novelty ledger → hard budget</text></g>
<g class="planned"><rect x="135" y="346" width="320" height="58" rx="8"/><text x="295" y="381" text-anchor="middle">Worker role adapter</text></g>
<g class="planned"><rect x="625" y="346" width="320" height="58" rx="8"/><text x="785" y="381" text-anchor="middle">Supervisor role adapter</text></g>
<path class="arrow" d="M540,101 V124"/><text x="555" y="119">IDs + revisions + scope</text>
<path class="arrow planned-line" d="M540,208 V231"/><text x="555" y="226">compact candidates</text>
<path class="arrow planned-line" d="M430,315 L340,340"/><path class="arrow planned-line" d="M650,315 L740,340"/>
</svg>
<figcaption id="component-map-summary">Source stores own truth and scope; indexes own rebuildable representations; the planned orchestrator owns selection and bounds; planned role adapters own relevance policy. Solid outlines are current, dashed outlines are planned, and the mixed index layer is explicitly labeled.</figcaption>
</figure>

```text
AUTHORITATIVE SOURCES (current)
  SQLite memories/tasks/rules/skills/specs | knowledge pages/files | git + provenance | source tree
                  | content IDs, scope, revision, validity, provenance
                  v
INDEX & MAINTENANCE (partly current, ambient unification planned)
  lexical index | structural/entity index | namespaced vector generations | pending queues/ledgers
                  | compact candidates only
                  v
RECALL ORCHESTRATOR (planned)
  query builder -> namespace fan-out -> rank fusion -> conflict/staleness -> novelty ledger -> budgeter
                  | bounded evidence packet; optional justified body pull
                  v
ROLE ADAPTERS (planned)
  worker policy                                         supervisor policy
                  |                                          |
                  +---------------- model context ------------+

OBSERVABILITY (planned across the loop)
  freshness/queue age | request/input/wire/storage cost | retrieval quality | packet tokens | leakage
```

Ownership boundaries are deliberate: source stores own truth and scope; indexers own rebuildable representations; the recall orchestrator owns candidate selection and packet bounds; role adapters own relevance policy; the model receives evidence but never becomes a persistence layer.

## 9. Current versus planned boundary

### Current and measured

- Structural git history, docs, provenance, lexical/semantic history search, merge exclusion, pending embedding queues, 32-input chunking, 120 request/minute limiter, LMDB namespaces, failure ledger, capability-absent behavior, and model metadata exist on the measured branch.
- Knowledge pages already have lexical + semantic retrieval and capability-honest weight redistribution.
- Task/rule/skill/memory/code search surfaces exist, but are invoked explicitly and do not yet share one ambient orchestrator or injection budget.
- Current model-change behavior invalidates the vector cache and re-arms rows. It is correct about incompatible spaces but creates a rebuild window; the shadow-generation design is planned.

### Planned mergeable implementation slices

| Slice | Depends on | Deliverable | Effort estimate | Cost gate | Quality gate |
| --- | --- | --- | ---: | --- | --- |
| A. Recall event + query builder | none | redacted canonical query, role/task/entity fields, one-embedding cache | 2.0 engineer-days | exactly one embedding request/event cache miss; <= 768 query tokens | deterministic fixtures; secret/quoted-bulk exclusion |
| B. Scoped retrieval adapters | A | common candidate schema for all six surfaces; namespace-first filters | 4.0 days | <= 20 candidates/channel/surface; no body loads | zero cross-scope leakage; surface contract tests |
| C. Fusion + provenance/conflicts | B | RRF baseline, structural/temporal/entity bonuses, precedence and conflict pairs | 3.0 days | local fusion p95 <= 50 ms for 500 candidates | nDCG/MRR gates; no semantic override of binding exact match |
| D. Packet budget + body pull | C | role ceilings, marginal-utility stop, omission counts, justified excerpt pull | 3.0 days | worker/supervisor p99 <= 2,000/3,000 default targets; hard 4,000/5,000 | 300k-token adversarial corpus remains below hard ceiling |
| E. Session recall ledger | A, D | evidence revision cache, delta injection, bounded expiry | 2.0 days | <= 1 MB/session and O(1) evidence lookup | repeated injection <= 10%; restart loses no truth |
| F. Maintenance generations | B | content hashes, queue metadata, backoff, per-surface SLO, shadow model switch | 4.0 days | 60 requests/tick; incremental units only; reported bytes/storage | fault injection, zero-vector/dim rejection, atomic generation switch |
| G. Evaluation + telemetry | B–F | 160 labeled events, ablations, latency/token/cost/freshness histograms | 4.0 days | benchmark runs without provider calls when vectors are recorded | all retrieval, harmful/stale, latency and packet gates reported |
| H. Role rollout | A–G | worker then supervisor adapters behind config, silent/shadow mode first | 3.0 days | default packet budgets above; kill switch | 1-week shadow telemetry, then canary gates before default-on |

Do not combine these into one feature branch. Each slice has a falsifiable gate and can merge behind an off/shadow flag. Worker rollout comes first because its scope is narrower; supervisor rollout follows only after cross-task breadth does not breach prompt or harmful-injection gates.

### Explicitly not included

- No unbounded “load everything relevant” mode.
- No vector-only authorization or post-search scope filtering.
- No automatic full diff/page/file injection.
- No invented dollar price for `cas-embed-v1`.
- No claim that planned ambient orchestration or shadow generations are currently operating.

## 10. Evidence and provenance

### Reproduction commands

The benchmark root was a private temporary directory under the worktree. Replace `<bench-root>` below with a new empty directory; do not target a live `.cas` store.

```bash
CAS_ROOT=<bench-root> cas init -y --no-integrations
CAS_ROOT=<bench-root> /usr/bin/time -v cas history backfill --no-symbols
CAS_ROOT=<bench-root> /usr/bin/time -v cas history docs --changelog
sqlite3 -readonly <bench-root>/cas.db '<the count/byte queries below>'
CAS_ROOT=<bench-root> /usr/bin/time -v strace -f -yy -s 32 \
  -e trace=%network,read,write,readv,writev \
  -o /tmp/cas-embed.strace cas history embed --limit 5000 --json
```

Count and input-byte query:

```sql
SELECT count(*) AS commits,
       sum(CASE WHEN ltrim(subject) LIKE 'Merge branch%'
                 OR ltrim(subject) LIKE 'Merge pull request%' THEN 1 ELSE 0 END) AS skipped,
       sum(CASE WHEN NOT (ltrim(subject) LIKE 'Merge branch%'
                          OR ltrim(subject) LIKE 'Merge pull request%') THEN 1 ELSE 0 END) AS eligible,
       sum(CASE WHEN NOT (ltrim(subject) LIKE 'Merge branch%'
                          OR ltrim(subject) LIKE 'Merge pull request%')
                THEN length(CAST(subject || CASE WHEN body IS NOT NULL AND trim(body) <> ''
                     THEN char(10) || trim(body) ELSE '' END AS blob)) ELSE 0 END) AS input_bytes
FROM history_commits;
```

Socket-byte receipts were summed from positive syscall return values on TCP file descriptors. The initial trace omitted `writev`, so its downstream bytes are measured but full upstream bytes are modeled from the retry: 398,722 TLS bytes for 386,106 input bytes (1.03267x), applied to 1,892,492 total input bytes. LMDB amplification is `18,636,800 / (2,239 * 1,024 * 4) = 2.03216`.

### Source evidence

- `cas-cli/src/cloud/embeddings.rs`: model/dimension identity, 32-input request cap, vector validation, cache reindex behavior.
- `cas-cli/src/cloud/embed_drain.rs`: shared knowledge/history drain, 512-unit tick, capability-absent boundary, generated-merge exclusion, failure ledger.
- `crates/cas-store/src/history_store.rs`: pending queues, oldest-first order, content-change re-arming, model-change re-arm.
- `docs/specs/2026-08-07-code-history-search.md`: published 120 requests/60 s service limit and privacy/provider contract.
- CAS task `cas-db6e`: prior live receipt used only as the named comparison baseline.

### Raw receipt summary

```text
commit: c9668ef5d8c2e8cd10125b189ab9ad8c26cb7947
git rev-list --count HEAD: 2577
structural backfill: 2577 commits, 9763 file changes, 6 chunks, 2.33 s
CHANGELOG docs: 54, 0.03 s
queued: 2631; excluded generated merges: 392; eligible: 2239
embedding input bytes: 1892492
first run: 2016 embedded, 392 skipped, 64 attempted requests, 223 pending, HTTP 503, 78.24 s
retry: 223 embedded, 8 requests, 0 pending, 6.85 s
successful requests: 71; attempted requests: 72
downstream TLS: 10375671 bytes
final LMDB data.mdb: 18636800 bytes; raw vectors: 9170944 bytes
SQLite cas.db: 10432512 bytes
```

The temporary benchmark root and copied authentication file are removed after the committed report is verified. The Markdown file is the source of truth; the self-contained HTML beside it is the human review surface.
