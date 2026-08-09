# CAS Memory: the learning loop for every agent

**Product showcase · refreshed 9 August 2026 · implementation reference `85963767`**

CAS turns the useful signal left by work into a reusable, inspectable asset. The product is not a bigger transcript: it is a controlled loop that captures what matters, gives it durable shape, retrieves the right part later, and lets the next task start with more context than the last one.

> **Work becomes memory; memory changes the next piece of work.**

This is an implementation-grounded product explainer. It describes what the current checkout supports, separates local capability from cloud-gated semantic capability, and links the public source that supports each claim. It does not claim adoption, quality, or an endorsement by Tencent or GitHub.

## The memory loop

```text
 WORK SIGNALS
 conversations · tasks · code · docs · git history
        │
        ▼
 CAPTURE ──► CLASSIFY & SCOPE ──► PERSIST
 observations       type, importance,       SQLite records + Markdown knowledge
 learnings          validity, lifecycle     source lineage + locks
 preferences        project / host-global /
 context            team-shared boundaries
        │                                      │
        └─────────────── FEEDBACK ◄────────────┘
                            ▲
                            │
 REUSE ◄── RECALL & INJECT ◄── HYBRID FIND & RANK ◄── DISTILL / VECTORIZE
 next task      compact orientation +          lexical / structural /             knowledge pages,
 next session   focused detail on demand        temporal / entity / semantic        history, code symbols
```

The loop is deliberately selective. CAS can provide a compact session orientation and pinned guidance, then let the agent search and open only the detail needed for the task. A useful outcome, correction, or preference can be captured again, improving the next pass rather than vanishing into a transcript.

## The memory surfaces, mapped to the loop

| Surface | Enters or supports the loop | What it contributes |
| --- | --- | --- |
| **Working/session context** | Recall & inject | A bounded orientation for the active session; detail stays available through tools rather than being loaded wholesale. |
| **Durable entries** | Capture → persist | Atomic `learning`, `preference`, `context`, and `observation` records, with importance, tags, lifecycle, and validity controls. |
| **Pinned/persona guidance** | Recall & inject | Always-active, high-priority guidance for the session start path. |
| **Scope boundaries** | Classify & scope | Project memory stays with the project; host-global material can be reused locally; team sharing is optional and explicit. |
| **Distilled knowledge pages** | Distill → persist | A source-linked, human-readable Markdown wiki; pages can be locked against automated overwrite. |
| **Tasks, rules, and skills** | Recall & inject | Current work, normative constraints, and procedural playbooks that make memory actionable. |
| **Git history and provenance** | Work signal → recall | Searchable commits and associated provenance help answer what changed and why. |
| **Source-code symbols** | Work signal → hybrid recall | tree-sitter structural symbols and local BM25 always provide source lookup once indexed; semantic source vectors enrich that path when the cloud capability is live. |

## How vectorization works

```text
 A changed knowledge page, history item, or eligible code symbol
                       │
                       ▼
               pending_embedding queue
                       │  prepare text/chunk
                       ▼
          cas-cloud /api/embeddings request
          model: cas-embed-v1 · default: 1024 dimensions
                       │  ≤32 inputs/request; batch budget + rate limiter
                       ▼
    validate dimension ── reject all-zero vectors ── leave failed work retryable
                       │
                       ▼
       LMDB vector caches with separated namespaces
       knowledge + history: index/knowledge-vectors
       source symbols:     index/code-vectors
                       │  persist {provider, model, dims}
                       ▼
      model/dimension change? clear stale cache → re-mark corpus pending → rebuild safely
                       │
                       ▼
 query vector + lexical / structural / temporal / entity candidates
                       │
                       ▼
          hybrid ranking → focused recall → injection → reuse
```

### Current capability boundary

- **Knowledge and history vectorization are implemented.** Their pending queues drain through the same cloud embedding client and local cache, while the durable pages, history ledger, and local lexical retrieval remain useful offline.
- **Source-code vectorization is also implemented.** The tree-sitter indexer queues eligible symbols; their identity, path, documentation, signature, and bounded source context form the embedding text. Vectors live in an isolated `index/code-vectors` LMDB environment, separate from knowledge/history.
- **Semantic retrieval is capability-gated.** It becomes live only with cloud authentication and a non-empty, model-compatible local cache. When those conditions are absent—or a query embedding fails—CAS retains the always-local tree-sitter/BM25 symbol path and does not create an empty vector cache just for lookup.
- **Hybrid does not mean “vectors only.”** Semantic similarity can join lexical, structural, temporal, and entity signals. Exact symbol names and paths retain their precision advantage; a semantic code result is merged with the pattern channel rather than replacing it.

