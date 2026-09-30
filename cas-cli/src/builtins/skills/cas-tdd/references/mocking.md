---
metadata:
  managed_by: cas
---

# Boundary mocks and per-operation interfaces

Adapted from mattpocock/skills `tdd/mocking.md`, MIT © 2026 Matt Pocock.

Use a real in-process implementation for code you own and a real test database
when practical. Inject a substitute at an external system, filesystem, clock,
randomness or separately-owned transport seam. Assert the caller's result,
rather than the private steps used to obtain it.

## Inject a port

```typescript
type PaymentPort = {
  charge(amount: number): Promise<{ receipt: string }>;
};

async function checkout(total: number, payments: PaymentPort) {
  const payment = await payments.charge(total);
  return { status: "confirmed", receipt: payment.receipt };
}

// The external payment result is controlled; checkout's behavior is real.
const payments: PaymentPort = {
  charge: async () => ({ receipt: "receipt-1" }),
};
expect(await checkout(15, payments)).toEqual({
  status: "confirmed", receipt: "receipt-1",
});
```

Creating the vendor client inside `checkout` would couple setup to credentials
and network state. Mocking `checkout` itself would remove the behavior under
observation. The production transport adapter and the test substitute fill the
same port; keep that port small and tied to real operations.

## SDK-style operations, not a generic fetch switch

```typescript
type AccountsPort = {
  getUser(id: string): Promise<{ id: string; name: string }>;
  getOrders(userId: string): Promise<Array<{ id: string }>>;
  createOrder(userId: string): Promise<{ id: string }>;
};

const accounts: AccountsPort = {
  getUser: async id => ({ id, name: "Alice" }),
  getOrders: async () => [],
  createOrder: async () => ({ id: "order-1" }),
};
```

Each operation has a typed input/result and one independently controlled mock.
A generic `fetch(endpoint, options)` fake needs conditional URL/method routing
and can return the wrong shape while hiding that error in shared setup. Keep
HTTP details in the transport adapter and exercise them in its boundary tests.
The operation fixture controls success, empty and failure cases directly.
Framework DI/provider overrides remain appropriate for this external port when
the consuming module runs normally; substituting an internal collaborator to
assert call choreography remains implementation-coupled.
