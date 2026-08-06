// cas-code-review.js — production Workflow for cas-code-review Steps 1-4
//
// Phase C of EPIC cas-b667, extended by cas-33f1. Handles Step 1 (intent
// extraction), Step 2 (persona selection), Step 3 (parallel persona dispatch,
// size-gated sharding), and Step 4 (deterministic JS merge). Tiny-diff bypass
// and Step 5 CAS/mode integration stay in the skill wrapper (SKILL.md).
//
// Self-contained: Workflow scripts run in a custom runtime that does not
// support ES module import statements. All helpers are inlined.
// For test imports of constants, see cas-code-review-constants.js.
//
// Called by the cas-code-review skill:
//   Workflow({ name: 'cas-code-review', args: {
//     diff_text,           // full git diff (pre-fetched by skill)
//     file_list,           // newline-separated changed file paths
//     base_sha,            // base commit SHA
//     commit_log,          // commit messages for intent extraction
//     task_context,        // optional CAS task context for intent extraction
//     large_diff_token_threshold, // optional sharding threshold (default 12000)
//     mode,                // 'interactive'|'report-only'|'headless'|'autofix'
//     task_id,             // optional CAS task ID
//   }})
//
// Returns: { status, residual, pre_existing, dropped, activation, stats }

export const meta = {
  name: 'cas-code-review',
  description: 'cas-code-review Steps 1-4: intent extraction, persona selection, sharded dispatch, deterministic merge',
  phases: [
    { title: 'Resolve', detail: 'validate args + fallow pre-check' },
    { title: 'Review', detail: 'parallel persona dispatch, sharded for large diffs (Codex + Claude diversity lane)' },
    { title: 'Merge', detail: 'deterministic 7-step merge (pure JS, no LLM)' },
  ],
}

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

const ALWAYS_ON_PERSONAS = ['correctness', 'testing', 'maintainability', 'project-standards']
const CODEX_PERSONA_MODEL = 'gpt-5.6-sol'
const CODEX_PERSONA_EFFORT = 'medium'
const CODEX_PERSONA_TIMEOUT_SECONDS = 600
const CODEX_SCHEMA_RETRIES = 2
const CODEX_TIMEOUT_RETRIES = 1
const CODEX_MAX_CONCURRENCY = 4
const CLAUDE_DIVERSITY_PERSONA = 'security'

// ─────────────────────────────────────────────────────────────────────────────
// SCHEMA — mirrors ReviewerOutput + Finding from crates/cas-types/src/code_review.rs
// ─────────────────────────────────────────────────────────────────────────────

const FINDING_SCHEMA = {
  type: 'object',
  required: ['title','severity','file','line','why_it_matters','autofix_class','owner','confidence','evidence','pre_existing','suggested_fix','requires_verification'],
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
    suggested_fix:        { type: ['string', 'null'] },
    requires_verification:{ type: ['boolean', 'null'] },
  },
}

const REVIEWER_OUTPUT_SCHEMA = {
  type: 'object',
  required: ['reviewer', 'findings', 'residual_risks', 'testing_gaps', 'skipped_reason'],
  additionalProperties: false,
  properties: {
    reviewer:       { type: 'string' },
    findings:       { type: 'array', items: FINDING_SCHEMA },
    residual_risks: { type: ['array', 'null'], items: { type: 'string' } },
    testing_gaps:   { type: ['array', 'null'], items: { type: 'string' } },
    skipped_reason: { type: ['string', 'null'] },
  },
}

// ─────────────────────────────────────────────────────────────────────────────
// PERSONA PROMPTS — condensed from references/personas/*.md
// Full verbatim versions in cas-code-review-constants.js (importable by tests)
// ─────────────────────────────────────────────────────────────────────────────

