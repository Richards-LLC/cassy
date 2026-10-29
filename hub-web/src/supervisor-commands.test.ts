// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { SlashCommands, SupervisorCommandSession, commandsForHarness, commandReceiptNode } from "./supervisor-commands";

describe("supervisor slash commands", () => {
  it("filters the real composer palette and keeps arbitrary slash text out of message sends", async () => {
    document.body.innerHTML = '<section><textarea aria-label="Message"></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn().mockResolvedValue(undefined);
    const commands = new SlashCommands(input, { harness: () => "codex", unavailable: () => undefined, label: () => "test supervisor", run });
    input.value = "/us"; input.dispatchEvent(new Event("input"));
    expect(document.querySelectorAll("[data-supervisor-command]")).toHaveLength(1);
    expect(document.querySelector("[data-supervisor-command]")?.textContent).toContain("Usage");
    const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    input.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(true);
    await Promise.resolve();
    expect(run).toHaveBeenCalledWith("usage", false);
    input.value = "/shell whoami";
    expect(commands.submit()).toBe(true);
    expect(run).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).toContain("No supported command");
    expect(commandsForHarness("grok")).toEqual([]);
  });

  it("requires a separate destructive confirmation naming the supervisor", async () => {
    document.body.innerHTML = '<section><textarea></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn().mockResolvedValue(undefined);
    const commands = new SlashCommands(input, { harness: () => "claude", unavailable: () => undefined, label: () => "test supervisor", run });
    input.value = "/clear";
    commands.submit();
    expect(run).not.toHaveBeenCalled();
    const confirm = document.querySelector<HTMLButtonElement>("[data-command-confirm]")!;
    expect(document.querySelector('[role="alertdialog"]')?.textContent).toContain("test supervisor");
    confirm.click(); await Promise.resolve();
    expect(run).toHaveBeenCalledWith("clear", true);
  });

  it("keeps submission separate from execution and escapes native output", () => {
    const row = commandReceiptNode({ client_ref: "cmd-1", command: "compact", phase: "submitted", detail: "Command submitted; harness execution is not confirmed yet.", output: "<unsafe>", interactive: false });
    expect(row.textContent).toContain("not confirmed");
    expect(row.querySelector("unsafe")).toBeNull();
    expect(row.querySelector("pre")?.textContent).toBe("<unsafe>");
  });
});

describe("supervisor command receipts", () => {
  it("polls only the outcome and stops after observation, without replaying a mutation", async () => {
    vi.useFakeTimers();
    try {
      const send = vi.fn().mockReturnValue(true);
      const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {} });
      await session.run("compact", false);
      const ref = [...session.receipts.keys()][0];
      await vi.advanceTimersByTimeAsync(2_000);
      expect(send.mock.calls.filter(([message]) => "SupervisorCommand" in message)).toHaveLength(1);
      expect(send.mock.calls.filter(([message]) => "SupervisorCommandStatus" in message)).toHaveLength(2);
      session.receive({ client_ref: ref, command: "compact", phase: "observed", detail: "Harness started compact context", output: "", interactive: false });
      await vi.advanceTimersByTimeAsync(5_000);
      expect(send).toHaveBeenCalledTimes(3);
    } finally { vi.useRealTimers(); }
  });
});
