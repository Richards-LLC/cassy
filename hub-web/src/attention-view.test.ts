// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { renderAttentionPanel } from "./attention-view";
import type { AttentionItem } from "./types";

const now = Date.parse("2026-09-07T12:00:00Z");
const event = (id: string, kind = "daemon_disconnected"): AttentionItem => ({
  id, kind, machineId: "machine", machineLabel: "Workstation",
  session: `a-long-session-codename-${id}`, message: "Inspect the transport and reconnect when the machine is reachable.",
  createdAt: "2026-09-07T11:59:00Z",
});
const callbacks = () => ({ dismiss: vi.fn(), act: vi.fn(), copy: vi.fn() });

describe("attention timeline", () => {
  it("keeps the empty timeline quiet without count badges", () => {
    const root = document.createElement("div");
    renderAttentionPanel(root, [], callbacks(), { now });
    expect(root.querySelector(".attention-empty")?.textContent).toContain("All clear");
    expect(root.querySelector(".attention-counts")).toBeNull();
    expect(root.querySelector(".attention-item")).toBeNull();
  });

  it("gives all twelve events one primary text action and a timestamp", () => {
    const root = document.createElement("div");
    renderAttentionPanel(root, Array.from({ length: 12 }, (_, i) => event(String(i))), callbacks(), { now });
    const items = root.querySelectorAll(".attention-item");
    expect(items).toHaveLength(12);
    for (const item of items) {
      expect(item.querySelectorAll(".attention-action")).toHaveLength(1);
      expect(item.querySelector(".attention-action")?.textContent).toBe("Retry");
      expect(item.querySelector("time")?.getAttribute("datetime")).toBe("2026-09-07T11:59:00Z");
      expect(item.querySelector(".attention-explicit-dismiss")?.textContent).toBe("Dismiss");
    }
  });

  it("shows Dismiss as the primary action when no recovery action exists", () => {
    const root = document.createElement("div");
    const cb = callbacks();
    const item = event("one", "checkpoint");
    renderAttentionPanel(root, [item], cb, { now });
    const dismiss = root.querySelector<HTMLButtonElement>(".attention-action")!;
    expect(dismiss.textContent).toBe("Dismiss");
    dismiss.click();
    expect(cb.dismiss).toHaveBeenCalledWith([item]);
  });

  it("states pending enrichment without an animated decoration", () => {
    const root = document.createElement("div");
    renderAttentionPanel(root, [{ ...event("pending", "unfamiliar_event"), enrichmentPending: true }], callbacks(), { now });
    expect(root.querySelector(".attention-enriching")?.textContent).toBe("Enriching…");
  });

  it("retains action-before-dismiss ordering and expands/collapses groups", async () => {
    const root = document.createElement("div");
    const cb = callbacks();
    const item = event("one");
    renderAttentionPanel(root, [item], cb, { now });
    root.querySelector<HTMLButtonElement>(".attention-action")!.click();
    expect(cb.act).toHaveBeenCalledWith(item, "retry");
    await Promise.resolve();
    expect(cb.dismiss).toHaveBeenCalledWith([item]);
    const toggle = root.querySelector<HTMLButtonElement>(".attention-group-toggle")!;
    const body = root.querySelector<HTMLElement>(".attention-group-body")!;
    toggle.click();
    expect(body.hidden).toBe(true);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    toggle.click();
    expect(body.hidden).toBe(false);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
  });
});