const PERSONA_PROMPTS = {

correctness: `# Persona: correctness
The orchestrator selects your execution transport. Follow this persona mandate only.

Hunt for defects that make the changed code wrong — logic errors, broken execution paths, failure modes the author did not consider. Trace the full execution path: inputs, branches, early returns, error propagation, invariants. If you cannot construct a concrete input that triggers the bug, confidence must reflect that.

In scope: off-by-one/boundary errors, None/null propagation to unchecked dereferences, race conditions (check-then-act, lease/lock handling, async cancellation), broken error handling (swallowed errors, Result ignored, retry without backoff/bound), contract violations, resource leaks (file handles, DB connections, locks, temp files), arithmetic bugs (overflow, truncation, float equality). Structural red-flags: Rust bare .unwrap()/.expect() on fallible input, todo!()/unimplemented!(), #[allow(dead_code)] on new code, let _ = <fallible>. TypeScript: $EXPR as any, // @ts-ignore without justification, empty catch. Dead/unwired new public code with zero references.

Out of scope: testing→testing, naming/duplication→maintainability, JS imports→fallow, auth/input→security, DB/async hot paths→performance, blast-radius→adversarial, CAS rules→project-standards.

Output ONLY: {"reviewer":"correctness","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
confidence ≥ 0.80: reproducible from code alone. 0.60–0.79: sound, inference gap stated. <0.60: use residual_risks. P0 threshold: ≥ 0.50. No prose outside the JSON envelope.`,

testing: `# Persona: testing
The orchestrator selects your execution transport. Follow this persona mandate only.

Hunt for gaps and weaknesses in test coverage of the changed code. Answer: if this diff broke, would a test fail? For every new/modified non-test symbol, verify a test would catch a plausible regression.

In scope: missing coverage for new/modified paths, branches, error paths; weak assertions (would pass on wrong return value); over-mocking hiding integration bugs; flaky patterns (time-dependent, hash-map iteration order, sleep instead of sync primitive); test anti-patterns introduced (#[ignore]/it.skip/pytest.mark.skip without linked issue, commented-out assertions, assert true); new public API with no test file.

Out of scope: logic bugs in production code→correctness, test-file rule-compliance→project-standards. Test duplication is often intentional — lenient.

Output ONLY: {"reviewer":"testing","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
file/line point at the production symbol with missing coverage. Most: manual, review-fixer or downstream-resolver. confidence 0.80+: confirmed absence by reading test files. No prose outside JSON.`,

maintainability: `# Persona: maintainability
The orchestrator selects your execution transport. Follow this persona mandate only.

Hunt for changes that make the codebase harder to read, reason about, or extend six months from now.

In scope: duplication (block exists elsewhere — grep the repo); naming drift (convention conflicts with neighbors); dead code (branches, params, fields, imports never read — grep-verified); premature/broken abstraction (helper for one caller, interface with one implementor); inappropriate abstraction level (business logic in serializer, SQL in handler); comment rot (contradicts code, stale doc-comment names old param); oversized functions introduced (400+ lines); backwards-compatibility cruft without justification in new code.

Out of scope: logic bugs→correctness, test quality→testing, rule violations→project-standards, security→security, performance→performance. Subjective style (tabs/spaces, import order, line length) — do not flag.

Output ONLY: {"reviewer":"maintainability","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Most: P2/P3, advisory or manual. P0/P1 rare. No prose outside JSON.`,

'project-standards': `# Persona: project-standards
The orchestrator selects your execution transport. Follow this persona mandate only.

Hunt for violations of the project's explicit, enforceable standards — CAS rules from mcp__cas__rule plus CLAUDE.md/AGENTS.md conventions. Enforce what this project has decided. Do not invent rules.

In scope: CAS rule compliance (run mcp__cas__rule action=list at start; check active rules against changed files; cite rule ID in title prefix, rule body in evidence); CLAUDE.md/AGENTS.md conventions enforceable objectively; managed-file headers (file with managed_by: cas modified without going through generator); module-boundary rules when documented; forbidden API calls listed in rules (e.g., no println! in library code, no TodoWrite); naming conventions when codified in a rule.

Out of scope: logic bugs→correctness, test coverage→testing, subjective readability without stated rule→maintainability. Inactive/draft/archived rules.

Output ONLY: {"reviewer":"project-standards","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Rule ID in title prefix (e.g., "rule-1234: ..."). confidence 0.80+: explicit rule + clear violation. <0.60: suppress. No prose outside JSON.`,

security: `# Persona: security
Run as the Claude Opus cross-vendor reviewer. Do not inherit caller model.
ACTIVATION: Confirmed by caller — diff touches auth boundaries, user input parsing/deserialization, or permission surfaces.

Hunt for exploitable defects — where malicious/malformed input, stolen credential, or authorization misuse lets an attacker read, write, or execute something they should not. Think in threat models: attacker input → boundary → target. Evidence-grounded, reproducible-from-code reasoning required.

In scope: injection (SQL, command, path traversal, template, header); broken authentication (missing/weak session validation, non-constant-time comparison, weak hashing); broken authorization (missing permission check, IDOR, capability upgrade, jail escape); sensitive data exposure (hardcoded secrets, secrets logged, PII to analytics); cryptographic misuse; deserialization of untrusted input; SSRF/open redirect; CAS-specific: new MCP tool without jail/permission checks, hook with elevated privileges, worker path influencing supervisor state; TOCTOU on permission checks.

Out of scope: pure correctness with no threat model→correctness. Theoretical with no reachable path.

Output ONLY: {"reviewer":"security","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Default P0/P1, owner:human, manual autofix. confidence 0.80+: trace attacker input to sink. <0.60: suppress. No prose outside JSON.`,

performance: `# Persona: performance
The orchestrator selects your execution transport. Follow this persona mandate only.
ACTIVATION: Confirmed by caller — diff touches DB queries, data transforms on large inputs, caching, or async code paths.

Hunt for code that will be slower, more wasteful, or less scalable than a reasonable alternative. Care about asymptotic complexity, unbounded work, async pitfalls. Every finding must point at a concrete cost scenario.

In scope: N+1 queries; unbounded queries/collections (SELECT without LIMIT, find_all without pagination); missing/wrong indexes; blocking work in async runtime (std::fs, std::thread::sleep in tokio); lock contention / await-while-holding-lock (Mutex held across .await); algorithmic complexity (O(n²) where O(n) straightforward); cache invalidation bugs; wasteful allocation in hot paths; thundering herd/retry storms (no jitter, backoff multiplier=1); connection pool misuse.

Out of scope: correctness→correctness, attacker-controlled DoS→security, test speed→testing.

Output ONLY: {"reviewer":"performance","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Most: P1/P2, manual or gated_auto. confidence 0.80+: traced data flow + concrete cost scenario. <0.60: suppress. No prose outside JSON.`,

adversarial: `# Persona: adversarial
The orchestrator selects your execution transport. Follow this persona mandate only.
ACTIVATION: Confirmed by caller — 50+ changed non-test lines AND touches CAS high-stakes modules (close_ops, verify_ops, factory coordination, SQLite stores, hook system, MCP dispatch). Skip for diffs under 20 non-test lines.

Red-team reader. Ask: what is the worst this change could plausibly do, and how would we know? Surface risks the other personas miss because they are in-lane. Reason about blast radius, reversibility, multi-component interactions, failures that appear only under concurrent factory sessions or production state.

In scope: blast-radius misjudgment (small refactor changes function used by 30 callers); reversibility gaps (migration without rollback, destructive op without dry-run); invariant erosion (existing invariants weakened — e.g., task in pending_verification cannot be closed); cross-component coupling (implicit assumption another module doesn't guarantee); state machine corruption (unmapped state, missing guard, exhaustive match missing arm); concurrency traps at system level (two workers racing on lease, supervisor/worker seeing different task state); failure-mode asymmetry (error path leaves ghost tasks, leaked processes); operational surprises (log on hot path, metric break); if CAS project memory records a past incident class this diff reopens, call it out explicitly.

Out of scope: narrow single-lane findings→owning persona, aesthetic concerns, speculation untethered from diff.

Output ONLY: {"reviewer":"adversarial","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
Almost always manual, owner:human or downstream-resolver. Severity = blast radius, not likelihood. confidence 0.80+: specific invariant broken + historical incident class. <0.60: suppress. No prose outside JSON.`,

fallow: `# Persona: fallow
Adapter, not auteur — run the CLI and translate output. The orchestrator selects your execution transport.
ACTIVATION: JS/TS repo with JS/TS files in diff (pre-checked by caller).

Skip rules — return clean envelope with residual_risks entry:
1. No package.json or tsconfig.json at repo root (outside node_modules).
2. No .ts/.tsx/.js/.jsx/.mjs/.cjs/.vue/.svelte/.astro/.mdx in changed_files.
3. fallow CLI not available (command -v fallow and npx fallow --version both fail).
4. Fallow runtime error (exit code 2).

Run: fallow audit --format json --quiet --explain --base <base_sha>
Exit 0 (pass) or 1 (issues) are normal; only 2 is error.

Translation: file→file (relative), line→start_line, issue-type→title "[fallow] <type>: <symbol>" (≤100 chars). error→P1, warning→P2, info→P3. auto_fixable→safe_auto/review-fixer; else manual/downstream-resolver. pre_existing: true if fallow attribution shows introduced: false. Confidence: 0.95 introduced, 0.80 pre-existing.

Output ONLY: {"reviewer":"fallow","findings":[...],"residual_risks":[...],"testing_gaps":[]}
reviewer MUST be "fallow". No prose outside JSON.`,

}

