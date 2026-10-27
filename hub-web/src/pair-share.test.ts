import { describe, expect, it, vi } from "vitest";
import { sendPairingCode } from "./pair-share";

describe("sending a pairing code to whoever runs the machine (cas-a56c)", () => {
  it("uses the device's share sheet where there is one, and the clipboard where there is not", async () => {
    const share = vi.fn(async () => undefined);
    const writeText = vi.fn(async () => undefined);
    expect(await sendPairingCode({ share, clipboard: { writeText } }, "run cas hub authorize KQ7M-4XTR")).toBe("shared");
    expect(share).toHaveBeenCalledWith({ text: "run cas hub authorize KQ7M-4XTR" });
    expect(writeText).not.toHaveBeenCalled();
    expect(await sendPairingCode({ clipboard: { writeText } }, "run cas hub authorize KQ7M-4XTR")).toBe("copied");
    expect(writeText).toHaveBeenCalledWith("run cas hub authorize KQ7M-4XTR");
    // Dismissing the share sheet is not a failure, and copies nothing behind the person's back.
    const dismissed = vi.fn(async () => { throw new DOMException("Share canceled", "AbortError"); });
    expect(await sendPairingCode({ share: dismissed, clipboard: { writeText } }, "x")).toBe("cancelled");
    expect(await sendPairingCode({ clipboard: { writeText: async () => { throw new Error("denied"); } } }, "x")).toBe("failed");
  });
});
