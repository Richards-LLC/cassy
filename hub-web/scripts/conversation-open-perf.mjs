#!/usr/bin/env node
// One finite command owns the server/browser; no production service is changed.
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { cpus, totalmem, loadavg } from "node:os";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const { values } = parseArgs({ options: {
  db: { type: "string" }, session: { type: "string" }, output: { type: "string" },
  capture: { type: "string" }, repeats: { type: "string", default: "3" },
} });
if (!values.output || (!values.capture && (!values.db || !values.session))) {
  throw Error("Usage: node scripts/conversation-open-perf.mjs --db /project/.cas/cas.db --session SESSION --output TASK_ARTIFACT_DIR [--repeats 3]\nOr --capture PRIVATE_CAPTURE_JSON to replay the identical input.");
}
const repeats = Number(values.repeats);
if (!Number.isSafeInteger(repeats) || repeats < 1 || repeats > 10) throw Error("repeats must be 1..10");
const output = resolve(values.output);
mkdirSync(output, { recursive: true });
const capture = values.capture ? resolve(values.capture) : resolve(output, "private-history.json");
const machine = { cpu: cpus()[0]?.model, logical_cpus: cpus().length, memory_bytes: totalmem(), load_average_at_start: loadavg() };
function run(command, args, env = {}) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", env: { ...process.env, ...env } });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
if (!values.capture) run("python3", ["scripts/conversation-perf-capture.py", "--db", resolve(values.db), "--session", values.session, "--output", capture]);
// Source maps change only a trailing comment, and allow CPU samples to name source files.
run("npm", ["run", "build", "--", "--sourcemap"]);
run("npx", ["playwright", "test", "--config", "e2e/conversation-perf.config.ts", "conversation-open.perf.ts", "--workers=1", `--repeat-each=${repeats}`], {
  CONVERSATION_PERF_CAPTURE: capture, CONVERSATION_PERF_OUTPUT: output,
});
const receipts = readdirSync(output).filter((name) => /^(desktop|phone)-\d+$/.test(name)).map((name) => JSON.parse(readFileSync(resolve(output, name, "receipt.json"), "utf8")));
const metrics = ["first_turn_painted_ms", "latest_turn_painted_ms", "interactive_upper_bound_ms", "input_ack_ms", "history_requests", "history_payload_bytes", "json_parse_ms", "script_ms", "layout_ms", "style_ms", "dom_elements", "dom_nodes", "rendered_turns", "heap_used_bytes", "long_task_total_ms", "long_task_max_ms"];
const summary = { machine, label: receipts[0].label, source_head: receipts[0].source_head, source_session: receipts[0].source_session, raw_queue_rows: receipts[0].raw_queue_rows, commander_turns: receipts[0].commander_turns, runs: [] };
for (const profile of ["desktop", "phone"]) for (const phase of ["initial", "accumulated"]) {
  const group = receipts.filter((receipt) => receipt.profile.name === profile).map((receipt) => receipt.results[phase]);
  summary.runs.push({ profile, phase, repetitions: group.length, metrics: Object.fromEntries(metrics.map((metric) => {
    const ordered = group.map((run) => run[metric]).filter((value) => typeof value === "number").sort((a, b) => a - b);
    return [metric, ordered.length ? { median: ordered[Math.floor(ordered.length / 2)], min: ordered[0], max: ordered.at(-1) } : null];
  })) });
}
writeFileSync(resolve(output, "summary.json"), JSON.stringify(summary, null, 2));
console.log(`PASS: ${receipts.length} profile receipts; summary and Chrome traces: ${output}`);
