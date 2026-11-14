import { describe, expect, it } from "vitest";
import { REFUSED_SEE_ABOVE, refusal, refusalSentence } from "./refusal";
import { outageRefusal, pairingRefusal } from "./connection-state-view";

describe("refusal (F6: plain reasons with a next step)", () => {
  it("names the control only for a control refusal, and never the absent header control (cas-3433)", () => {
    for (const detail of ["forbidden", "authorization refused", "lease expired", "This device is only observing"]) {
      expect(refusal(detail).action).toBe("take-control");
    }
    for (const detail of ["in_reply_to gone", "authentication required", "enqueue failed", "machine protocol error", undefined]) {
      expect(refusal(detail).action).toBeUndefined();
    }
    // cas-d15c: a refusal that waits for the session says so, so the view can
    // say Retry goes through once the session is live again.
    for (const detail of ["stream closed", "upstream_unavailable", outageRefusal("Atlas · Linux")]) {
      expect(refusal(detail).action).toBe("await-session");
    }
  });
});
