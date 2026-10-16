import { test } from "node:test";
import assert from "node:assert/strict";
import { assertSandboxIdentity } from "./presence-live-account.mjs";

test("live presence accepts sandbox A and refuses substituted identities", () => {
  const sandbox = { user_id: "cas-test-profile-a-v1", email: "cas-test-a@sandbox.petrastella.io" };
  assert.doesNotThrow(() => assertSandboxIdentity(sandbox, sandbox.user_id));
  assert.throws(() => assertSandboxIdentity(sandbox, "production-id"));
  assert.throws(() => assertSandboxIdentity({ ...sandbox, user_id: "production-id" }, sandbox.user_id));
  assert.throws(() => assertSandboxIdentity({ ...sandbox, email: "operator@example.com" }, sandbox.user_id));
});
