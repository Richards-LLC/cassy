# CAS Memory: experience that compounds

**Feature showcase · 8 August 2026**

CAS turns work that would normally disappear at the end of an agent session into reusable experience. It captures the durable lesson, distills project understanding, retrieves the right evidence when needed, and carries it into the next piece of work—without turning the whole project into a giant prompt.

> **The signature loop:** **work → capture → distill → retrieve → reuse**

The problem is familiar: a capable agent starts a new session and re-learns the repository, the constraints, the decisions, and the proven way to operate. CAS makes that learning cumulative. A preference can remain personal. A project fact can become durable memory. Documentation and code can become an inspectable project wiki. Past code changes can remain searchable as provenance. The next agent begins with an orientation, then pulls only the detail its task requires.

This is a product showcase, not a performance claim. It describes implementation capabilities visible in the public CAS repository and their convergence with published Tencent Agent Memory design principles.

## How the experience works

```text
Work
  ↓  capture the lesson that should survive
Durable memory
  ↓  distill source-backed project understanding
Inspectable knowledge
  ↓  retrieve only the useful context and provenance
Focused context
  ↓
Reuse in the next task, session, or agent
```

1. **Work creates signal.** An agent records a learning, preference, context item, or observation rather than leaving the insight only in a transcript.
2. **Capture makes it durable.** Memory has explicit scope, importance, lifecycle tier, tags, and validity boundaries.
3. **Distill creates orientation.** CAS can turn the repository’s own docs and code summaries into ordinary Markdown knowledge pages with source lineage.
4. **Retrieve stays focused.** Search spans memory, knowledge, indexed code, and history/provenance so an agent can discover the relevant thread before opening detail.
5. **Reuse begins the next loop ahead.** A compact knowledge index can orient a session; full pages and source evidence are brought in on demand.

## Signature capabilities

### Durable, atomic memory

