import { describe, expect, it } from "vitest";
import { accountEnrollmentLabel } from "./installation-inventory";

describe("installation inventory account row (cas-4634)", () => {
  it("says plainly whether the hub verified this installation into an operator inbox", () => {
    expect(accountEnrollmentLabel(undefined)).toBe("Not in an operator inbox");
    expect(accountEnrollmentLabel({ state: "unenrolled" })).toBe("Not in an operator inbox");
    expect(
      accountEnrollmentLabel({
        state: "enrolled",
        account_id: "acct-1",
        relay_device_id: "dev-1",
        grant_generation: "2",
        feed_generation: "1",
        epoch: "4",
        verified_at: "2026-10-05T21:00:00Z",
      }),
    ).toBe("In your operator inbox");
  });
});
