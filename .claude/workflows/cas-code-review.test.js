// cas-code-review.test.js — structural validation for the production Workflow script
// Run with: node --test cas-code-review.test.js
//
// Written test-first (cas-0f13 + cas-7c64, test-first posture).
// The production script under test: .claude/workflows/cas-code-review.js
//
// Tests validate:
//   1. The meta block has required fields and correct phase titles
//   2. PERSONA_PROMPTS has all 7 canonical reviewers (verbatim from personas/*.md)
//   3. The REVIEWER_OUTPUT_SCHEMA matches the ReviewerOutput contract
//   4. mergeFindings is imported from merge-findings.js (Phase A module)
//   5. ALWAYS_ON_PERSONAS contains exactly the 4 required personas
//   6. CONDITIONAL_PERSONAS contains exactly the 3 conditional personas
//   7. [Phase C] SETUP_SCHEMA exists and validates the combined setup agent output
//   8. [Phase C] Skill-facing args no longer require intent_summary or activated_personas
//      (the Workflow now handles Steps 1-2 internally)

import { test, describe } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

// Import the production script's exported symbols.
// The script itself is NOT a standard module (it's a Workflow script).
// We test the exported constants via a thin barrel-export pattern:
// the production script exports its testable parts at the bottom.
// Import testable constants from the constants module.
// (The production Workflow script cas-code-review.js cannot be imported as a
// standard ES module because Workflow scripts use top-level `return` — a
// non-standard extension of the Workflow runtime. Constants are in a
// separate importable module.)
import {
  PERSONA_PROMPTS,
  ALWAYS_ON_PERSONAS,
  CONDITIONAL_PERSONAS,
  REVIEWER_OUTPUT_SCHEMA,
  WORKFLOW_META,
  DEFAULT_LARGE_DIFF_TOKEN_THRESHOLD,
  INTERFACE_INTEGRATOR_SHARD,
  estimateDiffTokens,
  normalizeChangedFiles,
  shouldShardReview,
  subsystemForFile,
  shardPersonas,
  planReviewShards,
  summarizeShardPlan,
  gpt55ShouldRun,
  stripNullValues,
  skippedPersonaResults,
  personasRunCount,
  incompleteAlwaysOnPersonas,
} from './cas-code-review-constants.js'

import { mergeFindings, findingValidationErrors } from './merge-findings.js'

const CANONICAL_ALWAYS_ON = ['correctness', 'testing', 'maintainability', 'project-standards']
const CANONICAL_CONDITIONAL = ['security', 'performance', 'adversarial']
const CANONICAL_ALL = [...CANONICAL_ALWAYS_ON, ...CANONICAL_CONDITIONAL, 'fallow']
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor
const WORKFLOW_SOURCE = readFileSync(new URL('./cas-code-review.js', import.meta.url), 'utf8')

function loadEmbeddedMergeApi() {
  const start = WORKFLOW_SOURCE.indexOf('const OWNER_RANK =')
  const end = WORKFLOW_SOURCE.indexOf('// HELPERS', start)
  assert.notEqual(start, -1, 'embedded merge pipeline start marker must exist')
  assert.notEqual(end, -1, 'embedded merge pipeline end marker must exist')
  const source = WORKFLOW_SOURCE.slice(start, end)
  return new Function(`${source}\nreturn { findingValidationErrors, mergeFindings }`)()
}

async function runWorkflowDryRun(args, setupOverride = {}, agentOverrides = {}) {
  const source = WORKFLOW_SOURCE.replace('export const meta =', 'const meta =')
  const labels = []
  const calls = []
  const logs = []
  async function agent(prompt, options = {}) {
    labels.push(options.label)
    calls.push({ prompt, options })
    if (options.label === 'setup') {
      return {
        intent_summary: 'Goal: exercise workflow dry-run.\nScope: synthetic diff.',
        activate_security: false,
        activate_adversarial: false,
        activate_performance: false,
        fallow_skip_reason: 'non-JS/TS repo',
        ...setupOverride,
      }
    }
    if (Object.hasOwn(agentOverrides, options.label)) {
      const override = agentOverrides[options.label]
      return typeof override === 'function' ? override({ prompt, options }) : override
    }
    if (Object.hasOwn(agentOverrides, '*')) {
      const override = agentOverrides['*']
      return typeof override === 'function' ? override({ prompt, options }) : override
    }
    return {
      reviewer: options.label,
      findings: [],
      residual_risks: [],
      testing_gaps: [],
    }
  }
  async function pipeline(items, fn) {
    return Promise.all(items.map(fn))
  }
  function phase(name) {
    logs.push(`phase:${name}`)
  }
  function log(message) {
    logs.push(message)
  }

  const workflow = new AsyncFunction('args', 'agent', 'pipeline', 'phase', 'log', source)
  const result = await workflow(args, agent, pipeline, phase, log)
  return { result, labels, calls, logs }
}

// ─────────────────────────────────────────────────────────────────────────────
// MERGE IMPLEMENTATION PARITY
// ─────────────────────────────────────────────────────────────────────────────

