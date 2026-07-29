/**
 * cas-code-review-constants.js — exported constants for the cas-code-review Workflow.
 *
 * Kept in a separate ES module so both:
 *   - cas-code-review.js (Workflow script — can't be a standard ES module due to
 *     top-level `return` statements used by the Workflow runtime)
 *   - cas-code-review.test.js (Node.js test runner)
 * can import the same constants without hitting the Workflow-runtime-only syntax.
 */

// ─────────────────────────────────────────────────────────────────────────────
// META
// ─────────────────────────────────────────────────────────────────────────────

export const WORKFLOW_META = Object.freeze({
  name: 'cas-code-review',
  description: 'cas-code-review Steps 1-4: intent extraction, persona selection, sharded dispatch, deterministic merge',
  phases: [
    { title: 'Resolve', detail: 'validate args + fallow pre-check' },
    { title: 'Review', detail: 'parallel persona dispatch, sharded for large diffs (Codex + Claude diversity lane)' },
    { title: 'Merge', detail: 'deterministic 7-step merge (pure JS, no LLM)' },
  ],
})

// ─────────────────────────────────────────────────────────────────────────────
// PERSONA SETS
// ─────────────────────────────────────────────────────────────────────────────

export const ALWAYS_ON_PERSONAS = Object.freeze(
  ['correctness', 'testing', 'maintainability', 'project-standards']
)

export const CONDITIONAL_PERSONAS = Object.freeze(
  ['security', 'performance', 'adversarial']
)

// ─────────────────────────────────────────────────────────────────────────────
// SCHEMA — mirrors ReviewerOutput + Finding from crates/cas-types/src/code_review.rs
// ─────────────────────────────────────────────────────────────────────────────

const FINDING_SCHEMA = {
  type: 'object',
  required: ['title','severity','file','line','why_it_matters','autofix_class','owner','confidence','evidence','pre_existing'],
  additionalProperties: false,
  properties: {
    title:                { type: 'string', maxLength: 100 },
    severity:             { type: 'string', enum: ['P0','P1','P2','P3'] },
    file:                 { type: 'string' },
    line:                 { type: 'integer', minimum: 1 },
    why_it_matters:       { type: 'string' },
    autofix_class:        { type: 'string', enum: ['safe_auto','gated_auto','manual','advisory'] },
    owner:                { type: 'string', enum: ['review-fixer','downstream-resolver','human'] },
    confidence:           { type: 'number', minimum: 0.0, maximum: 1.0 },
    evidence:             { type: 'array', items: { type: 'string' }, minItems: 1 },
    pre_existing:         { type: 'boolean' },
    suggested_fix:        { type: 'string' },
    requires_verification:{ type: 'boolean' },
  },
}

export const REVIEWER_OUTPUT_SCHEMA = Object.freeze({
  type: 'object',
  required: ['reviewer', 'findings'],
  additionalProperties: false,
  properties: {
    reviewer:       { type: 'string' },
    findings:       { type: 'array', items: FINDING_SCHEMA },
    residual_risks: { type: 'array', items: { type: 'string' } },
    testing_gaps:   { type: 'array', items: { type: 'string' } },
    skipped_reason: { type: 'string' },
  },
})

// ─────────────────────────────────────────────────────────────────────────────
// INDEPENDENT PERSONA HELPERS
// gpt55* names and args remain as backwards-compatible public aliases.
// Runtime Workflow scripts keep inline copies of these functions because they
// cannot import ES modules.
// ─────────────────────────────────────────────────────────────────────────────