// ─────────────────────────────────────────────────────────────────────────────
// MERGE PIPELINE — Phase A validated (merge-findings.js, 30 unit tests)
// Inlined here since Workflow scripts cannot use import statements.
// ─────────────────────────────────────────────────────────────────────────────

const OWNER_RANK = { 'human': 2, 'downstream-resolver': 1, 'review-fixer': 0 }

const FINDING_REQUIRED_FIELDS = [
  'title', 'severity', 'file', 'line', 'why_it_matters', 'autofix_class',
  'owner', 'confidence', 'evidence', 'pre_existing',
]
const FINDING_ALLOWED_FIELDS = new Set([
  ...FINDING_REQUIRED_FIELDS, 'suggested_fix', 'requires_verification',
])
const VALID_SEVERITIES = new Set(['P0', 'P1', 'P2', 'P3'])
const VALID_AUTOFIX_CLASSES = new Set(['safe_auto', 'gated_auto', 'manual', 'advisory'])
const VALID_OWNERS = new Set(['review-fixer', 'downstream-resolver', 'human'])

function findingValidationErrors(finding) {
  if (!finding || typeof finding !== 'object' || Array.isArray(finding)) {
    return ['finding must be an object']
  }

  const errors = []
  for (const field of FINDING_REQUIRED_FIELDS) {
    if (!Object.hasOwn(finding, field)) errors.push(`missing required field: ${field}`)
  }
  if (Object.hasOwn(finding, 'title') &&
      (typeof finding.title !== 'string' || finding.title.length > 100)) {
    errors.push('title must be a string of at most 100 characters')
  }
  if (Object.hasOwn(finding, 'severity') && !VALID_SEVERITIES.has(finding.severity)) {
    errors.push('severity must be one of P0, P1, P2, P3')
  }
  if (Object.hasOwn(finding, 'file') && typeof finding.file !== 'string') {
    errors.push('file must be a string')
  }
  if (Object.hasOwn(finding, 'line') &&
      (!Number.isInteger(finding.line) || finding.line < 1)) {
    errors.push('line must be an integer greater than or equal to 1')
  }
  if (Object.hasOwn(finding, 'why_it_matters') && typeof finding.why_it_matters !== 'string') {
    errors.push('why_it_matters must be a string')
  }
  if (Object.hasOwn(finding, 'autofix_class') &&
      !VALID_AUTOFIX_CLASSES.has(finding.autofix_class)) {
    errors.push('autofix_class must be safe_auto, gated_auto, manual, or advisory')
  }
  if (Object.hasOwn(finding, 'owner') && !VALID_OWNERS.has(finding.owner)) {
    errors.push('owner must be review-fixer, downstream-resolver, or human')
  }
  if (Object.hasOwn(finding, 'confidence') &&
      (typeof finding.confidence !== 'number' || !Number.isFinite(finding.confidence) ||
       finding.confidence < 0 || finding.confidence > 1)) {
    errors.push('confidence must be a number between 0.0 and 1.0')
  }
  if (Object.hasOwn(finding, 'evidence') &&
      (!Array.isArray(finding.evidence) || finding.evidence.length < 1 ||
       finding.evidence.some(item => typeof item !== 'string'))) {
    errors.push('evidence must be a non-empty array of strings')
  }
  if (Object.hasOwn(finding, 'pre_existing') && typeof finding.pre_existing !== 'boolean') {
    errors.push('pre_existing must be a boolean')
  }
  if (Object.hasOwn(finding, 'suggested_fix') && typeof finding.suggested_fix !== 'string') {
    errors.push('suggested_fix must be a string when present')
  }
  if (Object.hasOwn(finding, 'requires_verification') &&
      typeof finding.requires_verification !== 'boolean') {
    errors.push('requires_verification must be a boolean when present')
  }
  for (const field of Object.keys(finding)) {
    if (!FINDING_ALLOWED_FIELDS.has(field)) errors.push(`unexpected field: ${field}`)
  }
  return errors
}

