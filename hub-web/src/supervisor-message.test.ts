import { describe, expect, it } from "vitest";
import { supervisorMessage, supervisorTarget } from "./supervisor-message";
import type { HubSession } from "./types";

const session = (supervisor: string): HubSession => ({
  name: "factory-live",
  supervisor,
  workers: ["worker-1"],
  liveness: "live",
});

describe("Commander supervisor composer targeting", () => {
  it("targets the selected session supervisor exactly", () => {
    expect(supervisorTarget(session("patient-lynx-59"))).toBe("patient-lynx-59");
    expect(supervisorMessage("patient-lynx-59", "Please review the mobile state"))
      .toMatchObject({ SendMessage: { target: "patient-lynx-59", text: "Please review the mobile state" } });
  });

  it("does not invent a fallback target for a session without a supervisor", () => {
    expect(supervisorTarget(session("  "))).toBeUndefined();
  });
});
