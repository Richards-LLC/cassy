// cas-2077: admission and isolation for the deployed-cloud inbox journey.
// This module performs no network requests and never loads a user's login.
import assert from "node:assert/strict";
import { lstat, readFile } from "node:fs/promises";
import { basename, isAbsolute, join, resolve, sep } from "node:path";

export const CLOUD_ORIGIN = "https://petra-stella-cloud.vercel.app";
export const COMMANDER_ORIGIN = "https://hub.petrastella.io";
const fields = new Set([
  "CAS_TEST_CLOUD_URL", "CAS_TEST_TEAM_ID", "CAS_TEST_PROJECT_ID",
  "CAS_TEST_A_TOKEN", "CAS_TEST_B_TOKEN", "CAS_TEST_A_EMAIL", "CAS_TEST_B_EMAIL",
]);

function profileIdentity(profile) {
  assert(profile === "A" || profile === "B", "select sandbox profile A or B");
  const letter = profile.toLowerCase();
  return { accountId: `cas-test-profile-${letter}-v1`, email: `cas-test-${letter}@sandbox.petrastella.io` };
}

/** Parse Cloud #160's credential file as data, never shell code. */
export function parseSandboxProfile(text, profile = "B") {
  const expected = profileIdentity(profile);
  const values = new Map();
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const match = /^(?:export\s+)?([A-Z_]+)=(.*)$/.exec(line);
    assert(match && fields.has(match[1]), "unexpected sandbox credential field");
    assert(!values.has(match[1]), "duplicate sandbox credential field");
    let value = match[2];
    if (value.startsWith("'") || value.startsWith('"')) {
      assert(value.length >= 2 && value.at(-1) === value[0], "invalid sandbox credential quoting");
      value = value.slice(1, -1);
    }
    assert(!/[\s'"`$\\]/.test(value), "computed sandbox credentials are refused");
    values.set(match[1], value);
  }
  assert.equal(values.get("CAS_TEST_CLOUD_URL"), CLOUD_ORIGIN, "sandbox Cloud origin mismatch");
  assert.equal(values.get("CAS_TEST_TEAM_ID"), "cas-test-sandbox-v1", "sandbox team mismatch");
  assert.equal(values.get("CAS_TEST_PROJECT_ID"), "cas-test-project", "sandbox project mismatch");
  assert.equal(values.get(`CAS_TEST_${profile}_EMAIL`), expected.email, "sandbox email mismatch");
  const token = values.get(`CAS_TEST_${profile}_TOKEN`);
  assert(typeof token === "string" && /^psc_k1_[A-Za-z0-9_-]+$/.test(token), "sandbox user token missing or invalid");
  return { ...expected, profile, token, cloudOrigin: CLOUD_ORIGIN, projectId: "cas-test-project" };
}

export async function loadSandboxProfile(path, profile = "B") {
  const metadata = await lstat(path);
  assert(metadata.isFile() && (metadata.mode & 0o777) === 0o600, "sandbox credentials must be a regular 0600 file");
  return parseSandboxProfile(await readFile(path, "utf8"), profile);
}

/** Required before launching a browser, reading credentials or issuing HTTP. */
export function liveOptions(env) {
  assert.equal(env.CAS_2077_LIVE, "1", "CAS_2077_LIVE=1 is required");
  const profile = env.CAS_2077_PROFILE ?? "B";
  profileIdentity(profile);
  const root = env.CAS_2077_LIVE_ROOT;
  assert(root && isAbsolute(root) && basename(root).startsWith("cas-2077-live-"), "an absolute disposable cas-2077-live-* root is required");
  const artifacts = env.CAS_2077_LIVE_ARTIFACTS;
  assert(artifacts && isAbsolute(artifacts), "an absolute artifact directory is required");
  return { root: resolve(root), artifacts: resolve(artifacts), profile };
}

export function assertSandboxIdentity(identity, profile) {
  const expected = profileIdentity(profile);
  assert.equal(identity?.user_id, expected.accountId, "sandbox authenticated account mismatch");
  assert.equal(identity?.email, expected.email, "sandbox authenticated email mismatch");
}

/** Fulfill only the local app at the allowed origin; never route Cloud APIs. */
export function commanderAssetPath(dist, requestUrl) {
  const url = new URL(requestUrl);
  if (url.origin !== COMMANDER_ORIGIN || !url.pathname.startsWith("/commander/")) return null;
  let relative;
  try { relative = decodeURIComponent(url.pathname.slice("/commander/".length)); }
  catch { return null; }
  const base = resolve(dist);
  const target = resolve(base, relative || "index.html");
  return target.startsWith(`${base}${sep}`) ? target : null;
}

/** An isolated CLI process cannot inherit the factory or installed user login. */
export function isolatedCasEnv(root, env) {
  return {
    HOME: join(root, "home"),
    PATH: env.PATH ?? "/usr/bin:/bin",
    LANG: env.LANG ?? "C.UTF-8",
    TERM: env.TERM ?? "xterm-256color",
    CAS_ROOT: join(root, "project", ".cas"),
    CAS_USER_CLOUD_JSON: join(root, "home", ".cas", "cloud.json"),
    CAS_SKIP_FACTORY_TOOLING: "1",
    CAS_SUPPRESS_DUPLICATE_WARNING: "1",
  };
}