function fingerprint(f) {
  const title = f.title.toLowerCase().replace(/[^a-z0-9]/g, ' ').replace(/\s+/g, ' ').trim()
  const bucket = Math.floor(f.line / 3)
  return `${f.file}|${bucket}|${title}`
}

function mergeFindings(reviewerOutputs) {
  const collected = []
  reviewerOutputs.filter(Boolean).forEach((output, index) => {
    const reviewer = typeof output.reviewer === 'string'
      ? output.reviewer
      : `unknown-reviewer-${index}`
    const findings = Array.isArray(output.findings) ? output.findings : []
    for (const finding of findings) collected.push({ reviewer, finding })
  })

  const dropped = []
  const valid = []
  for (const item of collected) {
    const validationErrors = findingValidationErrors(item.finding)
    if (validationErrors.length > 0) {
      dropped.push({
        reviewer: item.reviewer,
        reason: 'schema_validation_failed',
        validation_errors: validationErrors,
        finding: item.finding,
      })
    } else {
      valid.push(item)
    }
  }

  const gated = []
  for (const item of valid) {
    const threshold = item.finding.severity === 'P0' ? 0.50 : 0.60
    if (item.finding.confidence >= threshold) {
      gated.push(item.finding)
    } else {
      dropped.push({
        reviewer: item.reviewer,
        reason: 'confidence_below_threshold',
        threshold,
        finding: item.finding,
      })
    }
  }
  const byFp = new Map()
  for (const f of gated) {
    const fp = fingerprint(f)
    if (!byFp.has(fp)) {
      byFp.set(fp, { finding: { ...f }, count: 1 })
    } else {
      const entry = byFp.get(fp)
      entry.count++
      const boosted = Math.min(1.0, entry.finding.confidence + 0.10)
      const currentRank = OWNER_RANK[entry.finding.owner] ?? 0
      const incomingRank = OWNER_RANK[f.owner] ?? 0
      entry.finding = {
        ...entry.finding,
        confidence: boosted,
        owner: incomingRank > currentRank ? f.owner : entry.finding.owner,
      }
    }
  }
  const deduped = Array.from(byFp.values()).map(e => e.finding)
  const residual = deduped.filter(f => !f.pre_existing)
  const pre_existing = deduped.filter(f => f.pre_existing)
  const SEV_ORDER = { P0: 0, P1: 1, P2: 2, P3: 3 }
  residual.sort((a, b) =>
    (SEV_ORDER[a.severity] - SEV_ORDER[b.severity]) || (b.confidence - a.confidence)
  )
  return { residual, pre_existing, dropped }
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPERS
// ─────────────────────────────────────────────────────────────────────────────

function buildPersonaPrompt(name, diffText, fileList, intentSummary, baseSha) {
  const body = PERSONA_PROMPTS[name] ?? `# Persona: ${name}\n(Unknown persona — emit empty envelope)`
  return `${body}

---

## Change being reviewed

**Intent:** ${intentSummary}

**Base SHA:** ${baseSha}

**Changed files:**
${fileList}

**Full diff:**
\`\`\`diff
${diffText}
\`\`\`

**Findings contract** — output MUST be a single JSON object:
{"reviewer":"${name}","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
The canonical schema for Finding and ReviewerOutput is in references/findings-schema.md.
Each finding needs: title (≤100 chars), severity (P0-P3), file (relative path),
line (1-based int), why_it_matters, autofix_class (safe_auto|gated_auto|manual|advisory),
owner (review-fixer|downstream-resolver|human), confidence (0.0-1.0),
evidence (array ≥1 code-grounded string), pre_existing (bool).
Do NOT emit any prose outside the JSON envelope.`
}

function buildIndependentPrompt(diffText, fileList, intentSummary, baseSha) {
  return `# Persona: gpt-5.6-sol:independent
Review focus: independent broad read. Look for important correctness, testing, maintainability, security, performance, or integration issues missed by lane-specific reviewers. Avoid nitpicks.

## Review target

Intent:
${intentSummary}

Base SHA:
${baseSha}

Changed files:
${fileList}

Literal diff:
\`\`\`diff
${diffText}
\`\`\`

Output ONLY a JSON object matching:
{"reviewer":"gpt-5.6-sol:independent","findings":[...],"residual_risks":[...],"testing_gaps":[...]}
If you find nothing, return an empty findings array and name the review target you inspected in residual_risks. No prose outside JSON.`
}

function buildCodexReviewerShimPrompt(name, reviewerPrompt) {
  const schemaJson = JSON.stringify(REVIEWER_OUTPUT_SCHEMA, null, 2)
  return `# Codex reviewer transport shim
You are a thin transport adapter, not a reviewer. Execute the supplied reviewer prompt with Codex, validate its final JSON, and return the parsed object unchanged.

Transport contract:
- Model: ${CODEX_PERSONA_MODEL}
- Reasoning effort: ${CODEX_PERSONA_EFFORT}
- Sandbox: read-only
- Per-process timeout: ${CODEX_PERSONA_TIMEOUT_SECONDS} seconds
- Schema-mismatch retry budget: ${CODEX_SCHEMA_RETRIES}
- Timeout retry budget: ${CODEX_TIMEOUT_RETRIES}

Procedure:
1. Create a private temporary directory. Write the reviewer prompt and the exact REVIEWER_OUTPUT_SCHEMA below to separate files. Never splice the prompt or diff into a shell command.
2. Require \`codex\` and the portable shell primitives with \`command -v codex\`, \`command -v sleep\`, \`command -v kill\`, and \`command -v wait\`. If any are unavailable, do not run an unbounded process; return the skipped envelope from step 7 with the missing command or primitive named.
3. Run Codex with this exact portable shell-watchdog shape (the surrounding adapter may choose its own variable names, but must preserve the lifecycle):
   \`\`\`sh
   TIMEOUT_MARKER="$TMP_DIR/timed-out"
   codex exec -s read-only -m ${CODEX_PERSONA_MODEL} -c model_reasoning_effort=${CODEX_PERSONA_EFFORT} --output-schema "$SCHEMA_FILE" --output-last-message "$OUTPUT_FILE" --color never -C "$PWD" - < "$PROMPT_FILE" &
   CODEX_PID=$!
   (
     sleep ${CODEX_PERSONA_TIMEOUT_SECONDS}
     if kill -0 "$CODEX_PID" 2>/dev/null; then
       : > "$TIMEOUT_MARKER"
       kill -TERM "$CODEX_PID" 2>/dev/null || true
       sleep 5
       kill -KILL "$CODEX_PID" 2>/dev/null || true
     fi
   ) &
   WATCHDOG_PID=$!
   CODEX_EXIT=0
   wait "$CODEX_PID" || CODEX_EXIT=$?
   if kill -0 "$WATCHDOG_PID" 2>/dev/null; then
     kill "$WATCHDOG_PID" 2>/dev/null || true
   fi
   wait "$WATCHDOG_PID" 2>/dev/null || true
   \`\`\`
   Classify timeout from the existence of TIMEOUT_MARKER, not only the process exit code. This shell watchdog is the timeout mechanism; do not call GNU \`timeout\` or assume a platform-specific path.
4. Parse OUTPUT_FILE as one JSON object and validate it against REVIEWER_OUTPUT_SCHEMA. The codex --output-schema boundary is required, but you must still parse the saved final message before returning it.
5. On a parse or schema mismatch, retry at most ${CODEX_SCHEMA_RETRIES} times and append the concrete validation errors to the retry prompt. Schema mismatch retries do not consume the timeout retry budget.
6. When TIMEOUT_MARKER records a timeout, retry at most ${CODEX_TIMEOUT_RETRIES} time. Timeout retries do not consume the schema-mismatch retry budget.
7. On a missing timeout primitive, missing codex, authentication failure, another execution error, or exhausted retries, return:
   {"reviewer":"${name}","findings":[],"skipped_reason":"<specific transport failure>","residual_risks":[],"testing_gaps":[]}
8. Always remove the temporary directory. Return only the parsed ReviewerOutput object through the schema tool; do not add commentary or independently review the diff.

REVIEWER_OUTPUT_SCHEMA:
\`\`\`json
${schemaJson}
\`\`\`

REVIEWER PROMPT:
\`\`\`text
${reviewerPrompt}
\`\`\``
}

async function dispatchReviewPersona(name, reviewerPrompt, label) {
  if (name === CLAUDE_DIVERSITY_PERSONA) {
    return agent(reviewerPrompt, {
      label,
      phase: 'Review',
      schema: REVIEWER_OUTPUT_SCHEMA,
      model: 'opus',
      effort: 'medium',
    })
  }

  return agent(buildCodexReviewerShimPrompt(name, reviewerPrompt), {
    label,
    phase: 'Review',
    schema: REVIEWER_OUTPUT_SCHEMA,
    // Haiku performs transport-only Bash/parse work; Codex performs the review.
    model: 'haiku',
  })
}

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

function stripNullValues(value) {
  if (Array.isArray(value)) return value.map(stripNullValues)
  if (!value || typeof value !== 'object') return value
  return Object.fromEntries(
    Object.entries(value)
      .filter(([, nested]) => nested !== null)
      .map(([key, nested]) => [key, stripNullValues(nested)])
  )
}

function skippedPersonaResults(outputs = []) {
  return outputs.flatMap(output => {
    if (typeof output?.skipped_reason !== 'string' || !output.skipped_reason.trim()) return []
    return [{
      reviewer: output.reviewer,
      reason: output.skipped_reason,
    }]
  })
}

function personasRunCount(outputs = []) {
  return outputs.filter(output => !output?.skipped_reason).length
}

/**
 * cas-acf83 (GH #108): the execution block the close gate reads.
 *
 * `residual: []` is ambiguous on its own — it is what a clean review returns
 * AND what a review returns when every persona failed to launch. When the
 * Codex transport ran out of credits, the second kind sailed through the close
 * gate as if it were the first. This states which one happened, so an absent
 * verdict can never be mistaken for a passing one.
 *
 * `personas_failed` names each persona and why, so a launch failure is
 * diagnosable from the envelope alone.
 */
function buildExecution({
  personasRun = 0,
  skippedPersonas = [],
  skippedReason = null,
  requiredMissing = [],
} = {}) {
  return {
    personas_run: personasRun,
    personas_failed: skippedPersonas.map(s => `${s.reviewer}: ${s.reason}`),
    // cas-acf83: `personas_run > 0` is too weak alone. Every always-on persona
    // runs on the Codex transport (only `security` is Claude-hosted), so one
    // outage takes out all four mandatory lanes while a single surviving
    // persona would otherwise report a "successful" run. Name the missing
    // mandatory lanes so the close gate can refuse a partial review.
    required_personas_missing: requiredMissing,
    ...(skippedReason ? { skipped_reason: skippedReason } : {}),
  }
}

function incompleteAlwaysOnPersonas(skippedPersonas = []) {
  return [...new Set(
    skippedPersonas
      .map(skipped => skipped.reviewer)
      .filter(reviewer => ALWAYS_ON_PERSONAS.includes(reviewer))
  )]
}

/**
 * cas-acf83 (GH #108): mandatory lanes with no verdict, by SET DIFFERENCE.
 *
 * Deliberately not derived from the skip records: those are self-reports from
 * the very party that failed, keyed on a `reviewer` name it chooses. A persona
 * whose transport died before it could name itself — or that returned nothing
 * at all — leaves no skip record, and a skip-record scan would then report a
 * complete review. Asking instead "which always-on persona produced a usable
 * verdict?" cannot miss a lane, and a mislabelled verdict counts as missing,
 * which errs toward rejecting the close.
 */
function missingMandatoryPersonas(dispatched = []) {
  const delivered = new Set(
    dispatched
      .filter(entry => entry?.output && !entry.output.skipped_reason)
      // The name the ORCHESTRATOR dispatched under, never the one the result
      // claims: a persona that mislabels itself, or a shim that returns a
      // malformed reviewer field, must not be able to vouch for a lane.
      .map(entry => entry.persona)
  )
  return ALWAYS_ON_PERSONAS.filter(persona => !delivered.has(persona))
}

async function pipelineWithConcurrency(items, worker, maxConcurrency = CODEX_MAX_CONCURRENCY) {
  const results = []
  for (let offset = 0; offset < items.length; offset += maxConcurrency) {
    const batch = items.slice(offset, offset + maxConcurrency)
    results.push(...await pipeline(batch, worker))
  }
  return results
}

// ─────────────────────────────────────────────────────────────────────────────
// LARGE-DIFF SHARDING HELPERS — inline copy of cas-code-review-constants.js
// Runtime Workflow scripts cannot import ES modules.
// ─────────────────────────────────────────────────────────────────────────────

const DEFAULT_LARGE_DIFF_TOKEN_THRESHOLD = 12000
const INTERFACE_INTEGRATOR_SHARD = 'interface-integrator'

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

// ─────────────────────────────────────────────────────────────────────────────
// SETUP_SCHEMA — Phase C (inlined; Workflow scripts cannot import from ES modules)
// Combined Steps 1-2 agent output: intent extraction + persona selection in one call.
// ─────────────────────────────────────────────────────────────────────────────

const SETUP_SCHEMA = {
  type: 'object',
  required: ['intent_summary', 'activate_security', 'activate_adversarial', 'activate_performance', 'fallow_skip_reason'],
  additionalProperties: false,
  properties: {
    intent_summary:       { type: 'string' },
    activate_security:    { type: 'boolean' },
    activate_adversarial: { type: 'boolean' },
    activate_performance: { type: 'boolean' },
    fallow_skip_reason:   { type: ['string', 'null'] },
  },
}

// ─────────────────────────────────────────────────────────────────────────────
// WORKFLOW BODY — Steps 1-4 (Phase C: Steps 1-2 now inside Workflow)
//
// Skill wrapper passes: diff_text, file_list, base_sha, commit_log,
//   task_context (optional), mode, task_id (optional),
//   gpt56_independent / enable_gpt56_independent / independent_review (optional)
//   gpt55_independent / enable_gpt55_independent (legacy aliases)
// Workflow handles: intent extraction, persona selection, dispatch, merge
// ─────────────────────────────────────────────────────────────────────────────

phase('Resolve')

const {
  diff_text: diffText,
  file_list: fileList,
  base_sha: baseSha,
  commit_log: commitLog,
  task_context: taskContext,
  mode = 'headless',
  task_id: taskId,
} = args ?? {}

if (!diffText || !diffText.trim() || diffText.trim() === 'EMPTY_DIFF') {
  log('Diff is empty — returning clean envelope')
  return {
    residual: [], pre_existing: [], dropped: [], mode,
    status: 'did_not_execute',
    skipped_reason: 'empty diff',
    execution: buildExecution({
      skippedReason: 'empty diff',
      requiredMissing: [...ALWAYS_ON_PERSONAS],
    }),
    stats: { personas_run: 0, dropped_findings: 0 },
  }
}

if (!baseSha) {
  log('ERROR: base_sha required — pass from skill')
  return {
    residual: [], pre_existing: [], dropped: [], mode,
    status: 'did_not_execute',
    error: 'missing base_sha',
    execution: buildExecution({
      skippedReason: 'missing base_sha — cannot resolve a diff to review',
      requiredMissing: [...ALWAYS_ON_PERSONAS],
    }),
    stats: { personas_run: 0, dropped_findings: 0 },
  }
}

const changeLines = diffText.split('\n').filter(l => l.startsWith('+') || l.startsWith('-')).length
const fileCount = fileList ? fileList.split('\n').filter(Boolean).length : 0
log(`Diff: ${changeLines} changed lines, ${fileCount} files`)

// ── COMBINED SETUP AGENT (Steps 1 + 2 in one call) ───────────────────────
// Intent extraction + persona activation. One round-trip instead of 2-3.
// Schema-validated → activation flags are hard booleans, not freeform text.

const intentContext = taskContext
  ? `CAS task context:\n${taskContext}`
  : `Commit messages:\n${commitLog ?? '(no commit log provided)'}`

const setup = await agent(`You are the cas-code-review setup agent. Analyze the diff and decide:
1. A 2-3 line intent summary (Goal + Scope marker + Non-goals if any)
2. Which conditional personas to activate (LLM judgment — read the diff, do not pattern-match paths)
3. Whether the fallow persona should run (JS/TS detection)

## Source of truth for intent
${intentContext}

## File list
${fileList ?? '(not provided)'}

## Diff header (first 1500 chars)
${diffText.slice(0, 1500)}

## Activation rules — LLM-judged, not path pattern matching
Do NOT grep for /auth/ in paths and call it security activation. Read the diff, understand what it does, decide whether the heuristic fires. This is an LLM-judged decision, not path pattern matching.
- activate_security: diff touches auth/session/token boundaries, user input parsing/deserialization, or permission surfaces (MCP tool dispatch, jail/sandbox logic, factory-mode tool restriction)
- activate_adversarial: diff has 50+ non-test changed lines (you have ${changeLines}) AND touches CAS high-stakes modules (close_ops, verify_ops, factory coordination, SQLite stores, hook system, MCP dispatch). Always false if fewer than 20 non-test lines.
- activate_performance: diff touches DB queries, data transforms on large inputs, caching, or async hot paths
- fallow_skip_reason: null if this is a JS/TS repo with JS/TS files in the diff; a short string reason if fallow should skip (e.g. "non-JS/TS repo: no package.json and no JS/TS files in diff")

Return a single JSON object matching this schema exactly. No prose outside the JSON.`,
  {
    label: 'setup',
    phase: 'Resolve',
    schema: SETUP_SCHEMA,
    model: 'haiku',
  }
)

const intentSummary = setup?.intent_summary ?? '(intent extraction failed)'
const isFallowSkipped = !!setup?.fallow_skip_reason
const fallowRuns = !isFallowSkipped
const gpt55Runs = gpt55ShouldRun(args ?? {}, fileCount, changeLines)

// Build the active persona list from setup flags + always-on
const toRun = [...ALWAYS_ON_PERSONAS]
if (setup?.activate_security) toRun.push('security')
if (setup?.activate_performance) toRun.push('performance')
if (setup?.activate_adversarial) toRun.push('adversarial')
if (fallowRuns) toRun.push('fallow')
if (gpt55Runs) toRun.push('gpt-5.6-sol:independent')

const personasToDispatch = toRun.filter(name => name !== 'fallow' && name !== 'gpt-5.6-sol:independent')

log(`Intent: ${intentSummary.split('\n')[0]}`)
log(`Active personas: ${toRun.join(', ')}`)
log(`Conditional: security=${setup?.activate_security}, performance=${setup?.activate_performance}, adversarial=${setup?.activate_adversarial}, fallow=${fallowRuns}, gpt55=${gpt55Runs}`)

const shardPlan = planReviewShards(diffText, fileList ?? '', personasToDispatch, args ?? {})
const shardPlanSummary = summarizeShardPlan(shardPlan)
if (shardPlan.enabled) {
  const { missing_files: missing, duplicate_files: duplicates, extra_files: extra } = shardPlan.coverage
  log(`Large diff mode: estimated ${shardPlan.estimated_tokens} tokens > threshold ${shardPlan.threshold}; ${shardPlan.shards.length} shards`)
  log(`Shard coverage: ${shardPlan.coverage.covered_files.length}/${shardPlan.coverage.changed_files.length} files covered; missing=${missing.length}, duplicate=${duplicates.length}, extra=${extra.length}`)
  if (missing.length || duplicates.length || extra.length) {
    log(`ERROR: shard coverage invalid; missing=${missing.join(',')}; duplicate=${duplicates.join(',')}; extra=${extra.join(',')}`)
    return {
      residual: [],
      pre_existing: [],
      mode,
      error: 'invalid shard coverage',
      status: 'did_not_execute',
      activation: { activated: toRun, sharding: shardPlanSummary },
      execution: buildExecution({
        skippedReason: 'invalid shard coverage — no persona was dispatched',
        requiredMissing: [...ALWAYS_ON_PERSONAS],
      }),
      stats: { personas_run: 0, task_id: taskId ?? null },
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// PHASE 2: PARALLEL PERSONA DISPATCH (Step 3)
// ─────────────────────────────────────────────────────────────────────────────

phase('Review')

let personaResults = []
// cas-acf83: keep the orchestrator's own record of who was dispatched and
// what came back, so mandatory-lane coverage is decided by set difference
// rather than by trusting each result to name itself.
const dispatchRecord = []
if (!shardPlan.enabled) {
  personaResults = await pipelineWithConcurrency(
    personasToDispatch,
    async (name) => {
      const output = await dispatchReviewPersona(
        name,
        buildPersonaPrompt(name, diffText, fileList ?? '', intentSummary, baseSha),
        `review:${name}`
      )
      dispatchRecord.push({ persona: name, output })
      return output
    }
  )
} else {
  const shardJobs = shardPlan.shards.flatMap(shard =>
    shard.personas.map(name => ({ name, shard }))
  )
  log(`Large diff dispatch: ${shardJobs.length} shard/persona runs (${shardPlan.shards.map(s => `${s.id}:${s.personas.join('+')}`).join('; ')})`)
  personaResults = await pipelineWithConcurrency(
    shardJobs,
    async ({ name, shard }) => {
      const shardIntent = `${intentSummary}

Shard: ${shard.id}
Subsystem: ${shard.subsystem}
Files in this shard:
${shard.files.join('\n')}

${shard.kind === 'interface'
  ? 'Interface integrator pass: review only cross-shard contracts, changed function/type signatures, shared traits, exported APIs, and assumptions that could break callers in another shard.'
  : 'Subsystem shard pass: review this coherent subsystem slice; do not assume files outside the listed shard are unchanged, but keep findings grounded in this shard diff.'}`
      const shardDiff = shard.diff_text?.trim()
        ? shard.diff_text
        : `# No signature-like interface diff lines detected for ${shard.id}; review the changed file list and cross-shard contract risk only.`
      const output = await dispatchReviewPersona(
        name,
        buildPersonaPrompt(name, shardDiff, shard.files.join('\n'), shardIntent, baseSha),
        `review:${name}:${shard.id}`
      )
      dispatchRecord.push({ persona: name, output })
      return output
    }
  )
}

let fallowResult = null
if (fallowRuns) {
  fallowResult = await dispatchReviewPersona(
    'fallow',
    buildPersonaPrompt('fallow', diffText, fileList ?? '', intentSummary, baseSha),
    'review:fallow'
  )
}

let gpt55Result = null
if (gpt55Runs) {
  gpt55Result = await dispatchReviewPersona(
    'gpt-5.6-sol:independent',
    buildIndependentPrompt(diffText, fileList ?? '', intentSummary, baseSha),
    'review:gpt-5.6-sol:independent'
  )
}

const allOutputs = [...personaResults, fallowResult, gpt55Result]
  .filter(Boolean)
  .map(stripNullValues)
const skippedPersonas = skippedPersonaResults(allOutputs)
const personasRun = personasRunCount(allOutputs)
const incompletePersonas = incompleteAlwaysOnPersonas(skippedPersonas)
const missingMandatory = missingMandatoryPersonas(dispatchRecord)
// cas-acf83 (GH #108): zero personas is not a clean review, it is the absence
// of one. Distinguish it from both 'complete' and the pre-existing
// 'incomplete' (some always-on persona skipped, but a verdict still exists).
const reviewStatus = personasRun === 0
  ? 'did_not_execute'
  : incompletePersonas.length ? 'incomplete' : 'complete'
const gpt55Skipped = skippedPersonas.some(
  skipped => skipped.reviewer === 'gpt-5.6-sol:independent'
)

for (const skipped of skippedPersonas) {
  log(`Skipped persona ${skipped.reviewer}: ${skipped.reason}`)
}
if (incompletePersonas.length) {
  log(`ERROR: review incomplete; always-on personas skipped: ${incompletePersonas.join(', ')}`)
}
if (missingMandatory.length) {
  log(`ERROR: mandatory reviewers produced no verdict: ${missingMandatory.join(', ')} — \
an empty findings list from this run is silence, not a pass`)
}
if (personasRun === 0) {
  // cas-acf83: loudest possible signal. Everything downstream — the returned
  // envelope, the close gate — now treats this as a failed review, but the
  // operator watching the run should not have to infer that from a count.
  log(`ERROR: REVIEW DID NOT EXECUTE — no persona produced a verdict. \
Failures: ${skippedPersonas.map(s => `${s.reviewer} (${s.reason})`).join('; ') || 'none reported'}`)
}

// ─────────────────────────────────────────────────────────────────────────────
// PHASE 3: DETERMINISTIC MERGE (Step 4 — pure JS, Phase A validated)
// ─────────────────────────────────────────────────────────────────────────────

phase('Merge')

const { residual, pre_existing, dropped } = mergeFindings(allOutputs)

const p0 = residual.filter(f => f.severity === 'P0').length
const p1 = residual.filter(f => f.severity === 'P1').length
const p2 = residual.filter(f => f.severity === 'P2').length
const p3 = residual.filter(f => f.severity === 'P3').length

for (const item of dropped) {
  const title = typeof item.finding?.title === 'string' ? item.finding.title : '<untitled>'
  const detail = item.reason === 'schema_validation_failed'
    ? item.validation_errors.join(', ')
    : `confidence ${item.finding.confidence} below ${item.threshold}`
  log(`Dropped/unmergeable finding from ${item.reviewer}: ${title} (${item.reason}: ${detail})`)
}

log(`Merged: ${residual.length} new (P0:${p0}, P1:${p1}, P2:${p2}, P3:${p3}), ${pre_existing.length} pre-existing, ${dropped.length} dropped/unmergeable`)

return {
  status: reviewStatus,
  residual,
  pre_existing,
  dropped,
  mode,
  intent_summary: intentSummary,
  activation: {
    activated: toRun,
    fallow_skipped: isFallowSkipped,
    fallow_skip_reason: setup?.fallow_skip_reason ?? null,
    gpt55_independent: gpt55Runs,
    gpt55_independent_skipped: gpt55Skipped,
    gpt55_independent_skip_reason: gpt55Result?.skipped_reason ?? null,
    gpt56_independent: gpt55Runs,
    gpt56_independent_skipped: gpt55Skipped,
    gpt56_independent_skip_reason: gpt55Result?.skipped_reason ?? null,
    skipped_personas: skippedPersonas,
    degraded: incompletePersonas.length > 0,
    incomplete_personas: incompletePersonas,
    personas_run: personasRun,
    ...(shardPlan.enabled ? { sharding: shardPlanSummary } : {}),
  },
  execution: buildExecution({
    personasRun,
    skippedPersonas,
    requiredMissing: missingMandatory,
    skippedReason: personasRun === 0
      ? 'no persona produced a verdict — see execution.personas_failed'
      : null,
  }),
  stats: {
    total_findings: residual.length + pre_existing.length,
    p0, p1, p2, p3,
    personas_run: personasRun,
    dropped_findings: dropped.length,
    task_id: taskId ?? null,
  },
}
