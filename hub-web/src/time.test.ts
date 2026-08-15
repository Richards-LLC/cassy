import { describe, expect, it } from "vitest";
import { absoluteTimestamp, relativeTimestamp } from "./time";

describe("Commander timestamps", () => {
  it("uses compact relative labels and a stable absolute hover value", () => {
    const now = Date.parse("2026-08-15T04:00:00.000Z");
    expect(relativeTimestamp(now - 120_000, now)).toBe("2m");
    expect(relativeTimestamp(now - 3_600_000, now)).toBe("1h");
    expect(absoluteTimestamp(now)).toBe("2026-08-15T04:00:00.000Z");
  });
});
