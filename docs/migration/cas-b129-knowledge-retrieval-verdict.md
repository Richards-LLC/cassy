# Knowledge retrieval vs legacy memory retrieval — measurement and verdict

**Task:** cas-d075 (EPIC cas-b129, carried from cas-edee/M5, filed by the cas-7909/M6 survey §5.3)
**Measured:** 2026-08-07, host `soundwave`, post-cutover
**Original verdict: WORSE THAN LEGACY — legacy read paths GATED CLOSED.**
**Superseded 2026-08-07 by the addendum at the end of this document (cas-461a):
the conjunction defect is fixed and re-measured at parity-or-better. The gate in
§"The gate" is SATISFIED.** The body below is preserved unedited as the
pre-fix record.

## Why this document exists

Nothing else on EPIC cas-b129 measures whether the knowledge system *retrieves*
as well as the legacy memory system. The M4 parity harness answers a different
question by construction — "is the same knowledge still retrievable at the same
rank **from the legacy surfaces**" — and all of its channels are legacy-memory
channels. A green M4 replay proves only that the migration left the legacy read
paths undisturbed.

Decommissioning a legacy read path in favour of an unmeasured replacement is
exactly the loss this epic promises not to incur. This is that measurement, and
it is the gate any future legacy read-path removal must cite.

## Verdict

**Knowledge retrieval is materially worse than legacy retrieval today.** On 7 of
10 queries drawn from the real corpus vocabulary, the knowledge store returns
**zero** pages where the legacy store returns 4–10 hits. Two more return a single
page. One query behaves.

This is **not** a distillation or coverage failure. The content is present and
indexed — the same queries evaluated disjunctively match 18–107 of the 107
project pages. It is a **query-construction defect**, and it has a single named
cause.

## Cause

`SqliteKnowledgeStore::fts_query` (`crates/cas-store/src/knowledge_store.rs:735-746`)
tokenizes the query and joins the quoted tokens with a **space**. In FTS5 a space
is an implicit `AND`, so a knowledge search requires *every* term to occur in the
*same* page. `search` (`knowledge_store.rs:1189-1215`) passes that expression
straight to `knowledge_pages_fts MATCH ?1`.

The surface it replaces — Tantivy BM25 over `entries` — is **disjunctive**: it
matches any term and ranks by relevance. So the two sides do not merely differ in
index technology; they differ in boolean semantics, and the knowledge side is the
strict one. Query length is therefore inversely related to recall: past about
three terms, a user gets nothing.

## Measurement

Queries are the ten `search` cases from the committed M4 query set
(`fixtures/retrieval-parity/queryset.toml`). They were derived from the actual
high-frequency vocabulary of the real corpus, *before* and independently of this
task — they are not queries chosen to flatter or damn either side.

| Query | knowledge (AND, production) | knowledge (OR) | legacy BM25 |
|---|---|---|---|
| factory worker supervisor spawn | 7 | 55 | 5 |
| task close verification merge branch | 0 | 53 | 6 |
| worktree commit cas-src crates | 0 | 107 | 9 |
| cargo build tests check | 0 | 36 | 10 |
| session claude codex agent message | 0 | 72 | 10 |
| cloud sync config server local | 0 | 56 | 4 |
| release staging shipped status | 1 | 54 | 10 |
| root cause fixed stale pattern | 0 | 43 | 5 |
| review skill files project | 1 | 107 | 4 |
| roark richards account | 0 | 18 | 10 |

Legacy figures are hit counts from the committed global-inclusive parity baseline
`fixtures/retrieval-parity/baseline-soundwave.json` (captured under cas-96ae, the
first baseline on this epic that actually reaches the global store).

### Regressions, named individually

Each of these is a query that works against legacy memory and returns nothing
from knowledge:

1. `task close verification merge branch` — legacy 6, knowledge 0
2. `worktree commit cas-src crates` — legacy 9, knowledge 0
3. `cargo build tests check` — legacy 10, knowledge 0
4. `session claude codex agent message` — legacy 10, knowledge 0
5. `cloud sync config server local` — legacy 4, knowledge 0
6. `root cause fixed stale pattern` — legacy 5, knowledge 0
7. `roark richards account` — legacy 10, knowledge 0

Degraded but non-zero: `release staging shipped status` (10 → 1) and
`review skill files project` (4 → 1). At parity or better: none.

## Reproducing it

The knowledge side runs through the shipped command, not a reimplementation:

    cas knowledge search "cargo build tests check"

which prints `No distilled pages match 'cargo build tests check'.` The full table
above is reproduced by running that command for each query in the table and
reading the leading count. The disjunctive column is obtained by evaluating the
same tokens against `knowledge_pages_fts` joined with ` OR ` instead of a space,
against a read-only connection to `.cas/cas.db`.

The legacy column is read from the committed baseline:

    cas retrieval-parity capture && cas retrieval-parity replay

Note `--json` on `cas knowledge` is a **group-level** flag: `cas knowledge --json
search <q>`. Placed after the subcommand it is accepted but output stays text —
a parser that assumes JSON will silently read every result as empty. This
document's counts were taken from the text output for that reason.

## Corpus and lineage (methodology)

Post-cutover the project store holds **107** knowledge pages against **1254**
live entries; the global store holds **39** pages against **450** entries — 146
pages total, matching the M6 survey. Page types: context 121, persona 21,
learning 4.