function gpt55ShouldRun(args = {}, fileCount, changeLines) {
  const {
    gpt56_independent: gpt56IndependentArg,
    enable_gpt56_independent: enableGpt56IndependentArg,
    gpt55_independent: gpt55IndependentArg,
    enable_gpt55_independent: enableGpt55IndependentArg,
    independent_review: independentReviewArg,
  } = args ?? {}
  const gpt55Explicit = gpt56IndependentArg === true
    || gpt56IndependentArg === 'true'
    || enableGpt56IndependentArg === true
    || enableGpt56IndependentArg === 'true'
    || gpt55IndependentArg === true
    || gpt55IndependentArg === 'true'
    || enableGpt55IndependentArg === true
    || enableGpt55IndependentArg === 'true'
    || independentReviewArg === 'gpt-5.5'
    || independentReviewArg === 'gpt55'
    || independentReviewArg === 'gpt-5.5:independent'
    || independentReviewArg === 'gpt-5.6-sol'
    || independentReviewArg === 'gpt-5.6-sol:independent'
  const gpt55BroadDiff = fileCount >= 5 || changeLines >= 300
  return gpt55Explicit || gpt55BroadDiff
}

function gpt55SkippedPersonas(gpt55Result) {
  if (!gpt55Result?.skipped_reason) return []
  return [{
    reviewer: 'gpt-5.6-sol:independent',
    reason: gpt55Result.skipped_reason,
  }]
}

