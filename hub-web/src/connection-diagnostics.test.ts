import { describe, expect, it } from "vitest";
import { CAUSE_COPY, httpRefusalCause } from "./connection-diagnostics";

describe("httpRefusalCause", () => {
  it("names a hub that briefly can't check pairings, and keeps other refusals as refused (cas-6348)", () => {
    // The hub's 503 since cas-cb94: {error: "auth_store_unavailable", reason, retryable: true}.
    const cause = httpRefusalCause(503, "auth_store_unavailable");
    expect(cause).toBe("hub_auth_store_unavailable");
    expect(CAUSE_COPY[cause].title).toBe("Hub is temporarily unable to check pairings");
    expect(CAUSE_COPY[cause].action).toBe("Commander is retrying by itself; this device's pairing is intact.");
    expect(httpRefusalCause(503, undefined, "auth_store_unavailable")).toBe("hub_auth_store_unavailable");
    expect(httpRefusalCause(503, "health_probe")).toBe("health_http_unavailable");
    for (const [status, code] of [[503, undefined], [500, "auth_store_unavailable"], [409, "conflict"]] as const) {
      expect(httpRefusalCause(status, code)).toBe("http_refused");
    }
    expect(CAUSE_COPY.http_refused.title).toBe("HTTP request was refused");
  });
});
