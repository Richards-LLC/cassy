import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { mkdtemp, writeFile, chmod, symlink, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  CLOUD_ORIGIN, COMMANDER_ORIGIN, parseSandboxProfile, loadSandboxProfile,
  liveOptions, assertSandboxIdentity, commanderAssetPath, isolatedCasEnv,
} from "./operator-inbox-live-safety.mjs";

const fixture = () => `export CAS_TEST_CLOUD_URL='${CLOUD_ORIGIN}'
CAS_TEST_TEAM_ID=cas-test-sandbox-v1
CAS_TEST_PROJECT_ID=cas-test-project
CAS_TEST_A_EMAIL=cas-test-a@sandbox.petrastella.io
CAS_TEST_B_EMAIL=cas-test-b@sandbox.petrastella.io
CAS_TEST_A_TOKEN=psc_k1_fixture_A
CAS_TEST_B_TOKEN=psc_k1_fixture_B
`;

test("live entrypoints refuse before credentials, HTTP, browser or scratch without opt-in", () => {
  for (const file of ["operator-inbox-live-native.mjs", "operator-inbox-live.mjs"]) {
    const child = spawnSync(process.execPath, [fileURLToPath(new URL(file, import.meta.url))], {
      env: { PATH: process.env.PATH, HOME: "/missing-cas-2077-home" }, encoding: "utf8", timeout: 5000,
    });
    assert.equal(child.status, 1, file);
    assert.match(child.stderr, /CAS_2077_LIVE=1 is required/, file);
    assert.doesNotMatch(child.stderr, /ENOENT|ERR_MODULE_NOT_FOUND|psc_k1_/, file);
  }
});

test("default selection loads only the approved B account; A is explicit", () => {
  const b = parseSandboxProfile(fixture());
  assert.equal(b.accountId, "cas-test-profile-b-v1");
  assert.equal(b.token, "psc_k1_fixture_B");
  assert.equal(parseSandboxProfile(fixture(), "A").token, "psc_k1_fixture_A");
});

test("a real account, other origin/team/project, shell program or admin key refuses before use", () => {
  for (const source of [
    fixture().replace(CLOUD_ORIGIN, "https://elsewhere.example"),
    fixture().replace("cas-test-sandbox-v1", "real-team"),
    fixture().replace("cas-test-project", "real-project"),
    fixture().replace("cas-test-b@sandbox.petrastella.io", "owner@example.com"),
    fixture().replace("psc_k1_fixture_B", "$(cat credential)"),
    fixture() + "ADMIN_SECRET=never-use-admin\n",
    fixture() + "CAS_TEST_B_TOKEN=psc_k1_duplicate\n",
  ]) assert.throws(() => parseSandboxProfile(source));
  assert.throws(() => parseSandboxProfile(fixture(), "production"));
});

test("profile loading refuses loose permissions and symlinks", async () => {
  const root = await mkdtemp(join(tmpdir(), "cas-2077-safety-"));
  try {
    const file = join(root, "profiles.env");
    await writeFile(file, fixture(), { mode: 0o600 });
    assert.equal((await loadSandboxProfile(file)).profile, "B");
    await chmod(file, 0o644);
    await assert.rejects(loadSandboxProfile(file), /0600/);
    await chmod(file, 0o600);
    await symlink(file, join(root, "link.env"));
    await assert.rejects(loadSandboxProfile(join(root, "link.env")), /regular/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("live opt-in and disposable paths are mandatory without importing Playwright", () => {
  assert.throws(() => liveOptions({}), /CAS_2077_LIVE/);
  for (const root of ["relative", "/home/person", "/tmp/another-run"]) {
    assert.throws(() => liveOptions({ CAS_2077_LIVE: "1", CAS_2077_LIVE_ROOT: root, CAS_2077_LIVE_ARTIFACTS: "/tmp/evidence" }));
  }
  assert.equal(liveOptions({ CAS_2077_LIVE: "1", CAS_2077_LIVE_ROOT: "/tmp/cas-2077-live-test", CAS_2077_LIVE_ARTIFACTS: "/tmp/evidence" }).profile, "B");
});

test("the authenticated API identity must match the selected disposable account", () => {
  const profile = parseSandboxProfile(fixture());
  assertSandboxIdentity({ user_id: profile.accountId, email: profile.email }, profile.profile);
  assert.throws(() => assertSandboxIdentity({ user_id: "real-user", email: profile.email }, "B"));
  assert.throws(() => assertSandboxIdentity({ user_id: profile.accountId, email: "real@example.com" }, "B"));
});

test("candidate asset routing cannot intercept Cloud, hub API or escape dist", () => {
  const dist = join(tmpdir(), "cas-2077-dist");
  assert.equal(commanderAssetPath(dist, `${COMMANDER_ORIGIN}/commander/`), join(dist, "index.html"));
  assert.equal(commanderAssetPath(dist, `${COMMANDER_ORIGIN}/commander/app.js?v=1`), join(dist, "app.js"));
  for (const url of [
    `${CLOUD_ORIGIN}/api/operator/feed`, `${COMMANDER_ORIGIN}/v1/health`,
    `${COMMANDER_ORIGIN}/commander/%2e%2e%2fsecret`, `${COMMANDER_ORIGIN}/commander/%zz`,
  ]) assert.equal(commanderAssetPath(dist, url), null);
});

test("CLI children cannot inherit production authority, factory identity or the live hub home", () => {
  const child = isolatedCasEnv("/tmp/cas-2077-live-test", {
    PATH: "/usr/bin", HOME: "/real-user", CAS_ROOT: "/real-project/.cas",
    CAS_AGENT_ROLE: "supervisor", CAS_FACTORY_SESSION: "real-factory",
    ADMIN_SECRET: "private", CAS_TEST_A_TOKEN: "private", CAS_USER_CLOUD_JSON: "/real-login",
  });
  assert.equal(child.HOME, "/tmp/cas-2077-live-test/home");
  assert.equal(child.CAS_ROOT, "/tmp/cas-2077-live-test/project/.cas");
  for (const name of ["CAS_AGENT_ROLE", "CAS_FACTORY_SESSION", "ADMIN_SECRET", "CAS_TEST_A_TOKEN"]) assert.equal(child[name], undefined);
});
