import { describe, expect, it } from "vitest";
import { isEditableElement, renderDecision, shellSignature, type ShellSignatureParts } from "./render-model";

const base: ShellSignatureParts = {
  machineId: "machine-1",
  session: "cas-src-fast-kestrel-6",
  machineIds: ["machine-1"],
  sessionKeys: ["machine-1/cas-src-fast-kestrel-6"],
  catalogLoaded: true,
  drawerOpen: false,
  attentionCollapsed: false,
  contextTab: "status",
  fleetEmpty: false,
  supervisor: "fast-kestrel-6",
  backLabel: undefined,
  compatibility: undefined,
  leaseHeldByMe: true,
  leaseController: "Daniel",
  controlDisabled: false,
  commandPaletteOpen: false,
  sessionPickerOpen: false,
  pairingView: "",
};

describe("shell signature", () => {
  it("is stable across a heartbeat that changes nothing structural", () => {
    expect(shellSignature({ ...base })).toBe(shellSignature({ ...base }));
  });

  for (const [name, change] of [
    ["the selected session", { session: "cas-src-other" }],
    ["the selected machine", { machineId: "machine-2" }],
    ["a machine joining the catalog", { machineIds: ["machine-1", "machine-2"] }],
    ["a session appearing", { sessionKeys: ["machine-1/a", "machine-1/b"] }],
    ["the catalog finishing its load", { catalogLoaded: false }],
    ["the machines drawer opening", { drawerOpen: true }],
    ["the attention panel collapsing", { attentionCollapsed: true }],
    ["the context tab", { contextTab: "attention" }],
    ["the fleet becoming empty", { fleetEmpty: true }],
    ["the supervisor target", { supervisor: "other-supervisor" }],
    ["a back target appearing", { backLabel: "Back to machine-1" }],
    ["a compatibility warning", { compatibility: "Hub is version-skewed" }],
    ["control being taken", { leaseHeldByMe: false }],
    ["a different controller", { leaseController: "someone-else" }],
    ["control becoming unavailable", { controlDisabled: true }],
    ["the command palette opening", { commandPaletteOpen: true }],
    ["the session picker opening", { sessionPickerOpen: true }],
    ["a pairing invitation arriving", { pairingView: "relay-request|ABCD-1234||Waiting for a machine to claim the code…|" }],
    ["cancellation cleanup becoming outstanding", { pairingView: "|||cleanup-failed" }],
  ] as [string, Partial<ShellSignatureParts>][]) {
    it(`changes with ${name}`, () => {
      expect(shellSignature({ ...base, ...change })).not.toBe(shellSignature(base));
    });
  }

  it("does not change when only heartbeat data moved", () => {
    // Latency, attention counts, the status payload and the message status are
    // absent from the parts by construction: there is nowhere to put them.
    const parts = { ...base } as ShellSignatureParts & Record<string, unknown>;
    parts.latencyMs = 41;
    parts.attentionCount = 12;
    parts.statusPayload = { agents: [1, 2, 3] };
    expect(shellSignature(parts)).toBe(shellSignature(base));
  });

  it("separates its fields so two changes cannot cancel out", () => {
    const shifted = shellSignature({ ...base, machineId: "machine", session: "-1cas-src-fast-kestrel-6" });
    expect(shifted).not.toBe(shellSignature(base));
  });
});

describe("render decision", () => {
  it("updates regions in place when nothing structural changed", () => {
    expect(renderDecision({ signatureChanged: false, composing: false })).toBe("regions");
  });

  it("keeps updating regions while the operator types, never rebuilding under them", () => {
    expect(renderDecision({ signatureChanged: false, composing: true })).toBe("regions");
  });

  it("rebuilds the shell when structure changed and nothing is being typed into", () => {
    expect(renderDecision({ signatureChanged: true, composing: false })).toBe("shell");
  });

  it("defers a structural rebuild that would land mid-sentence", () => {
    expect(renderDecision({ signatureChanged: true, composing: true })).toBe("defer");
  });
});

describe("editable element detection", () => {
  it("treats the composer, palette query and pairing inputs as being typed into", () => {
    expect(isEditableElement({ tagName: "TEXTAREA" })).toBe(true);
    expect(isEditableElement({ tagName: "input" })).toBe(true);
    expect(isEditableElement({ tagName: "SELECT" })).toBe(true);
    expect(isEditableElement({ tagName: "DIV", isContentEditable: true })).toBe(true);
  });

  it("does not treat a button or the body as composing", () => {
    expect(isEditableElement({ tagName: "BUTTON" })).toBe(false);
    expect(isEditableElement({ tagName: "BODY" })).toBe(false);
    expect(isEditableElement(null)).toBe(false);
    expect(isEditableElement(undefined)).toBe(false);
  });
});

describe("render decision and the pairing dialog", () => {
  it("defers a structural change while any field in the app has focus", () => {
    expect(renderDecision({ signatureChanged: true, composing: true })).toBe("defer");
    expect(renderDecision({ signatureChanged: true, composing: true, pairingStepChanged: false, focusInPairingDialog: true })).toBe("defer");
  });

  it("rebuilds for a pairing step change when the focus is inside the pairing dialog", () => {
    // Submit with Device label focused, then the exchange succeeds or is
    // cancelled: the new step is what the operator asked for.
    expect(renderDecision({ signatureChanged: true, composing: true, pairingStepChanged: true, focusInPairingDialog: true })).toBe("shell");
  });

  it("never lets a pairing step change take the composer's keyboard", () => {
    expect(renderDecision({ signatureChanged: true, composing: true, pairingStepChanged: true, focusInPairingDialog: false })).toBe("defer");
  });

  it("stays on the region path when nothing structural changed, whatever has focus", () => {
    expect(renderDecision({ signatureChanged: false, composing: true, pairingStepChanged: false, focusInPairingDialog: true })).toBe("regions");
  });
});