CAS stores learnings, preferences, context, and observations as individual memory records that survive sessions. They can carry importance, tags, scope, lifecycle tier, and a validity window—enough structure to preserve a useful fact without burying it in a chat log. [Public CAS memory surface](https://github.com/pippenz/cas/blob/main/cas-cli/src/mcp/tools/core/memory.rs) · [memory model](https://github.com/pippenz/cas/blob/main/crates/cas-types/src/entry.rs)

### A wiki made from the project itself

Knowledge distillation writes project understanding as ordinary Markdown pages on disk, indexed with source provenance. That makes the result both useful to an agent and inspectable by a person with normal repository tools. [Architecture: knowledge pages](https://github.com/pippenz/cas/blob/main/cas-cli/docs/ARCHITECTURE.md#knowledge-pages) · [knowledge MCP surface](https://github.com/pippenz/cas/blob/main/cas-cli/src/mcp/tools/core/knowledge.rs)

### Progressive disclosure, not prompt sprawl

CAS uses a compact knowledge index for orientation and exposes searchable, readable pages for the moments when depth is necessary. The principle is simple: discover first, load detail second. [CAS README: Knowledge](https://github.com/pippenz/cas#knowledge) · [knowledge retrieval implementation](https://github.com/pippenz/cas/blob/main/cas-cli/src/mcp/tools/core/knowledge.rs)

### Retrieval with multiple useful paths

CAS search can draw from persistent entries, knowledge, indexed code symbols, graph and temporal signals, and Git history/provenance. The source-code path is tree-sitter symbol indexing plus local BM25—not a claim that the entire codebase is vectorized end-to-end. Optional cloud capability can add semantic ranking where available. [Hybrid search implementation](https://github.com/pippenz/cas/blob/main/cas-cli/src/hybrid_search/hybrid.rs) · [README: search, honestly](https://github.com/pippenz/cas#context-system)

### History that can answer “why?”

Code history and provenance make prior changes part of retrievable working memory: an agent can connect a change, its path, its timing, and available task/session provenance instead of rediscovering intent from scratch. [History store](https://github.com/pippenz/cas/blob/main/crates/cas-store/src/history_store.rs) · [history search surface](https://github.com/pippenz/cas/blob/main/cas-cli/src/mcp/tools/core/search.rs)

### Trust controls built into the artifact

Knowledge is readable Markdown, source-linked, and lockable. A lock is intentionally respected by automated distillation and incoming synchronization, preserving human-authored truth. Memory sharing is optional and scoped; local-first operation remains useful without cloud services. [User-sovereignty lock](https://github.com/pippenz/cas/blob/main/cas-cli/docs/ARCHITECTURE.md#knowledge-pages) · [README: local-first and optional cloud](https://github.com/pippenz/cas#cloud-optional)

## Where this becomes compelling

| Moment | What CAS carries forward | Why it changes the experience |
| --- | --- | --- |
| **A new coding session** | Architecture, conventions, constraints, and relevant past changes | The agent starts oriented instead of asking the team to repeat the project’s story. |
| **A recurring incident or review** | Previous diagnosis, guardrails, and a reusable operating pattern | A hard-won workflow can become a durable playbook rather than tribal memory. |
| **A handoff across agents** | Shared project knowledge plus bounded, optionally scoped memory assets | Work can continue with context while respecting ownership and sharing boundaries. |
| **A decision with history** | Source pages and searchable commit provenance | The agent can trace what changed and inspect the evidence behind a constraint. |
| **An unfamiliar repository** | A distilled map of docs and code, with drill-down links | Cold start becomes navigation and verification—not blind rediscovery. |

## Convergence with Tencent’s published memory architecture

Tencent’s public materials describe an agent-memory system as more than conversation retention: it should extract reusable assets, organize them in layers, retrieve selectively, preserve governance, and let teams carry experience into new work. CAS converges with these principles in a local-first coding-agent context. This is an **alignment of architecture**, not an endorsement by Tencent and not a claim that either system was built from the other.

| Tencent principle — sourced statement | CAS implementation — observed public evidence | Interpretation |
| --- | --- | --- |
| **Memory is reusable work, not merely chat.** TencentDB Agent Memory describes Chat Memory, Skills, Wiki, and CodeGraph as reusable assets created from conversations, documents, and code. [TencentDB Agent Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory#what-is-tencentdb-agent-memory) | CAS exposes durable memory alongside skills, a distilled knowledge wiki, and indexed code/search surfaces. [CAS README](https://github.com/pippenz/cas#context-system) | Both systems treat accumulated work as assets the next agent can use. |
| **Layer and compress context.** Tencent describes an L0 conversation → L1 atom → L2 scenario → L3 core/persona refinement path. [TencentDB Agent Memory: technical implementation](https://github.com/TencentCloud/TencentDB-Agent-Memory#technical-implementation) | CAS keeps atomic memories while distilling repository sources into concise, source-linked knowledge pages. [CAS architecture](https://github.com/pippenz/cas/blob/main/cas-cli/docs/ARCHITECTURE.md#knowledge-pages) | CAS’s memory-plus-knowledge split converges on the value of compact, reusable context without claiming identical layers. |
| **Retrieve on demand, within a context budget.** Tencent says its retrieval uses layered fallback and caps results to avoid overwhelming the context window. [TencentDB Agent Memory: technical implementation](https://github.com/TencentCloud/TencentDB-Agent-Memory#technical-implementation) | CAS provides a compact knowledge index and retrieves full pages when requested; its hybrid search combines local channels and optional semantic capability. [CAS README](https://github.com/pippenz/cas#knowledge) · [hybrid search](https://github.com/pippenz/cas/blob/main/cas-cli/src/hybrid_search/hybrid.rs) | Both favor selective recall over wholesale context injection. |
| **Make knowledge include documents and code.** Tencent describes Wiki pages and CodeGraph as searchable assets with code symbols and relationships. [TencentDB Agent Memory: knowledge map](https://github.com/TencentCloud/TencentDB-Agent-Memory#-a-knowledge-map-that-reads-both-docs-and-code) | CAS distills documentation to Markdown knowledge and indexes code symbols; search includes code and history paths. [CAS architecture](https://github.com/pippenz/cas/blob/main/cas-cli/docs/ARCHITECTURE.md) · [code search](https://github.com/pippenz/cas/tree/main/crates/cas-search) | In both designs, useful memory reaches beyond the conversation transcript. |
| **Govern who can reuse memory.** Tencent documents private, team, and restricted visibility plus ownership and access control. [TencentDB Agent Memory: team memory panel](https://github.com/TencentCloud/TencentDB-Agent-Memory#-a-team-memory-panel-controlled-by-humans) | CAS offers scoped memory and optional project/team sharing while protecting locked knowledge from automated overwrite. [CAS README: Cloud](https://github.com/pippenz/cas#cloud-optional) · [lock behavior](https://github.com/pippenz/cas/blob/main/cas-cli/docs/ARCHITECTURE.md#knowledge-pages) | CAS converges on governed reuse, with its own local-first and scope-specific design. |
| **Preserve durable records and lifecycle.** Tencent’s overview documents memory types, metadata, append-only records, expiration, and memory decay. [Tencent Cloud: Agent Long-Term Memory Feature Overview](https://www.tencentcloud.com/document/product/409/80363) | CAS memory records include type, scope, importance, lifecycle tier, archive state, and optional validity boundaries. [CAS entry model](https://github.com/pippenz/cas/blob/main/crates/cas-types/src/entry.rs) | Both recognize that memory needs lifecycle controls, not just a storage bucket. |

## Architecture in one view

```text
                   ┌───────────────────────────┐
                   │ Work: tasks, code, docs,  │
                   │ conversations, outcomes   │
                   └─────────────┬─────────────┘
                                 │
              ┌──────────────────┴──────────────────┐
              │                                     │
   ┌──────────▼──────────┐               ┌──────────▼──────────┐
   │ Atomic memories     │               │ Project knowledge   │
   │ facts / preferences │               │ Markdown + sources  │
   │ context / lessons   │               │ human-lockable      │
   └──────────┬──────────┘               └──────────┬──────────┘
              └──────────────────┬──────────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │ Retrieve and inspect     │
                    │ memory · knowledge ·     │
                    │ code · history/provenance│
                    └────────────┬────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │ Reuse in the next task   │
                    └─────────────────────────┘
```

The value is the loop, not a single store: facts stay atomic; project understanding becomes readable; retrieval reaches into code and history; and the human can inspect or protect the result.

## Trust and inspectability

- **Readable by default.** Distilled knowledge pages are Markdown files with source provenance, not an opaque prompt cache.
- **Human authority is explicit.** A locked knowledge page is protected from automatic distillation and incoming overwrite. [Implementation evidence](https://github.com/pippenz/cas/blob/main/cas-cli/docs/ARCHITECTURE.md#knowledge-pages)
- **Recall is bounded and truthful.** CAS describes local BM25/FTS capability separately from optional cloud semantic ranking. [Public capability statement](https://github.com/pippenz/cas#context-system)
- **Sharing is not compulsory.** Local-first use works without an account; cloud sync and team sharing are optional, with project and preference scope rules documented publicly. [CAS Cloud](https://github.com/pippenz/cas#cloud-optional)
- **Evidence remains inspectable.** Knowledge source lineage, code symbols, and git-history search leave a route back to the record behind a retrieved answer. [Architecture](https://github.com/pippenz/cas/blob/main/cas-cli/docs/ARCHITECTURE.md) · [history store](https://github.com/pippenz/cas/blob/main/crates/cas-store/src/history_store.rs)

## Honest boundaries

CAS is not presented as a replacement for judgment, source review, or access control. Retrieval can surface relevant material; an agent still needs to inspect the source and apply the correct policy. Source-code retrieval is lexical/symbol-based locally rather than end-to-end vectorized code search; semantic ranking and cross-machine/team delivery are optional cloud capabilities. The local-first experience remains deliberately useful without them. The Tencent comparison is architectural convergence based on public materials—not a benchmark, partnership, endorsement, or claim of identical implementation.

## Sources and provenance

**Research window:** 8 August 2026. **CAS repository reference:** [`286af839`](https://github.com/pippenz/cas/commit/286af839) (local inspection; public repository links in this report target the `main` branch). No adoption, ROI, quality, or performance figures are asserted.

### External sources

- [Tencent Cloud — Agent Long-Term Memory Feature Overview](https://www.tencentcloud.com/document/product/409/80363): memory model, types, metadata, isolation, lifecycle examples, and applicable scenarios.
- [TencentDB Agent Memory — GitHub repository](https://github.com/TencentCloud/TencentDB-Agent-Memory): reusable asset model, layered refinement, on-demand retrieval, Wiki/CodeGraph, and governed sharing statements.
- [CAS — public GitHub repository](https://github.com/pippenz/cas): product description and implementation evidence linked inline throughout this report.

### Research commands

```text
exa-search --contents https://www.tencentcloud.com/document/product/409/80363
exa-search --contents https://github.com/TencentCloud/TencentDB-Agent-Memory
exa-search --contents https://github.com/pippenz/cas
git rev-parse HEAD
rg -n "knowledge|memory|history|hybrid" README.md cas-cli/docs/ARCHITECTURE.md cas-cli/src crates/cas-store crates/cas-types
```

**Interpretation discipline:** Tencent statements above are linked to Tencent’s public materials. CAS implementation statements are linked to public CAS documentation or source. The “Interpretation” column states the reasoned comparison and should not be read as a statement by either Tencent or CAS users.