### Live coverage snapshot: implemented ≠ enabled ≠ indexed ≠ complete

The 9 August live status is deliberately shown as coverage, not as a blanket semantic-search claim:

| State | Live observation | What it means—and does not mean |
| --- | --- | --- |
| **Implemented** | The checkout contains `code_embedding_text` and `embed_pending_code` in `cas-cli/src/cloud/code_embeddings.rs` (lines 22 and 54), plus the source-code hybrid merge path. | CAS has a source-symbol vector pipeline; it is not merely a planned design. |
| **Authenticated** | `cas auth whoami --json` reported `logged_in: true`. | A cloud embedder can be configured; authentication alone does not prove an individual query will receive a semantic row. |
| **Indexed** | `cas status --json` reported 362 indexed files, 6,694 stored symbols, no file lag, and the current `85963767` HEAD. | The always-local tree-sitter/BM25 source path is current. |
| **Vector coverage** | The same status reported 548 vector-eligible symbols: 533 vectorized, 15 pending, 0 failed. | Source vectors exist and the queue is partly caught up; pending work means coverage is not yet 100%. |
| **Enabled at query time** | The code channel opens only with cloud auth plus an existing non-empty cache whose metadata matches the embedder. | A stale/missing/mismatched cache or an embedding failure falls back to structural/BM25 lookup; status coverage is not substituted for this gate. |

The MCP code-search query `embedding pending code symbol` supplied a live semantic/paraphrase retrieval receipt, while the exact symbol-and-path receipt came from local source inspection. Those are complementary pieces of evidence: a search result establishes searchable conceptual coverage; the exact source path establishes the current implementation boundary.

## The safety properties behind the visual

| Control | Why it matters in the product experience | Current evidence |
| --- | --- | --- |
| **Pending state is visible and retryable** | A failed cloud request does not masquerade as completed learning. | Pending queues retain pages, history items, and symbols until a valid vector is recorded. |
| **Zero vectors are rejected** | An all-zero vector is not a meaningful similarity signal; storing it would pollute recall. | The cache refuses it and leaves the unit pending. |
| **Metadata gates comparison** | Vectors from different models or dimensions are not silently mixed. | `{provider, model, dims}` is stored with the cache; a mismatch clears the stale cache and re-arms work. |
| **Namespaces protect corpus boundaries** | A code query must not return a plausible-looking knowledge or history record. | Knowledge/history share a prefixed namespace; source vectors use a separate LMDB environment and a code prefix. |
| **Rate and request limits are explicit** | A large backlog cannot turn into an unbounded burst. | The endpoint accepts at most 32 inputs per request; drains honor an invocation budget and shared limiter. |
| **Local fallback remains real** | Offline or logged-out use remains a useful product, not a broken semantic promise. | BM25/FTS and tree-sitter symbol lookup run locally; absent semantic weight is not treated as a live retrieval channel. |

## What the next agent experiences

1. **Start oriented.** Pinned guidance and a compact context/knowledge index present the project’s operating shape without flooding the prompt.
2. **Ask a focused question.** Search can reach memories, knowledge, tasks, rules, skills, code symbols, and history/provenance.
3. **Inspect the evidence.** Knowledge stays readable Markdown with source lineage; history and code search link a result back to the record or symbol behind it.
4. **Act, then improve the loop.** The outcome can become a scoped, durable memory or a distilled page. Retrieval on the next task can now use it.

## Grounding: convergence, not endorsement

Tencent’s public Agent Memory materials describe reusable assets drawn from conversations, documents, and code; layered context; selective retrieval; and governed sharing. CAS follows the same broad product direction in a local-first coding-agent system: capture reusable work, keep it inspectable, retrieve it selectively, and scope who can reuse it. This is an architectural comparison only—not a partnership, benchmark, or endorsement by Tencent.

