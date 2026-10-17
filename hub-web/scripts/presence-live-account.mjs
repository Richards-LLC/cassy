import assert from "node:assert/strict";

// Cloud #160's fixed disposable profile; never accept arbitrary account IDs.
export function assertSandboxIdentity(identity, accountId) {
  assert.equal(accountId, "cas-test-profile-a-v1", "only sandbox profile A is allowed");
  assert.equal(identity.user_id, accountId, "test account id mismatch");
  assert.equal(identity.email, "cas-test-a@sandbox.petrastella.io", "only sandbox profile A is allowed");
}
