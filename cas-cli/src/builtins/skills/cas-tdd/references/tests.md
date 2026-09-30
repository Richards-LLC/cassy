---
metadata:
  managed_by: cas
---

# Test examples and audit patterns

Adapted from mattpocock/skills `tdd/tests.md`, MIT © 2026 Matt Pocock.
Examples use a test runner's `test`/`expect` vocabulary and illustrative public
functions; they are design examples, not a runnable project suite.

## Public behavior versus internal choreography

```typescript
// Good: observable capability through the public interface.
test("user can retrieve a created account", async () => {
  const account = await accounts.create({ name: "Alice" });
  expect((await accounts.get(account.id)).name).toBe("Alice");
});

// Anti-example: an internal call can happen while the user operation fails.
test("create calls repository.insert", async () => {
  await accounts.create({ name: "Alice" });
  expect(repository.insert).toHaveBeenCalled();
});
```

Inspect through the same interface a caller uses. Reading storage directly
would bypass the retrieval behavior promised by the first test.

## Independent expected values

```typescript
// Good: 15 comes from the specified example, not the production algorithm.
expect(calculateTotal([{ price: 10 }, { price: 5 }])).toBe(15);

// Anti-example: repeats production's sum and can copy its defect.
const expected = items.reduce((sum, item) => sum + item.price, 0);
expect(calculateTotal(items)).toBe(expected);

// Anti-example: proves nothing even if the constant is wrong.
expect(RETRY_LIMIT).toBe(RETRY_LIMIT);

// Change detector: can fail on an edit but does not exercise retries.
expect(RETRY_LIMIT).toBe(3);
```

For a promised three-attempt retry contract, exercise a failing external
operation and assert the public outcome at exhaustion against that contract.
Count external attempts only when the count itself is observable behavior.
A hand-derived snapshot that copies production's formatting or mapping repeats
its assumptions. Capture observed public output from an independently checked
example, review the snapshot change, and keep its contract explicit.

## Three source/prose audit patterns

| Pattern | Why it fails as behavioral proof | Replacement or intentional contract |
| --- | --- | --- |
| Reading a `.rs` file with `include_str!`/filesystem reads and asserting on its text | Correct text can ship in dead or unwired code. | Exercise the exported behavior. For genuine cross-file wiring, keep a structural check with a specific `// pin: <reason>`. |
| Comparing `.find()` positions or asserting line order in source | Source order can change without changing dispatch or execution order. | Send inputs through the real handler and assert its observable effects; pin textual order only if a source consumer requires it. |
| Asserting a phrase in a skill/doc | Rewording can break the test while behavior stays the same; the phrase alone cannot prove execution. | Check routing/installation/parser behavior, or use a reasoned contract registry for safety/tool strings another consumer actually requires. |

In the Cassy source repository, run `python3 scripts/check-test-shape.py` from
the checkout for mechanical source-as-text and constant/literal violations;
the scoped entry point accepts `--changed-since <base>`. Other projects use
their own equivalent lint. A reasoned pin documents the actual consumer,
not a preference to freeze wording. Prose-pin consolidation belongs in the
contract registry; do not create another scattered literal assertion.