function personasRunCount(personasToDispatchCount, fallowRuns, gpt55Runs, gpt55Skipped) {
  return personasToDispatchCount + (fallowRuns ? 1 : 0) + (gpt55Runs && !gpt55Skipped ? 1 : 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// LARGE-DIFF SHARDING HELPERS
// ─────────────────────────────────────────────────────────────────────────────

export const DEFAULT_LARGE_DIFF_TOKEN_THRESHOLD = 12000
export const INTERFACE_INTEGRATOR_SHARD = 'interface-integrator'

function estimateDiffTokens(diffText = '') {
  return Math.ceil(String(diffText).length / 4)
}

function normalizeChangedFiles(fileList = '') {
  return String(fileList)
    .split('\n')
    .map(path => path.trim())
    .filter(Boolean)
}

function largeDiffThreshold(args = {}) {
  const raw = args.large_diff_token_threshold
    ?? args.review_shard_token_threshold
    ?? args.shard_token_threshold
  const n = Number(raw)
  return Number.isFinite(n) && n > 0 ? n : DEFAULT_LARGE_DIFF_TOKEN_THRESHOLD
}

function shouldShardReview(diffText = '', args = {}) {
  return estimateDiffTokens(diffText) > largeDiffThreshold(args)
}

function subsystemForFile(path = '') {
  if (/^(docs\/|.*\.md$|\.claude\/skills\/|cas-cli\/src\/builtins\/(codex\/)?skills\/)/.test(path)) {
    return 'docs-skills'
  }
  if (/^\.claude\/workflows\/|cas-cli\/src\/builtins\/workflows\//.test(path)) {
    return 'code-review-workflow'
  }
  if (/^cas-cli\/src\/ui\/factory\/|^crates\/cas-factory/.test(path)) {
    return 'factory-ui'
  }
  if (/^cas-cli\/src\/mcp\/tools\/core\/task\/|^cas-cli\/src\/mcp\/tools\/core\/agent_coordination\//.test(path)) {
    return 'mcp-task-lifecycle'
  }
  if (/^crates\/cas-store\/|^crates\/cas-types\/|^crates\/cas-core\/src\/migration\//.test(path)) {
    return 'store-types'
  }
  if (/(^|\/)(tests?|__tests__)\/|(_test|\.test|\.spec)\./.test(path)) {
    return 'tests'
  }
  return 'code-other'
}

function isDocsOnlyShard(shard) {
  return shard?.kind === 'subsystem' && shard.subsystem === 'docs-skills'
}

function isMechanicalTestShard(shard) {
  return shard?.kind === 'subsystem' && shard.subsystem === 'tests'
}

function shardPersonas(shard, basePersonas = []) {
  const unique = [...new Set(basePersonas)]
  if (shard?.kind === 'interface') {
    return unique.filter(name => ['correctness', 'maintainability', 'adversarial'].includes(name))
  }
  if (isDocsOnlyShard(shard)) {
    return unique.includes('project-standards') ? ['project-standards'] : [unique[0]].filter(Boolean)
  }
  if (isMechanicalTestShard(shard)) {
    return unique.includes('testing') ? ['testing'] : [unique[0]].filter(Boolean)
  }
  return unique
}

function extractDiffBlocksByFile(diffText = '') {
  const blocks = new Map()
  let currentFile = null
  let current = []
  for (const line of String(diffText).split('\n')) {
    const m = line.match(/^diff --git a\/(.+?) b\/(.+)$/)
    if (m) {
      if (currentFile) blocks.set(currentFile, current.join('\n'))
      currentFile = m[2]
      current = [line]
    } else if (currentFile) {
      current.push(line)
    }
  }
  if (currentFile) blocks.set(currentFile, current.join('\n'))
  return blocks
}

function diffForFiles(diffText = '', files = []) {
  const blocks = extractDiffBlocksByFile(diffText)
  return files.map(file => blocks.get(file)).filter(Boolean).join('\n')
}

function interfaceDiff(diffText = '') {
  const keep = []
  let currentFile = null
  let pendingHeader = []
  let emittedHeader = false
  const interesting = /^[+-]\s*(pub\s+)?(async\s+)?(fn|struct|enum|trait|impl|type|interface|class|export\s+(function|class|type|interface|const)|const\s+\w+\s*=|function)\b/
  for (const line of String(diffText).split('\n')) {
    const fileMatch = line.match(/^diff --git a\/(.+?) b\/(.+)$/)
    if (fileMatch) {
      currentFile = fileMatch[2]
      pendingHeader = [line]
      emittedHeader = false
      continue
    }
    if (!currentFile) continue
    if (/^(index |--- |\+\+\+ |@@ )/.test(line)) {
      pendingHeader.push(line)
      continue
    }
    if (interesting.test(line)) {
      if (!emittedHeader) {
        keep.push(...pendingHeader)
        emittedHeader = true
      }
      keep.push(line)
    }
  }
  return keep.join('\n')
}

function planReviewShards(diffText = '', fileList = '', basePersonas = [], args = {}) {
  const changedFiles = normalizeChangedFiles(fileList)
  const threshold = largeDiffThreshold(args)
  const estimatedTokens = estimateDiffTokens(diffText)
  if (estimatedTokens <= threshold) {
    return {
      enabled: false,
      threshold,
      estimated_tokens: estimatedTokens,
      shards: [],
      coverage: {
        changed_files: changedFiles,
        covered_files: changedFiles,
        missing_files: [],
        duplicate_files: [],
        extra_files: [],
      },
    }
  }

  const groups = new Map()
  for (const file of changedFiles) {
    const subsystem = subsystemForFile(file)
    if (!groups.has(subsystem)) groups.set(subsystem, [])
    groups.get(subsystem).push(file)
  }

  const shards = [...groups.entries()].map(([subsystem, files]) => {
    const shard = {
      id: `subsystem:${subsystem}`,
      kind: 'subsystem',
      subsystem,
      files,
      diff_text: diffForFiles(diffText, files),
    }
    shard.personas = shardPersonas(shard, basePersonas)
    return shard
  })

  const integrator = {
    id: INTERFACE_INTEGRATOR_SHARD,
    kind: 'interface',
    subsystem: 'cross-shard-interfaces',
    files: changedFiles,
    diff_text: interfaceDiff(diffText),
  }
  integrator.personas = shardPersonas(integrator, basePersonas)
  shards.push(integrator)

  const covered = shards
    .filter(shard => shard.kind === 'subsystem')
    .flatMap(shard => shard.files)
  const counts = covered.reduce((acc, file) => {
    acc[file] = (acc[file] ?? 0) + 1
    return acc
  }, {})
  const changedSet = new Set(changedFiles)
  const coveredSet = new Set(covered)

  return {
    enabled: true,
    threshold,
    estimated_tokens: estimatedTokens,
    shards,
    coverage: {
      changed_files: changedFiles,
      covered_files: [...coveredSet],
      missing_files: changedFiles.filter(file => !coveredSet.has(file)),
      duplicate_files: Object.entries(counts).filter(([, count]) => count > 1).map(([file]) => file),
      extra_files: covered.filter(file => !changedSet.has(file)),
    },
  }
}

function summarizeShardPlan(plan) {
  if (!plan?.enabled) return plan
  return {
    ...plan,
    shards: plan.shards.map(({ diff_text: diffText, ...shard }) => ({
      ...shard,
      diff_tokens: estimateDiffTokens(diffText ?? ''),
    })),
  }
}

export {
  estimateDiffTokens,
  normalizeChangedFiles,
  largeDiffThreshold,
  shouldShardReview,
  subsystemForFile,
  shardPersonas,
  planReviewShards,
  summarizeShardPlan,
  gpt55ShouldRun,
  gpt55SkippedPersonas,
  personasRunCount,
}

// ─────────────────────────────────────────────────────────────────────────────
// SETUP_SCHEMA — Phase C (cas-7c64)
// Combined Steps 1-2 agent output: intent extraction + persona selection in one call.
// Using boolean flags per conditional persona (not a string array) gives the
// schema layer hard-type enforcement on the activation decision.
// ─────────────────────────────────────────────────────────────────────────────

export const SETUP_SCHEMA = Object.freeze({
  type: 'object',
  required: [
    'intent_summary',
    'activate_security',
    'activate_adversarial',
    'activate_performance',
    'fallow_skip_reason',
  ],
  additionalProperties: false,
  properties: {
    // Step 1: Intent — 2-3 line synthesis of what the author was trying to do
    intent_summary: {
      type: 'string',
      description: 'Goal (1 line) + Scope marker (new feature/refactor/fix/etc.) + Non-goals (optional)',
    },
    // Step 2: Conditional persona activation flags (LLM judgment, not path pattern matching)
    activate_security: {
      type: 'boolean',
      description: 'true if diff touches auth boundaries, user input parsing, or permission surfaces',
    },
    activate_adversarial: {
      type: 'boolean',
      description: 'true if diff is 50+ non-test lines AND touches CAS high-stakes modules',
    },
    activate_performance: {
      type: 'boolean',
      description: 'true if diff touches DB queries, data transforms, caching, or async hot paths',
    },
    // Fallow detection
    fallow_skip_reason: {
      type: ['string', 'null'],
      description: 'null if fallow should run; string reason if it should skip (non-JS/TS repo, etc.)',
    },
  },
})

// ─────────────────────────────────────────────────────────────────────────────
// PERSONA PROMPTS — verbatim from references/personas/ (embedded for self-containment)
// ─────────────────────────────────────────────────────────────────────────────

export const PERSONA_PROMPTS = Object.freeze({

correctness: `# Persona: correctness

## Model tier
The orchestrator selects your execution transport. Follow this persona mandate only.

## Mandate
Hunt for defects that make the changed code *wrong* — logic errors, broken execution paths, and failure modes the author did not consider. Trace execution paths, check invariants. If you cannot construct a concrete input that triggers the bug, your confidence must reflect that.

## In scope
- Off-by-one, boundary errors, slice index bugs, empty-collection edge cases.
- Null/None/undefined propagation reaching unchecked dereferences.
- Race conditions: unsynchronized shared state, check-then-act, lease/lock handling, async cancellation safety.
- Broken error handling: swallowed errors, Result ignored, retry loops without backoff/bound, partial failure leaving inconsistent state.
- Contract violations: preconditions unchecked, postconditions not upheld.
- Resource leaks: file handles, DB connections, locks, channels, temp files.
- Arithmetic bugs: integer overflow/underflow, truncation, float equality.
- Structural red-flags (Rust): bare \`.unwrap()\`/\`.expect()\` on fallible input, \`todo!()\`/\`unimplemented!()\`, \`#[allow(dead_code)]\` on new code, \`let _ = <fallible>\`.
- Structural red-flags (TypeScript): \`$EXPR as any\`, \`// @ts-ignore\` without justification, empty catch.
- Structural red-flags (Python): bare \`except:\`, \`# type: ignore\` without justification.
- Dead/unwired new public code (function, type, command, route, handler, MCP tool) with zero references.

## Out of scope
Testing → \`testing\`. Naming/duplication → \`maintainability\`. JS imports → \`fallow\`. Auth/input → \`security\`. DB/async → \`performance\`. Blast-radius → \`adversarial\`. CAS rules → \`project-standards\`.

## Calibration
0.80+: reproducible from code alone (full path traced, input identified). 0.60–0.79: sound reasoning, inference gap stated. <0.60: use residual_risks. P0 threshold: ≥ 0.50.

## Output contract
Return ONLY: {"reviewer":"correctness","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Do NOT emit prose outside the JSON envelope.`,

testing: `# Persona: testing

## Model tier
The orchestrator selects your execution transport. Follow this persona mandate only.

## Mandate
Hunt for gaps and weaknesses in test coverage of the changed code. Answer: *if this diff broke, would a test fail?* For every new/modified non-test symbol, verify a test would catch a plausible regression.

## In scope
- Missing coverage for new/modified code paths, branches, error paths, edge cases.
- Weak assertions (test runs but would pass on wrong return value).
- Over-mocking hiding integration bugs (mock returns Ok(()) unconditionally, real misuse uncaught).
- Flaky patterns: time-dependent assertions, hash-map iteration order, sleep instead of sync primitive.
- Test anti-patterns introduced: \`#[ignore]\`/\`it.skip\`/\`pytest.mark.skip\` without linked issue, commented-out assertions, \`assert true\`.
- New public API with no test file at all.

## Out of scope
Logic bugs in production code → \`correctness\`. Test-file rule-compliance → \`project-standards\`. Test duplication (often intentional) → lenient.

## Calibration
0.80+: confirmed absence by reading test files (name both production line and absent test). 0.60–0.79: coverage appears thin but may be in adjacent file.

## Output contract
Return ONLY: {"reviewer":"testing","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
\`file\`/\`line\` point at the *production* symbol with missing coverage. Most findings: \`manual\`, \`review-fixer\` or \`downstream-resolver\`. Do NOT emit prose outside the JSON envelope.`,

maintainability: `# Persona: maintainability

## Model tier
The orchestrator selects your execution transport. Follow this persona mandate only.

## Mandate
Hunt for changes that make the codebase harder to read, reason about, or extend six months from now.

## In scope
- Duplication: block that already exists elsewhere (grep), or copies a pattern the codebase already extracted.
- Naming drift: symbol uses convention conflicting with neighbors, or name misrepresents what code does.
- Dead code: branches, params, fields, imports never read or always false (grep-verified).
- Premature/broken abstraction: helper for one caller, interface with one implementor, generic with single concrete type.
- Inappropriate abstraction level: business logic in serializer, SQL in handler, UI state in store model.
- Comment rot: comment contradicts code, stale doc-comment names old param, TODO unlikely revisited.
- Oversized functions introduced in diff (400+ lines with no structure).
- Backwards-compatibility cruft without justification in new code.

## Out of scope
Logic bugs → \`correctness\`. Test quality → \`testing\`. Rule violations → \`project-standards\`. Security → \`security\`. Performance → \`performance\`. Subjective style (tabs/spaces, import order, line length).

## Calibration
0.80+: evidence in diff and surrounding code, can point at both sides. 0.60–0.79: smell present, judgment depends on inferred convention. <0.60: suppress.

## Output contract
Return ONLY: {"reviewer":"maintainability","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Most: P2/P3, \`advisory\` or \`manual\`. P0/P1 rare. Do NOT emit prose outside the JSON envelope.`,

'project-standards': `# Persona: project-standards

## Model tier
The orchestrator selects your execution transport. Follow this persona mandate only.

## Mandate
Hunt for violations of the project's explicit, enforceable standards — CAS rules from \`mcp__cas__rule\` plus \`CLAUDE.md\`/\`AGENTS.md\` conventions. Enforce what *this project* has decided. Do not invent rules.

## In scope
- CAS rule compliance: run \`mcp__cas__rule action=list\` at start; check active rules against changed files; cite rule ID in title and rule text in evidence.
- \`CLAUDE.md\`/\`AGENTS.md\` conventions enforceable objectively.
- Managed-file headers: file with \`managed_by: cas\` modified without going through generator.
- Module-boundary rules when documented.
- Forbidden API calls listed in rules (e.g., "no \`println!\` in library code", "no \`TodoWrite\`").
- Naming conventions when codified in a rule.

## Out of scope
Logic bugs → \`correctness\`. Test coverage → \`testing\`. Subjective readability without stated rule → \`maintainability\`. Inactive/draft/archived rules.

## Calibration
0.80+: explicit rule + clear violation (include both rule body and code in evidence). 0.60–0.79: rule applies but wording ambiguous. <0.60: not a stated rule — suppress.

## Output contract
Return ONLY: {"reviewer":"project-standards","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Rule ID in title prefix. Do NOT emit prose outside the JSON envelope.`,

security: `# Persona: security

## Model tier
Run as the **Claude Opus** cross-vendor reviewer. Do not inherit the caller's model.

## Activation (confirmed by caller before dispatch)
Touches authentication boundaries, user input parsing/deserialization, or permission surfaces (auth/session/token, HTTP/socket/CLI input, authorization checks, factory tool restrictions).

## Mandate
Hunt for exploitable defects — where malicious/malformed input, stolen credential, or authorization misuse lets an attacker read, write, or execute something they should not. Think in threat models: attacker input → boundary → target. Evidence-grounded, reproducible-from-code reasoning required.

## In scope
Injection (SQL, command, path traversal, template, header). Broken authentication (missing/weak session validation, non-constant-time comparison, weak hashing). Broken authorization (missing permission check, IDOR, capability upgrade, jail escape). Sensitive data exposure (hardcoded secrets, secrets logged, PII to analytics). Cryptographic misuse. Deserialization of untrusted input. SSRF/open redirect. CAS-specific: new MCP tool without jail/permission checks, hook with elevated privileges, worker path influencing supervisor state. TOCTOU on permission checks.

## Out of scope
Pure correctness with no threat model → \`correctness\`. Attacker-controlled DoS → here if input-controlled, else \`performance\`. Theoretical with no reachable path.

## Calibration
0.80+: trace attacker input to sink, boundary check demonstrably absent. 0.60–0.79: pattern present, trust boundary unclear. <0.60: suppress.

## Output contract
Return ONLY: {"reviewer":"security","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Default P0/P1, \`owner:human\`, \`manual\` autofix. Do NOT emit prose outside the JSON envelope.`,

performance: `# Persona: performance

## Model tier
The orchestrator selects your execution transport. Follow this persona mandate only.

## Activation (confirmed by caller before dispatch)
Touches DB queries, data transforms on potentially large inputs, caching, or async code paths.

## Mandate
Hunt for code that will be slower, more wasteful, or less scalable than a reasonable alternative. Care about asymptotic complexity, unbounded work, async pitfalls. Every finding must point at a concrete cost scenario (input size, call frequency, known hot path).

## In scope
N+1 queries. Unbounded queries/collections (SELECT without LIMIT, find_all without pagination, unbounded channel). Missing/wrong indexes. Blocking work in async runtime (std::fs, std::thread::sleep in tokio). Lock contention / await-while-holding-lock (Mutex held across .await). Algorithmic complexity (O(n²) where O(n) straightforward). Cache invalidation bugs (stale return, missing key invalidation, stampede TTL). Wasteful allocation in hot paths (String::from/format! in tight loop, clone in every iteration). Thundering herd/retry storms (no jitter, backoff multiplier=1). Connection pool misuse.

## Out of scope
Correctness → \`correctness\`. Attacker-controlled DoS → \`security\`. Test speed → \`testing\`. Style-level "this could be a one-liner". Microbenchmarks unless explicitly a hot path.

## Calibration
0.80+: traced data flow + concrete cost scenario in evidence. 0.60–0.79: pattern present, frequency uncertain. <0.60: suppress.

## Output contract
Return ONLY: {"reviewer":"performance","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Most: P1/P2, \`manual\` or \`gated_auto\`. Do NOT emit prose outside the JSON envelope.`,

adversarial: `# Persona: adversarial

## Model tier
The orchestrator selects your execution transport. Follow this persona mandate only.

## Activation (confirmed by caller before dispatch)
Diff is 50+ changed non-test lines AND touches CAS high-stakes modules (close_ops, verify_ops, factory coordination, SQLite stores, hook system, MCP dispatch). Skip for diffs under 20 non-test lines regardless of files.

## Mandate
Red-team reader. Ask: *what is the worst this change could plausibly do, and how would we know?* Surface risks the other personas miss because they are in-lane. Reason about blast radius, reversibility, multi-component interactions, failures that only appear under concurrent factory sessions or production state.

## In scope
Blast-radius misjudgment ("small" refactor changes function used by 30 callers). Reversibility gaps (migration without rollback, destructive op without dry-run, schema change breaking older processes). Invariant erosion (task in pending_verification cannot be closed — bypassed). Cross-component coupling (implicit assumption another module doesn't guarantee). State machine corruption (unmapped state, missing guard). Concurrency traps at system level (two workers racing on lease, supervisor/worker seeing different task state). Failure-mode asymmetry (error path leaves artifacts, ghost tasks, leaked processes). Operational surprises (log on hot path, metric break). Lessons from CAS project memory: if memory records a past incident class this diff reopens, call it out.

## Out of scope
Narrow single-lane findings → owning persona. Aesthetic concerns. Speculation untethered from diff.

## Calibration
0.80+: specific invariant broken + specific historical incident class from CAS memory. 0.60–0.79: plausible, triggering condition requires production state. <0.60: suppress.

## Output contract
Return ONLY: {"reviewer":"adversarial","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Almost always \`manual\`, \`owner:human\` or \`downstream-resolver\`. Severity = blast radius, not likelihood. Do NOT emit prose outside the JSON envelope.`,

fallow: `# Persona: fallow

## Model tier
Your job is *adapter, not auteur* — run a deterministic CLI and translate its output faithfully. The orchestrator selects your execution transport.

## Mandate
Run \`fallow audit\` and translate each finding to a ReviewerOutput Finding. Fallow findings have mechanically-derived truth value — confidence is fixed at 0.95 for new findings, 0.80 for pre-existing.

## Skip rules (return clean envelope with residual_risks entry):
1. No JS/TS surface: no package.json or tsconfig.json at repo root (excluding node_modules).
2. No JS/TS files in diff: zero .ts/.tsx/.js/.jsx/.mjs/.cjs/.vue/.svelte/.astro/.mdx in changed_files.
3. Fallow not available: \`command -v fallow\` and \`npx fallow --version\` both fail.
4. Fallow runtime error (exit code 2).

## Run command
\`\`\`bash
fallow audit --format json --quiet --explain --base <base_sha>
\`\`\`
Exit 0 (pass) or 1 (issues found) are normal; only 2 is error.

## JSON → Finding translation
file→file (relative verbatim), line→start_line, issue-type→title "[fallow] <type>: <symbol>" (≤100 chars). error→P1, warning→P2, info→P3. auto_fixable→safe_auto/review-fixer; else manual/downstream-resolver. pre_existing: true if fallow attribution shows introduced: false. Confidence: 0.95 introduced, 0.80 pre-existing. Never below 0.60. residual_risks: include aggregate fallow verdict (pass/warn/fail), elapsed time, max cyclomatic.

## Output contract
Return ONLY: {"reviewer":"fallow","findings":[...],"residual_risks":[...],"testing_gaps":[]}
reviewer MUST be "fallow" (lowercase). Do NOT emit prose outside the JSON envelope.`,

})