| Public grounding | What it contributes to this showcase |
| --- | --- |
| [TencentDB Agent Memory: reusable assets, Wiki, and CodeGraph](https://github.com/TencentCloud/TencentDB-Agent-Memory#what-is-tencentdb-agent-memory) | A public example of memory extending beyond conversation retention into reusable work assets. |
| [TencentDB Agent Memory: layered refinement and retrieval](https://github.com/TencentCloud/TencentDB-Agent-Memory#technical-implementation) | Support for the product principle of selective, layered context rather than prompt sprawl. |
| [Tencent Cloud: Agent Long-Term Memory overview](https://www.tencentcloud.com/document/product/409/80363) | Public discussion of memory types, metadata, isolation, and lifecycle. |
| [CAS public repository](https://github.com/pippenz/cas) | The public product and source evidence for CAS; links below point to the corresponding implementation areas. |

## Evidence map

| Claim | Direct public evidence |
| --- | --- |
| Durable memory types and scope | [memory MCP surface](https://github.com/pippenz/cas/blob/main/cas-cli/src/mcp/tools/core/memory.rs) · [entry model](https://github.com/pippenz/cas/blob/main/crates/cas-types/src/entry.rs) |
| Knowledge pages, source lineage, and locks | [knowledge MCP surface](https://github.com/pippenz/cas/blob/main/cas-cli/src/mcp/tools/core/knowledge.rs) · [architecture](https://github.com/pippenz/cas/blob/main/cas-cli/docs/ARCHITECTURE.md#knowledge-pages) |
| Always-active pinned guidance | [session context builder](https://github.com/pippenz/cas/blob/main/crates/cas-core/src/hooks/context/build_start.rs) |
| History/provenance | [history search MCP surface](https://github.com/pippenz/cas/blob/main/cas-cli/src/mcp/tools/core/search.rs) · [history store](https://github.com/pippenz/cas/blob/main/crates/cas-store/src/history_store.rs) |
| Vector client, limits, metadata, and cache | [embeddings implementation](https://github.com/pippenz/cas/blob/main/cas-cli/src/cloud/embeddings.rs) |
| Knowledge/history drain | [embedding drain](https://github.com/pippenz/cas/blob/main/cas-cli/src/cloud/embed_drain.rs) |
| Source-code queue and vector drain | [source-code embeddings](https://github.com/pippenz/cas/blob/main/cas-cli/src/cloud/code_embeddings.rs) · [durable code-vector ledger](https://github.com/pippenz/cas/blob/main/crates/cas-store/src/code_vector_store.rs) |
| Source-code structural + semantic merge | [code hybrid search](https://github.com/pippenz/cas/blob/main/cas-cli/src/hybrid_search/code.rs) · [tree-sitter code crate](https://github.com/pippenz/cas/tree/main/crates/cas-code) |

## Provenance and review record

**Inspection date:** 9 August 2026. **Local checkout inspected:** `85963767`. **External research window:** 8–9 August 2026. Public links intentionally point to `main`, so readers can inspect the evolving public implementation.

**Commands and live evidence used:**

```text
mcp__cs__search action=code_search query=embed_pending_code include_source=true
rg -n -i 'pending_embedding|cas-embed-v1|embedding|vector|lmdb|zero vector|tree-sitter|BM25|semantic' crates cas-cli docs README.md
rg -n 'embed_pending_code|drain_all_pending_with|DEFAULT_EMBEDDING_MODEL|DEFAULT_EMBEDDING_DIMS|MAX_EMBED_INPUTS_PER_REQUEST|VectorNamespace' cas-cli/src/cloud cas-cli/src/hybrid_search cas-cli/src/daemon crates/cas-store
git show --stat --oneline --all -- cas-cli/src/cloud/code_embeddings.rs
cas doctor
cas status --json
cas auth whoami --json
```

The doctor/status receipts showed a current source index, 533 vectorized source symbols, 15 pending symbols, no failed symbols, and authenticated cloud status. The code-search call returned indexed symbols from the live CAS environment, while local source inspection found the current source-vector implementation from `cas-733e` plus its race-safety follow-up `cas-c84d`. This report therefore supersedes earlier wording that described source vectors as planned. It still labels semantic retrieval accurately: cloud authentication, a compatible model, and populated cache are required before that channel contributes results.
