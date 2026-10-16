import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

for (const file of ["operator-inbox-live-native.mjs", "operator-inbox-live.mjs"]) {
  test(`${file} refuses before credentials, HTTP, browser or disposable roots without opt-in`, () => {
    const child = spawnSync(process.execPath, [fileURLToPath(new URL(file, import.meta.url))], {
      env: { PATH: process.env.PATH, HOME: "/missing-cas-2077-home" }, encoding: "utf8", timeout: 5000,
    });
    assert.equal(child.status, 1);
    assert.match(child.stderr, /CAS_2077_LIVE=1 is required/);
    assert.doesNotMatch(child.stderr, /ENOENT|ERR_MODULE_NOT_FOUND|psc_k1_/);
  });
}
