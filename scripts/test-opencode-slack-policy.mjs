#!/usr/bin/env node
// Exercise the shipped OpenCode plugin source with a fixture hook executable.
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const source = await readFile(new URL('../crates/cas-mux/src/opencode.rs', import.meta.url), 'utf8');
const pluginSource = source.match(/pub const OPENCODE_PLUGIN_SOURCE: &str = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(pluginSource, 'plugin is embedded in cas-mux');
const root = await mkdtemp(join(tmpdir(), 'cas-slack-policy-'));
const bin = join(root, 'bin');
await mkdir(bin);
const captured = join(root, 'input.json');
const stub = `#!${process.execPath}
import { writeFileSync } from 'node:fs';
let input = '';
for await (const chunk of process.stdin) input += chunk;
writeFileSync(process.env.CASSY_TEST_CAPTURE, input);
if (process.env.CASSY_TEST_DECISION === 'error') process.exit(1);
if (process.env.CASSY_TEST_DECISION === 'invalid') { console.log('not-json'); process.exit(0); }
if (process.env.CASSY_TEST_DECISION === 'deny') console.log(JSON.stringify({hookSpecificOutput:{permissionDecision:'deny', permissionDecisionReason:'Use violet.violet_post and the violet skill; reads use violet.violet_read'}}));
else console.log('{}');
`;
// Node treats the extensionless executable as CommonJS; use a CJS wrapper.
await writeFile(join(bin, 'hook.mjs'), stub.split('\n').slice(1).join('\n'));
await writeFile(join(bin, 'cas'), `#!${process.execPath}\nimport('file://' + __dirname + '/hook.mjs');\n`, { mode: 0o755 });
const previous = Object.fromEntries(['PATH', 'CAS_SESSION_ID', 'CAS_ROOT', 'CASSY_TEST_DECISION', 'CASSY_TEST_CAPTURE'].map((key) => [key, process.env[key]]));
process.env.PATH = bin;
process.env.CAS_SESSION_ID = '';
process.env.CAS_ROOT = root;
process.env.CASSY_TEST_CAPTURE = captured;
await writeFile(join(root, 'plugin.mjs'), pluginSource);
const { CassyPlugin } = await import(pathToFileURL(join(root, 'plugin.mjs')).href);
const hooks = await CassyPlugin();
const before = (tool, args = {}) => hooks['tool.execute.before']({ tool, sessionID: 'ses_test' }, { args });
after(async () => {
  for (const [key, value] of Object.entries(previous)) {
    if (value === undefined) delete process.env[key]; else process.env[key] = value;
  }
  await rm(root, { recursive: true, force: true });
});

test('denial propagates before the Slack tool can run', async () => {
  process.env.CASSY_TEST_DECISION = 'deny';
  await assert.rejects(before('slack_send_message', { text: 'release note' }), /violet\.violet_post/);
  const input = JSON.parse(await readFile(captured, 'utf8'));
  assert.equal(input.hook_event_name, 'PreToolUse');
  assert.equal(input.tool_name, 'slack_send_message');
  assert.deepEqual(input.tool_input, { text: 'release note' });
});
test('Violet tools bypass unrelated-hook failures', async () => {
  process.env.PATH = join(root, 'absent');
  await before('violet_violet_post');
  await before('violet_violet_read');
  await before('mcp__violet__slack_post');
  await before('cas_mcp_execute', { code: 'violet.violet_post({})' });
  await before('cas_mcp_execute', { code: 'violet.violet_post' });
  await before('cas_mcp_execute', { code: JSON.stringify({ server: 'violet', tool: 'violet_post', args: {} }) });
  process.env.PATH = bin;
});
test('read allowed by the common policy can execute', async () => {
  process.env.CASSY_TEST_DECISION = 'allow';
  await before('slack_read_channel');
});
test('proxy code and arguments travel as JSON, without shell execution', async () => {
  process.env.CASSY_TEST_DECISION = 'deny';
  const code = 'slack.send_message({"text":"$(touch should-never-run)"})';
  await assert.rejects(before('cas_mcp_execute', { code }), /violet\.violet_post/);
  assert.deepEqual(JSON.parse(await readFile(captured, 'utf8')).tool_input, { code });
});
test('hook opt-out and Violet proxy allow decisions are honored', async () => {
  process.env.CASSY_TEST_DECISION = 'allow';
  await before('slack_send_message');
  await before('cas_mcp_execute', { code: 'violet.violet_post({})' });
});
test('missing policy executable refuses Slack dispatch', async () => {
  process.env.PATH = join(root, 'absent');
  await assert.rejects(before('slack_send_message'), /policy unavailable/);
  process.env.PATH = bin;
});
test('invalid policy output refuses Slack dispatch', async () => {
  process.env.CASSY_TEST_DECISION = 'invalid';
  await assert.rejects(before('slack_send_message'), /Invalid Cassy Slack policy response/);
});
test('failed policy executable refuses Slack dispatch', async () => {
  process.env.CASSY_TEST_DECISION = 'error';
  await assert.rejects(before('slack_send_message'), /policy failed/);
});