The comparison is deliberately restricted to content that actually migrated. The
mapping spec routes 438 rows to `Disposition::StayEntry` ("Remains in entries,
untouched"), so a whole-corpus comparison would not be apples-to-apples.

Establishing "what migrated" required work, because **`sources_json` is empty on
all 146 pages** — deliberately, per Rule P2 (`memory_migration/apply.rs:21`):
`sources` is CAS-owned provenance, not migration lineage. (`knowledge_sources`,
also empty, is an unrelated file-ingestion ledger and is *not* evidence of
anything here.) So no stored join exists.

Lineage is nevertheless recoverable **exactly**, because page ids are
deterministic:

    page_id(db, legacy_id) = "cas-kn-mig-" + hex(sha256(db_label || 0x1f || legacy_id)[..5])

(`memory_migration/apply.rs:56-68`). Recomputing that over every live entry id
reconstructs the mapping with no heuristics: **107/107** project pages and
**39/39** global pages resolve to a live entry, with **zero** orphan pages. "What
migrated" is an exact set, not an estimate.

## The confound this does NOT claim

Legacy entries never had semantic retrieval — the semantic channel is defined
over `KnowledgePage`, not `Entry`, and every legacy row carries
`pending_embedding = 1` (M1 inventory §3.2). So a semantic-vs-lexical comparison
would measure something that never existed on the legacy side. The comparison
above is lexical-vs-lexical on both sides, and the question asked is narrow and
answerable: **did distillation preserve findability?** It did not, for the reason
named above.

Equally, this document makes no claim that distillation lost *content*. The
disjunctive column is the evidence that it did not.

## The gate

**No legacy read path may be removed while `fts_query` builds a conjunction.**
Removing the legacy BM25 path today would convert a working search surface into a
silently empty one for the majority of multi-term queries — the failure is silent,
which is what makes it dangerous: a user gets a clean "no matches", not an error.

A future removal must cite this document and show that the conjunction defect is
fixed and re-measured. The cheap remedy to evaluate first is making `fts_query`
disjunctive (or an `OR`-with-`AND`-preference ranking, which is what BM25 over a
disjunctive match set already gives you) — but that is a change to shipped
retrieval behaviour and belongs in its own task with its own before/after numbers,
not as a rider here.

## Status of the deliverable

Complete: the measurement, the named regressions, the verdict, the lineage
methodology, and the reproduction recipe.

Owed (follow-up, does not change the verdict): promoting the reproduction recipe
from documented commands to a single committed `cas` subcommand that prints both
sides and a summary line. The verdict above is what gates removals; the
subcommand is ergonomics.

---

# Addendum — 2026-08-07: conjunction fixed, re-measured at parity-or-better

**Task:** cas-461a. **Measured:** 2026-08-07, host `soundwave`.
**Revised verdict: AT PARITY OR BETTER on the cas-d075 query set. The removal
gate stated above is SATISFIED.**

## What changed

`SqliteKnowledgeStore::fts_query` now joins terms with `OR` instead of a space,
so multi-term knowledge search is disjunctive and ranked by `bm25()` — matching
the boolean semantics of the legacy Tantivy surface it replaces. Explicit
double-quoted runs are preserved as FTS5 phrases, which the pre-fix tokenizer
did not support at all (it split on every non-alphanumeric character, so user
quotes were discarded).

No separate "AND-preference" pass was needed: `search` already orders by
`bm25()`, and BM25 over a disjunctive match set inherently ranks pages carrying
more of the query's terms above pages carrying fewer. That is the behaviour the
pre-fix section above anticipated, obtained for free.

## Re-measurement

Same ten `search` cases from `fixtures/retrieval-parity/queryset.toml`, same
legacy baseline (`fixtures/retrieval-parity/baseline-soundwave.json`), same
corpus. Both binaries were run against an **identical byte-for-byte copy** of the
live project store (`sqlite3 .backup` of `.cas/cas.db` plus the knowledge
directory, in a scratch dir with `CAS_ROOT`/`CAS_DIR` unset) so the two columns
differ only by the code under test. All counts come from the shipped command at
its default depth:

    cas knowledge search "<query>" --limit 10

| Query | knowledge BEFORE (AND) | knowledge AFTER (OR) | legacy BM25 |
|---|---|---|---|
| factory worker supervisor spawn | 7 | 10 | 5 |
| task close verification merge branch | 0 | 10 | 6 |
| worktree commit cas-src crates | 0 | 10 | 9 |
| cargo build tests check | 0 | 10 | 10 |
| session claude codex agent message | 0 | 10 | 10 |
| cloud sync config server local | 0 | 10 | 4 |
| release staging shipped status | 1 | 10 | 10 |
| root cause fixed stale pattern | 0 | 10 | 5 |
| review skill files project | 1 | 10 | 4 |
| roark richards account | 0 | 10 | 10 |

**Regressions: none. Queries returning zero: 7 → 0. At parity or better: 10/10.**

The BEFORE column was re-derived, not copied from the table above: the shipped
pre-fix binary (`cas 2.50.0`, `4132e03`) was run against the same scratch copy
and reproduced 7 / 0 / 0 / 0 on the spot-checked rows, confirming the harness is
measuring the same thing the original verdict measured.

## What this measurement does and does not claim

It claims **findability is restored**: every query that silently returned nothing
now returns results, and none returns fewer than legacy.

It does **not** claim ranking quality. Every post-fix row reads `10` because the
match sets are larger than the requested depth and saturate `--limit 10`; the
uncapped disjunctive match sets are the 18–107 figures in the pre-fix table,
which are the same OR semantics now shipping. So these numbers prove the recall
cliff is gone, not that the *best* page ranks first. Ordering is BM25's job and
is unchanged in kind from the legacy surface, but a rank-quality comparison is a
different measurement than this one and is not asserted here.

## Coverage

Unit tests on the constructed expression (`fts_query_is_disjunctive_and_
preserves_explicit_phrases`) and on store behaviour
(`search_returns_partial_term_matches_and_ranks_full_matches_first`), plus a CLI
integration test (`test_knowledge_multi_term_search_is_disjunctive`) that pins
the disjunctive win, the phrase-adjacency guarantee, and the negative case — a
query sharing no term with the corpus still reports no matches, so "disjunctive"
has not become "matches anything".