describe('merge implementation parity', () => {
  // cas-0e5b3 (GH #112): this file under .claude/workflows/ is a RENDERED
  // ARTIFACT, not a second source. `sync_workflows` (cas-cli/src/builtins.rs)
  // force-writes it from `BUILTIN_WORKFLOWS` on `cas update --sync`, and the
  // constant's doc calls workflows "pure CAS-managed artifacts [that] should
  // never be hand-edited".
  //
  // The old failure message here — "edit .claude and builtin Workflows
  // together" — invited the wrong repair. Editing the rendered copy is exactly
  // what gets silently reverted by the next sync, so a reader who followed it
  // would reintroduce the drift they were trying to fix. The real repair is
  // one-directional: change the builtin, regenerate the copy.
  test('the rendered .claude copy is in sync with the builtin source', () => {
    const builtinPath = '../../cas-cli/src/builtins/workflows/cas-code-review.js'
    const builtin = readFileSync(new URL(builtinPath, import.meta.url), 'utf8')

    if (WORKFLOW_SOURCE !== builtin) {
      // Name the first divergent line: a whole-file diff of a 40KB script is
      // unreadable, and the last drift (CODEX_PERSONA_EFFORT, 1 line of 1100)
      // sat unnoticed for a week behind exactly that wall of output.
      const mine = WORKFLOW_SOURCE.split('\n')
      const theirs = builtin.split('\n')
      let firstDiff = -1
      for (let i = 0; i < Math.max(mine.length, theirs.length); i += 1) {
        if (mine[i] !== theirs[i]) { firstDiff = i; break }
      }
      assert.fail(
        `.claude/workflows/cas-code-review.js is STALE relative to its builtin source.\n` +
        `This file is generated — do NOT edit it to fix this.\n` +
        `  builtin (source of truth): ${firstDiff + 1}: ${theirs[firstDiff] ?? '<absent>'}\n` +
        `  rendered copy (stale):     ${firstDiff + 1}: ${mine[firstDiff] ?? '<absent>'}\n` +
        `Repair: edit cas-cli/src/builtins/workflows/cas-code-review.js, then\n` +
        `regenerate with \`cas update --sync\` (or copy builtin → .claude/workflows/).`,
      )
    }
  })

  test('standalone exported validator matches the embedded Workflow validator', () => {
    const embedded = loadEmbeddedMergeApi()
    const candidates = [
      null,
      {},
      {
        title: 'Valid finding',
        severity: 'P2',
        file: 'src/lib.rs',
        line: 12,
        why_it_matters: 'It matters.',
        autofix_class: 'manual',
        owner: 'downstream-resolver',
        confidence: 0.8,
        evidence: ['src/lib.rs:12'],
        pre_existing: false,
      },
      {
        title: 'x'.repeat(101),
        severity: 'critical',
        file: 42,
        line: 0,
        why_it_matters: false,
        autofix_class: 'automatic',
        owner: 'worker',
        confidence: 2,
        evidence: [],
        pre_existing: 'no',
        unexpected: true,
      },
    ]

    for (const candidate of candidates) {
      assert.deepEqual(
        findingValidationErrors(candidate),
        embedded.findingValidationErrors(candidate),
      )
    }
  })

  test('standalone merge output matches the embedded Workflow merge', () => {
    const embedded = loadEmbeddedMergeApi()
    const validFinding = {
      title: 'Shared issue',
      severity: 'P1',
      file: 'src/lib.rs',
      line: 12,
      why_it_matters: 'It matters.',
      autofix_class: 'manual',
      owner: 'downstream-resolver',
      confidence: 0.8,
      evidence: ['src/lib.rs:12'],
      pre_existing: false,
    }
    const reviewerOutputs = [
      { reviewer: 'correctness', findings: [validFinding] },
      {
        reviewer: 'testing',
        findings: [{ ...validFinding, line: 13, owner: 'human', confidence: 0.7 }],
      },
      {
        reviewer: 'gpt-5.6-sol:independent',
        findings: [{ ...validFinding, title: 'Low confidence', confidence: 0.55 }],
      },
      { reviewer: 'maintainability', findings: [{ title: 'Under-filled' }] },
    ]

    assert.deepEqual(
      mergeFindings(reviewerOutputs),
      embedded.mergeFindings(reviewerOutputs),
    )
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// META BLOCK
// ─────────────────────────────────────────────────────────────────────────────

describe('cas-acf83 (GH #108): execution provenance', () => {
  // Dispatch labels are `review:<persona>` (and `review:<persona>:<shard>`);
  // a real persona names itself with the bare persona name in its result.
  const personaOf = label => String(label).replace(/^review:/, '').split(':')[0]

  const BASE_ARGS = {
    diff_text: 'diff --git a/lib.rs b/lib.rs\n-old\n+new',
    file_list: 'lib.rs',
    base_sha: 'abc123',
    commit_log: 'synthetic',
  }

  // A review that did not run returns `residual: []` — structurally identical
  // to a clean review. The close gate only looks for P0s, so it accepted the
  // first as if it were the second. Every return path must now say which one
  // happened.

  test('every early return reports execution, not just findings', () => {
    // The bail-outs that return an empty envelope without dispatching anyone.
    // Each is the exact shape that used to reach the close gate looking clean.
    const bailouts = ['empty diff', 'missing base_sha', 'invalid shard coverage']
    for (const marker of bailouts) {
      const at = WORKFLOW_SOURCE.indexOf(`'${marker}'`)
      assert.notEqual(at, -1, `bail-out for ${marker} must still exist`)
      const block = WORKFLOW_SOURCE.slice(Math.max(0, at - 400), at + 400)
      assert.match(
        block,
        /execution: buildExecution\(/,
        `the ${marker} bail-out must state that it did not execute`,
      )
      assert.match(
        block,
        /status: 'did_not_execute'/,
        `the ${marker} bail-out must carry the explicit failure status`,
      )
    }
  })

  test('buildExecution names failed personas so a launch failure is diagnosable', () => {
    const at = WORKFLOW_SOURCE.indexOf('function buildExecution(')
    assert.notEqual(at, -1, 'buildExecution must exist')
    const src = WORKFLOW_SOURCE.slice(at, WORKFLOW_SOURCE.indexOf('\n}', WORKFLOW_SOURCE.indexOf('return {', at)))
    assert.match(src, /personas_run/, 'must report how many personas produced a verdict')
    assert.match(
      src,
      /personas_failed[\s\S]*reviewer[\s\S]*reason/,
      'must name each failed persona AND why',
    )
    assert.match(src, /skipped_reason/, 'must carry the producer-level skip reason')
    assert.match(
      src,
      /required_personas_missing/,
      'must report mandatory lanes with no verdict — personas_run > 0 alone is too weak',
    )
  })

  test('zero personas is its own status, distinct from complete and incomplete', () => {
    assert.match(
      WORKFLOW_SOURCE,
      /personasRun === 0\s*\n?\s*\?\s*'did_not_execute'/,
      "personas_run === 0 must map to 'did_not_execute', not 'complete'",
    )
    assert.match(
      WORKFLOW_SOURCE,
      /REVIEW DID NOT EXECUTE/,
      'the run log must say so loudly, not leave it to be inferred from a count',
    )
  })

  // --- behavioural: run the real workflow through the dry-run harness -------

  test('a transport outage that skips every persona reports personas_run 0', async () => {
    // The filed incident: every persona returns a skipped_reason instead of a
    // verdict. The merged findings are empty either way — only `execution`
    // distinguishes this from a clean review.
    const { result } = await runWorkflowDryRun(BASE_ARGS, {}, {
      '*': ({ options }) => ({
        reviewer: personaOf(options.label),
        findings: [],
        residual_risks: [],
        testing_gaps: [],
        skipped_reason: '402 insufficient credits',
      }),
    })

    assert.equal(result.residual.length, 0, 'precondition: findings look clean')
    assert.equal(result.execution.personas_run, 0)
    assert.equal(result.status, 'did_not_execute')
    assert.ok(
      result.execution.personas_failed.length > 0,
      'each failed persona must be named with its reason',
    )
    assert.ok(
      result.execution.personas_failed.every(f => f.includes('402 insufficient credits')),
      `must carry the transport reason: ${JSON.stringify(result.execution.personas_failed)}`,
    )
  })

  test('a partial outage names the mandatory lanes that produced no verdict', async () => {
    // Only the always-on personas fail. `personas_run > 0` alone would call
    // this a successful review — required_personas_missing is what stops it.
    const alwaysOn = ['correctness', 'testing', 'maintainability', 'project-standards']
    const { result } = await runWorkflowDryRun(BASE_ARGS, { activate_security: true }, {
      '*': ({ options }) => alwaysOn.includes(personaOf(options.label))
        ? {
            reviewer: personaOf(options.label),
            findings: [],
            residual_risks: [],
            testing_gaps: [],
            skipped_reason: 'transport unavailable',
          }
        : {
            reviewer: personaOf(options.label),
            findings: [],
            residual_risks: [],
            testing_gaps: [],
          },
    })

    assert.ok(result.execution.personas_run > 0, 'a persona did run')
    assert.deepEqual(
      [...result.execution.required_personas_missing].sort(),
      [...alwaysOn].sort(),
      'every missing mandatory lane must be named',
    )
  })

  test('a clean full run reports execution with no missing lanes', async () => {
    const { result } = await runWorkflowDryRun(BASE_ARGS, {}, {
      '*': ({ options }) => ({
        reviewer: personaOf(options.label),
        findings: [],
        residual_risks: [],
        testing_gaps: [],
      }),
    })
    assert.ok(result.execution.personas_run > 0)
    assert.deepEqual(result.execution.required_personas_missing, [])
    assert.deepEqual(result.execution.personas_failed, [])
    assert.equal(result.status, 'complete')
  })
})

describe('WORKFLOW_META', () => {
  test('has required name field', () => {
    assert.equal(typeof WORKFLOW_META.name, 'string')
    assert.ok(WORKFLOW_META.name.length > 0)
    assert.equal(WORKFLOW_META.name, 'cas-code-review')
  })

  test('has required description field', () => {
    assert.equal(typeof WORKFLOW_META.description, 'string')
    assert.ok(WORKFLOW_META.description.length > 0)
  })

  test('phases array covers Resolve, Review, Merge', () => {
    assert.ok(Array.isArray(WORKFLOW_META.phases))
    const titles = WORKFLOW_META.phases.map(p => p.title)
    assert.ok(titles.some(t => t.includes('Resolve') || t.includes('resolve')),
      `phases must include Resolve: ${titles}`)
    assert.ok(titles.some(t => t.includes('Review') || t.includes('review')),
      `phases must include Review: ${titles}`)
    assert.ok(titles.some(t => t.includes('Merge') || t.includes('merge')),
      `phases must include Merge: ${titles}`)
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// PERSONA PROMPTS
// ─────────────────────────────────────────────────────────────────────────────

describe('PERSONA_PROMPTS', () => {
  test('has all 8 canonical personas (7 + fallow)', () => {
    for (const name of CANONICAL_ALL) {
      assert.ok(name in PERSONA_PROMPTS, `Missing persona: ${name}`)
    }
  })

  test('each persona prompt is a non-empty string', () => {
    for (const [name, prompt] of Object.entries(PERSONA_PROMPTS)) {
      assert.equal(typeof prompt, 'string',
        `${name}: prompt must be a string`)
      assert.ok(prompt.length > 100,
        `${name}: prompt is suspiciously short (${prompt.length} chars)`)
    }
  })

  test('correctness prompt references its mandate and output contract', () => {
    const p = PERSONA_PROMPTS.correctness
    assert.ok(p.includes('ReviewerOutput') || p.includes('reviewer'),
      'correctness prompt must reference ReviewerOutput or reviewer field')
    assert.ok(p.includes('correctness'),
      'correctness prompt must self-identify as correctness reviewer')
  })

  test('fallow prompt references fallow audit CLI', () => {
    const p = PERSONA_PROMPTS.fallow
    assert.ok(p.includes('fallow audit'),
      'fallow prompt must reference fallow audit command')
    assert.ok(p.includes('JS/TS') || p.includes('TypeScript') || p.includes('JavaScript'),
      'fallow prompt must reference JS/TS scope')
  })

  test('security persona prompt references auth/input surfaces', () => {
    const p = PERSONA_PROMPTS.security
    assert.ok(p.includes('auth') || p.includes('session') || p.includes('input'),
      'security prompt must reference auth/session/input surfaces')
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// ALWAYS_ON_PERSONAS / CONDITIONAL_PERSONAS
// ─────────────────────────────────────────────────────────────────────────────

describe('ALWAYS_ON_PERSONAS', () => {
  test('contains exactly the 4 required always-on personas', () => {
    assert.deepEqual([...ALWAYS_ON_PERSONAS].sort(), [...CANONICAL_ALWAYS_ON].sort())
  })
})

describe('CONDITIONAL_PERSONAS', () => {
  test('contains exactly the 3 conditional personas', () => {
    assert.deepEqual([...CONDITIONAL_PERSONAS].sort(), [...CANONICAL_CONDITIONAL].sort())
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// REVIEWER_OUTPUT_SCHEMA
// ─────────────────────────────────────────────────────────────────────────────

describe('REVIEWER_OUTPUT_SCHEMA', () => {
  function assertStrictObjectSchema(schema, path = '$') {
    if (Array.isArray(schema.type) && schema.type.includes('object')) {
      assert.fail(`${path}: nullable object schemas are not expected here`)
    }
    if (schema.type === 'object') {
      assert.equal(schema.additionalProperties, false, `${path}: object must reject extra keys`)
      assert.deepEqual(
        [...schema.required].sort(),
        Object.keys(schema.properties).sort(),
        `${path}: strict structured output requires every property key`,
      )
      for (const [key, propertySchema] of Object.entries(schema.properties)) {
        assertStrictObjectSchema(propertySchema, `${path}.${key}`)
      }
    }
    if (
      schema.type === 'array'
      || (Array.isArray(schema.type) && schema.type.includes('array'))
    ) {
      assertStrictObjectSchema(schema.items, `${path}[]`)
    }
  }

  test('is a JSON Schema object', () => {
    assert.equal(REVIEWER_OUTPUT_SCHEMA.type, 'object')
  })

  test('requires reviewer and findings fields', () => {
    assert.ok(REVIEWER_OUTPUT_SCHEMA.required.includes('reviewer'))
    assert.ok(REVIEWER_OUTPUT_SCHEMA.required.includes('findings'))
  })

  test('has additionalProperties: false (strict schema)', () => {
    assert.equal(REVIEWER_OUTPUT_SCHEMA.additionalProperties, false)
  })

  test('findings items have correct severity enum', () => {
    const findingSchema = REVIEWER_OUTPUT_SCHEMA.properties.findings.items
    const severityEnum = [...findingSchema.properties.severity.enum]
    assert.deepEqual(severityEnum.sort(), ['P0', 'P1', 'P2', 'P3'])
  })

  test('findings items have correct owner enum', () => {
    const findingSchema = REVIEWER_OUTPUT_SCHEMA.properties.findings.items
    const ownerEnum = [...findingSchema.properties.owner.enum]
    assert.deepEqual(ownerEnum.sort(), ['downstream-resolver', 'human', 'review-fixer'])
  })

  test('findings items have correct autofix_class enum', () => {
    const findingSchema = REVIEWER_OUTPUT_SCHEMA.properties.findings.items
    const autofixEnum = [...findingSchema.properties.autofix_class.enum]
    assert.deepEqual(autofixEnum.sort(), ['advisory', 'gated_auto', 'manual', 'safe_auto'])
  })

  test('confidence is a number bounded 0.0..1.0', () => {
    const findingSchema = REVIEWER_OUTPUT_SCHEMA.properties.findings.items
    const conf = findingSchema.properties.confidence
    assert.equal(conf.type, 'number')
    assert.equal(conf.minimum, 0.0)
    assert.equal(conf.maximum, 1.0)
  })

  test('evidence requires at least one item', () => {
    const findingSchema = REVIEWER_OUTPUT_SCHEMA.properties.findings.items
    const evid = findingSchema.properties.evidence
    assert.equal(evid.minItems, 1)
  })

  test('allows skipped_reason for skipped reviewer envelopes', () => {
    assert.deepEqual(REVIEWER_OUTPUT_SCHEMA.properties.skipped_reason.type, ['string', 'null'])
  })

  test('is accepted by strict structured-output required-property rules', () => {
    assertStrictObjectSchema(REVIEWER_OUTPUT_SCHEMA)
  })

  test('represents domain-optional fields as required nullable properties', () => {
    const findingSchema = REVIEWER_OUTPUT_SCHEMA.properties.findings.items
    assert.deepEqual(findingSchema.properties.suggested_fix.type, ['string', 'null'])
    assert.deepEqual(findingSchema.properties.requires_verification.type, ['boolean', 'null'])
    assert.deepEqual(REVIEWER_OUTPUT_SCHEMA.properties.residual_risks.type, ['array', 'null'])
    assert.deepEqual(REVIEWER_OUTPUT_SCHEMA.properties.testing_gaps.type, ['array', 'null'])
    assert.deepEqual(REVIEWER_OUTPUT_SCHEMA.properties.skipped_reason.type, ['string', 'null'])
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// CODEX PERSONA TRANSPORT + INDEPENDENT PERSONA HELPERS
// ─────────────────────────────────────────────────────────────────────────────

describe('Codex reviewer transport', () => {
  test('shared shim pins model, effort, schema, timeout, and distinct retry budgets', () => {
    const source = readFileSync(new URL('./cas-code-review.js', import.meta.url), 'utf8')
    const shimStart = source.indexOf('function buildCodexReviewerShimPrompt')
    const shimEnd = source.indexOf('\nasync function dispatchReviewPersona', shimStart)
    const shim = source.slice(shimStart, shimEnd)

    assert.notEqual(shimStart, -1, 'shared Codex reviewer shim must exist')
    assert.match(shim, /-m \$\{CODEX_PERSONA_MODEL\}/)
    assert.match(shim, /model_reasoning_effort=\$\{CODEX_PERSONA_EFFORT\}/)
    assert.match(shim, /--output-schema/)
    assert.match(shim, /CODEX_PERSONA_TIMEOUT_SECONDS/)
    assert.match(shim, /CODEX_SCHEMA_RETRIES/)
    assert.match(shim, /CODEX_TIMEOUT_RETRIES/)
    assert.match(shim, /schema mismatch retries do not consume the timeout retry budget/i)
  })

  test('uses a portable shell watchdog instead of a platform-specific timeout binary', async () => {
    const { calls } = await runWorkflowDryRun({
      diff_text: 'diff --git a/lib.rs b/lib.rs\n-old\n+new',
      file_list: 'lib.rs',
      base_sha: 'abc123',
      commit_log: 'synthetic',
    })
    const correctness = calls.find(call => call.options.label === 'review:correctness')

    assert.doesNotMatch(correctness.prompt, /\/usr\/bin\/timeout/)
    assert.match(correctness.prompt, /command -v sleep/)
    assert.match(correctness.prompt, /sleep 600/)
    assert.match(correctness.prompt, /TIMEOUT_MARKER/)
  })

  test('normal reviewers use Codex while security stays on Claude Opus', async () => {
    const { calls } = await runWorkflowDryRun({
      diff_text: 'diff --git a/auth.rs b/auth.rs\n-old\n+new',
      file_list: 'auth.rs',
      base_sha: 'abc123',
      commit_log: 'synthetic',
    }, {
      activate_security: true,
    })

    const correctness = calls.find(call => call.options.label === 'review:correctness')
    assert.match(correctness.prompt, /codex exec -s read-only -m gpt-5\.6-sol/)
    assert.equal(correctness.options.model, 'haiku')
    for (const field of REVIEWER_OUTPUT_SCHEMA.properties.findings.items.required) {
      assert.ok(correctness.prompt.includes(field), `Codex shim must embed schema field: ${field}`)
    }

    const security = calls.find(call => call.options.label === 'review:security')
    assert.doesNotMatch(security.prompt, /codex exec/)
    assert.equal(security.options.model, 'opus')
    assert.deepEqual(security.options.schema, REVIEWER_OUTPUT_SCHEMA)
  })

  test('fallow and independent reviewer share the Codex shim', async () => {
    const { calls } = await runWorkflowDryRun({
      diff_text: 'diff --git a/app.js b/app.js\n-old\n+new',
      file_list: 'app.js',
      base_sha: 'abc123',
      commit_log: 'synthetic',
      gpt55_independent: true,
    }, {
      fallow_skip_reason: null,
    })

    for (const label of ['review:fallow', 'review:gpt-5.6-sol:independent']) {
      const call = calls.find(candidate => candidate.options.label === label)
      assert.ok(call, `${label} must dispatch`)
      assert.match(call.prompt, /codex exec -s read-only -m gpt-5\.6-sol/)
    }
  })
})

describe('gpt-5.6-sol independent activation helpers', () => {
  test('independent prompt is a direct reviewer prompt, not a nested transport adapter', () => {
    const source = readFileSync(new URL('./cas-code-review.js', import.meta.url), 'utf8')
    const promptStart = source.indexOf('function buildIndependentPrompt')
    const promptEnd = source.indexOf('\nfunction gpt55ShouldRun', promptStart)
    const prompt = source.slice(promptStart, promptEnd)

    assert.notEqual(promptStart, -1)
    assert.match(prompt, /# Persona: gpt-5\.6-sol:independent/)
    assert.doesNotMatch(prompt, /thin Sonnet-low wrapper/)
  })

  test('activates at broad file-count boundary', () => {
    assert.equal(gpt55ShouldRun({}, 4, 299), false)
    assert.equal(gpt55ShouldRun({}, 5, 299), true)
  })

  test('activates at broad changed-line boundary', () => {
    assert.equal(gpt55ShouldRun({}, 4, 299), false)
    assert.equal(gpt55ShouldRun({}, 4, 300), true)
  })

  test('activates for every explicit arg variant', () => {
    for (const args of [
      { gpt56_independent: true },
      { gpt56_independent: 'true' },
      { enable_gpt56_independent: true },
      { enable_gpt56_independent: 'true' },
      { gpt55_independent: true },
      { gpt55_independent: 'true' },
      { enable_gpt55_independent: true },
      { enable_gpt55_independent: 'true' },
      { independent_review: 'gpt-5.5' },
      { independent_review: 'gpt55' },
      { independent_review: 'gpt-5.5:independent' },
      { independent_review: 'gpt-5.6-sol' },
      { independent_review: 'gpt-5.6-sol:independent' },
    ]) {
      assert.equal(gpt55ShouldRun(args, 0, 0), true, JSON.stringify(args))
    }
  })

  test('shared skipped persona accounting preserves reason and excludes skipped runs', () => {
    const outputs = [
      { reviewer: 'correctness', findings: [] },
      {
        reviewer: 'gpt-5.6-sol:independent',
        findings: [],
        skipped_reason: 'codex CLI not installed',
      },
    ]
    const skipped = skippedPersonaResults(outputs)
    assert.deepEqual(skipped, [{
      reviewer: 'gpt-5.6-sol:independent',
      reason: 'codex CLI not installed',
    }])
    assert.equal(personasRunCount(outputs), 1)
    assert.deepEqual(incompleteAlwaysOnPersonas(skipped), [])
  })

  test('null stripping removes strict-schema sentinels recursively', () => {
    assert.deepEqual(stripNullValues({
      reviewer: 'correctness',
      skipped_reason: null,
      findings: [{ suggested_fix: null, requires_verification: true }],
    }), {
      reviewer: 'correctness',
      findings: [{ requires_verification: true }],
    })
    assert.deepEqual(skippedPersonaResults([]), [])
  })

  test('workflow surfaces and logs confidence-gated independent findings', async () => {
    const independentFinding = {
      title: 'Factory hook matcher omits AskUserQuestion',
      severity: 'P1',
      file: 'cas-cli/src/ui/factory/daemon/runtime/teams.rs',
      line: 361,
      why_it_matters: 'The deny branch would be unreachable in real sessions.',
      autofix_class: 'manual',
      owner: 'downstream-resolver',
      confidence: 0.55,
      evidence: ['teams.rs matcher excludes AskUserQuestion'],
      pre_existing: false,
      requires_verification: true,
    }
    const { result, logs } = await runWorkflowDryRun({
      diff_text: 'diff --git a/teams.rs b/teams.rs\n-old\n+new',
      file_list: 'teams.rs',
      base_sha: 'abc123',
      gpt55_independent: true,
    }, {}, {
      'review:gpt-5.6-sol:independent': {
        reviewer: 'gpt-5.6-sol:independent',
        findings: [independentFinding],
        residual_risks: [],
        testing_gaps: [],
      },
    })

    assert.deepEqual(result.residual, [])
    assert.equal(result.dropped.length, 1)
    assert.equal(result.dropped[0].reason, 'confidence_below_threshold')
    assert.equal(result.dropped[0].reviewer, 'gpt-5.6-sol:independent')
    assert.equal(result.stats.dropped_findings, 1)
    assert.equal(result.activation.gpt56_independent, true)
    assert.equal(result.activation.gpt56_independent_skipped, false)
    assert.ok(logs.some(line => line.includes(
      'Dropped/unmergeable finding from gpt-5.6-sol:independent'
    )))
  })
})

describe('skipped-lane accounting', () => {
  const baseArgs = {
    diff_text: 'diff --git a/lib.rs b/lib.rs\n-old\n+new',
    file_list: 'lib.rs',
    base_sha: 'abc123',
    commit_log: 'synthetic',
  }

  test('always-on transport failures are surfaced, excluded, and mark the review incomplete', async () => {
    const skipped = reviewer => ({
      reviewer,
      findings: [],
      residual_risks: [],
      testing_gaps: [],
      skipped_reason: `${reviewer} transport failed`,
    })
    const { result } = await runWorkflowDryRun(baseArgs, {}, {
      'review:correctness': skipped('correctness'),
      'review:testing': skipped('testing'),
    })

    assert.equal(result.status, 'incomplete')
    assert.equal(result.activation.degraded, true)
    assert.deepEqual(result.activation.incomplete_personas, ['correctness', 'testing'])
    assert.deepEqual(result.activation.skipped_personas, [
      { reviewer: 'correctness', reason: 'correctness transport failed' },
      { reviewer: 'testing', reason: 'testing transport failed' },
    ])
    assert.equal(result.activation.personas_run, 2)
    assert.equal(result.stats.personas_run, 2)
  })

  test('conditional skipped lanes are surfaced and excluded without making always-on coverage incomplete', async () => {
    const { result } = await runWorkflowDryRun(baseArgs, {
      activate_performance: true,
    }, {
      'review:performance': {
        reviewer: 'performance',
        findings: [],
        residual_risks: [],
        testing_gaps: [],
        skipped_reason: 'codex auth unavailable',
      },
    })

    assert.equal(result.status, 'complete')
    assert.equal(result.activation.degraded, false)
    assert.deepEqual(result.activation.incomplete_personas, [])
    assert.deepEqual(result.activation.skipped_personas, [
      { reviewer: 'performance', reason: 'codex auth unavailable' },
    ])
    assert.equal(result.stats.personas_run, 4)
  })

  test('strict-schema null sentinels are stripped before deterministic merge', async () => {
    const finding = {
      title: 'Null stripping reaches merge',
      severity: 'P2',
      file: 'lib.rs',
      line: 1,
      why_it_matters: 'Domain consumers expect absent optional values, not null sentinels.',
      autofix_class: 'manual',
      owner: 'human',
      confidence: 0.9,
      evidence: ['synthetic strict-schema envelope'],
      pre_existing: false,
      suggested_fix: null,
      requires_verification: null,
    }
    const { result } = await runWorkflowDryRun(baseArgs, {}, {
      'review:correctness': {
        reviewer: 'correctness',
        findings: [finding],
        residual_risks: null,
        testing_gaps: null,
        skipped_reason: null,
      },
    })

    assert.equal(result.residual.length, 1)
    assert.equal(Object.hasOwn(result.residual[0], 'suggested_fix'), false)
    assert.equal(Object.hasOwn(result.residual[0], 'requires_verification'), false)
    assert.equal(result.activation.skipped_personas.length, 0)
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// LARGE-DIFF SHARDING HELPERS (cas-33f1)
// ─────────────────────────────────────────────────────────────────────────────

const LARGE_DIFF = [
  'diff --git a/cas-cli/src/ui/factory/director/prompts.rs b/cas-cli/src/ui/factory/director/prompts.rs',
  'index 111..222 100644',
  '--- a/cas-cli/src/ui/factory/director/prompts.rs',
  '+++ b/cas-cli/src/ui/factory/director/prompts.rs',
  '@@ -1,2 +1,4 @@',
  '-pub fn old_prompt() {}',
  '+pub fn generate_prompt() {}',
  '+const DELIVERY_GATE: bool = true;',
  'diff --git a/crates/cas-store/src/code_review/merge.rs b/crates/cas-store/src/code_review/merge.rs',
  'index 111..222 100644',
  '--- a/crates/cas-store/src/code_review/merge.rs',
  '+++ b/crates/cas-store/src/code_review/merge.rs',
  '@@ -10,2 +10,4 @@',
  '-pub struct OldMerge;',
  '+pub struct MergedFindings;',
  '+impl MergedFindings { pub fn len(&self) -> usize { 0 } }',
  'diff --git a/docs/reviews/example.md b/docs/reviews/example.md',
  'index 111..222 100644',
  '--- a/docs/reviews/example.md',
  '+++ b/docs/reviews/example.md',
  '@@ -1 +1 @@',
  '-old',
  '+new',
].join('\n')

describe('large-diff sharding helpers', () => {
  test('default threshold is a positive token budget', () => {
    assert.ok(DEFAULT_LARGE_DIFF_TOKEN_THRESHOLD > 1000)
    assert.equal(estimateDiffTokens('12345678'), 2)
  })

  test('normalizes newline-separated changed files', () => {
    assert.deepEqual(
      normalizeChangedFiles(' a.rs\n\n docs/readme.md \n'),
      ['a.rs', 'docs/readme.md']
    )
  })

  test('below threshold disables sharding and preserves full coverage', () => {
    const fileList = 'cas-cli/src/ui/factory/director/prompts.rs\n'
    const plan = planReviewShards('tiny diff', fileList, ['correctness'], {
      large_diff_token_threshold: 9999,
    })
    assert.equal(shouldShardReview('tiny diff', { large_diff_token_threshold: 9999 }), false)
    assert.equal(plan.enabled, false)
    assert.deepEqual(plan.shards, [])
    assert.deepEqual(plan.coverage.missing_files, [])
    assert.deepEqual(plan.coverage.covered_files, ['cas-cli/src/ui/factory/director/prompts.rs'])
  })

  test('over threshold creates subsystem shards plus one interface integrator shard', () => {
    const fileList = [
      'cas-cli/src/ui/factory/director/prompts.rs',
      'crates/cas-store/src/code_review/merge.rs',
      'docs/reviews/example.md',
    ].join('\n')
    const plan = planReviewShards(
      LARGE_DIFF,
      fileList,
      ['correctness', 'testing', 'maintainability', 'project-standards', 'adversarial'],
      { large_diff_token_threshold: 1 }
    )

    assert.equal(plan.enabled, true)
    assert.ok(plan.shards.some(shard => shard.id === 'subsystem:factory-ui'))
    assert.ok(plan.shards.some(shard => shard.id === 'subsystem:store-types'))
    assert.ok(plan.shards.some(shard => shard.id === 'subsystem:docs-skills'))
    assert.equal(plan.shards.filter(shard => shard.id === INTERFACE_INTEGRATOR_SHARD).length, 1)
    assert.deepEqual(plan.coverage.missing_files, [])
    assert.deepEqual(plan.coverage.duplicate_files, [])
    assert.deepEqual([...plan.coverage.covered_files].sort(), fileList.split('\n').sort())
  })

  test('activation summaries omit full shard diff bodies', () => {
    const plan = planReviewShards(
      LARGE_DIFF,
      'cas-cli/src/ui/factory/director/prompts.rs\ndocs/reviews/example.md',
      ['correctness', 'project-standards'],
      { large_diff_token_threshold: 1 }
    )
    const summary = summarizeShardPlan(plan)
    assert.equal(summary.enabled, true)
    assert.ok(summary.shards.every(shard => !('diff_text' in shard)))
    assert.ok(summary.shards.every(shard => Number.isInteger(shard.diff_tokens)))
  })

  test('docs-only shards route fewer personas than code shards', () => {
    const personas = ['correctness', 'testing', 'maintainability', 'project-standards', 'adversarial']
    const docs = { kind: 'subsystem', subsystem: 'docs-skills' }
    const code = { kind: 'subsystem', subsystem: 'factory-ui' }
    const iface = { kind: 'interface', subsystem: 'cross-shard-interfaces' }

    assert.deepEqual(shardPersonas(docs, personas), ['project-standards'])
    assert.deepEqual(shardPersonas(code, personas), personas)
    assert.deepEqual(shardPersonas(iface, personas), ['correctness', 'maintainability', 'adversarial'])
  })

  test('subsystem classifier groups by concern, not by file count chunks', () => {
    assert.equal(subsystemForFile('cas-cli/src/ui/factory/director/prompts.rs'), 'factory-ui')
    assert.equal(subsystemForFile('cas-cli/src/mcp/tools/core/task/lifecycle.rs'), 'mcp-task-lifecycle')
    assert.equal(subsystemForFile('crates/cas-types/src/code_review.rs'), 'store-types')
    assert.equal(subsystemForFile('docs/reviews/example.md'), 'docs-skills')
  })
})

describe('large-diff Workflow dry-run dispatch', () => {
  const fileList = [
    'cas-cli/src/ui/factory/director/prompts.rs',
    'docs/reviews/example.md',
  ].join('\n')

  test('below threshold preserves single full-diff persona dispatch shape', async () => {
    const { result, labels } = await runWorkflowDryRun({
      diff_text: LARGE_DIFF,
      file_list: fileList,
      base_sha: 'abc123',
      commit_log: 'synthetic',
      large_diff_token_threshold: 99999,
    })

    assert.deepEqual(labels, [
      'setup',
      'review:correctness',
      'review:testing',
      'review:maintainability',
      'review:project-standards',
    ])
    assert.equal(result.activation.sharding, undefined)
    assert.equal(result.stats.personas_run, 4)
  })

  test('over threshold dispatches subsystem shards and interface integrator', async () => {
    const { result, labels } = await runWorkflowDryRun({
      diff_text: LARGE_DIFF,
      file_list: fileList,
      base_sha: 'abc123',
      commit_log: 'synthetic',
      large_diff_token_threshold: 1,
    }, {
      activate_adversarial: true,
    })

    assert.equal(result.activation.sharding.enabled, true)
    assert.deepEqual(result.activation.sharding.coverage.missing_files, [])
    assert.ok(labels.includes('review:correctness:subsystem:factory-ui'))
    assert.ok(labels.includes('review:project-standards:subsystem:docs-skills'))
    assert.ok(labels.includes('review:correctness:interface-integrator'))
    assert.ok(labels.includes('review:maintainability:interface-integrator'))
    assert.ok(labels.includes('review:adversarial:interface-integrator'))
  })

  test('bounds concurrent persona transports during Cartesian shard fan-out', async () => {
    let active = 0
    let maxActive = 0
    const { labels } = await runWorkflowDryRun({
      diff_text: LARGE_DIFF,
      file_list: fileList,
      base_sha: 'abc123',
      commit_log: 'synthetic',
      large_diff_token_threshold: 1,
    }, {
      activate_adversarial: true,
      activate_performance: true,
    }, {
      '*': async ({ options }) => {
        active += 1
        maxActive = Math.max(maxActive, active)
        await new Promise(resolve => setTimeout(resolve, 5))
        active -= 1
        return {
          reviewer: options.label.replace(/^review:/, '').split(':')[0],
          findings: [],
          residual_risks: [],
          testing_gaps: [],
        }
      },
    })

    assert.ok(labels.filter(label => label.startsWith('review:')).length > 4)
    assert.ok(maxActive <= 4, `expected at most 4 concurrent transports, saw ${maxActive}`)
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// INTEGRATION: mergeFindings import still works through Phase A module
// ─────────────────────────────────────────────────────────────────────────────

describe('mergeFindings integration (Phase A)', () => {
  test('mergeFindings is a function', () => {
    assert.equal(typeof mergeFindings, 'function')
  })

  test('mergeFindings on empty input returns empty residual', () => {
    const { residual, pre_existing } = mergeFindings([])
    assert.deepEqual(residual, [])
    assert.deepEqual(pre_existing, [])
  })

  test('mergeFindings deduplicates duplicate findings from shard persona runs', () => {
    const finding = {
      title: 'Shared contract may break callers',
      severity: 'P1',
      file: 'crates/cas-types/src/code_review.rs',
      line: 42,
      why_it_matters: 'Two shard personas reported the same interface risk.',
      autofix_class: 'manual',
      owner: 'human',
      confidence: 0.75,
      evidence: ['same file and line bucket'],
      pre_existing: false,
    }
    const { residual } = mergeFindings([
      { reviewer: 'review:correctness:interface-integrator', findings: [finding] },
      { reviewer: 'review:maintainability:subsystem:store-types', findings: [{ ...finding, confidence: 0.70 }] },
    ])
    assert.equal(residual.length, 1)
    assert.equal(residual[0].confidence, 0.85)
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// PHASE C: SETUP_SCHEMA (combined Steps 1-2 agent output)
// The single setup agent returns intent + activation decisions in one call,
// halving the Phase 1 round-trips vs the Phase B design.
// ─────────────────────────────────────────────────────────────────────────────

import {
  SETUP_SCHEMA,
} from './cas-code-review-constants.js'

describe('SETUP_SCHEMA (Phase C — combined setup agent)', () => {
  test('is a JSON Schema object', () => {
    assert.equal(SETUP_SCHEMA.type, 'object')
  })

  test('requires intent_summary field', () => {
    assert.ok(SETUP_SCHEMA.required.includes('intent_summary'),
      'SETUP_SCHEMA must require intent_summary')
  })

  test('requires activate_security field', () => {
    assert.ok(SETUP_SCHEMA.required.includes('activate_security'),
      'SETUP_SCHEMA must require activate_security')
  })

  test('requires activate_adversarial field', () => {
    assert.ok(SETUP_SCHEMA.required.includes('activate_adversarial'),
      'SETUP_SCHEMA must require activate_adversarial')
  })

  test('requires fallow_skip_reason field', () => {
    assert.ok(SETUP_SCHEMA.required.includes('fallow_skip_reason'),
      'SETUP_SCHEMA must require fallow_skip_reason')
  })

  test('activate_security is boolean', () => {
    const prop = SETUP_SCHEMA.properties.activate_security
    assert.equal(prop.type, 'boolean',
      'activate_security must be boolean for deterministic activation')
  })

  test('activate_adversarial is boolean', () => {
    const prop = SETUP_SCHEMA.properties.activate_adversarial
    assert.equal(prop.type, 'boolean',
      'activate_adversarial must be boolean for deterministic activation')
  })

  test('activate_performance is boolean (optional conditional)', () => {
    const prop = SETUP_SCHEMA.properties.activate_performance
    assert.ok(prop, 'activate_performance property must exist')
    assert.equal(prop.type, 'boolean')
  })

  test('fallow_skip_reason allows null (fallow should run)', () => {
    const prop = SETUP_SCHEMA.properties.fallow_skip_reason
    const types = Array.isArray(prop.type) ? prop.type : [prop.type]
    assert.ok(types.includes('null') || prop.nullable === true,
      'fallow_skip_reason must allow null to signal fallow should run')
  })

  test('intent_summary is a string', () => {
    const prop = SETUP_SCHEMA.properties.intent_summary
    assert.equal(prop.type, 'string')
  })

  test('has additionalProperties: false for strict validation', () => {
    assert.equal(SETUP_SCHEMA.additionalProperties, false)
  })
})
