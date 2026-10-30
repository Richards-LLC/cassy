// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { SlashCommands, SupervisorCommandSession, commandsForHarness, commandReceiptNode, updateCommandReceipts, type CommandReceipt } from "./supervisor-commands";

describe("supervisor slash commands", () => {
  it("filters command suggestions but submits only an exact supported command", async () => {
    document.body.innerHTML = '<section><textarea aria-label="Message"></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn().mockResolvedValue(undefined);
    const commands = new SlashCommands(input, { harness: () => "codex", unavailable: () => undefined, label: () => "test supervisor", run });
    input.value = "/us"; input.dispatchEvent(new Event("input"));
    expect(document.querySelectorAll("[data-supervisor-command]")).toHaveLength(1);
    expect(document.querySelector("[data-supervisor-command]")?.textContent).toContain("Usage");
    const partial = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    input.dispatchEvent(partial);
    expect(partial.defaultPrevented).toBe(false);
    expect(run).not.toHaveBeenCalled();
    input.value = "/usage"; input.dispatchEvent(new Event("input"));
    const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    input.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(true);
    await Promise.resolve();
    expect(run).toHaveBeenCalledWith("usage", false);
    expect(commandsForHarness("grok")).toEqual([]);
    commands.dispose();
  });

  it("lets pasted paths and unsupported slash text reach the normal composer send", () => {
    document.body.innerHTML = '<section><textarea aria-label="Message"></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn();
    const commands = new SlashCommands(input, { harness: () => "codex", unavailable: () => "No command control", label: () => "test supervisor", run });
    for (const text of ["/home/pippenz/notes.md", "/mnt/notes.md review this", "/shell whoami", "/usage-report", "//usage", " /usage"]) {
      input.value = text; input.dispatchEvent(new Event("input"));
      const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
      input.dispatchEvent(enter);
      expect(enter.defaultPrevented, text).toBe(false);
      expect(commands.submit(), text).toBe(false);
      expect(document.querySelector<HTMLElement>("#supervisor-command-palette")!.hidden, text).toBe(true);
      expect(input.value).toBe(text);
    }
    expect(run).not.toHaveBeenCalled();
    commands.dispose();
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
  it("keeps output open and keyboard focus through receipt updates", () => {
    const parent = document.createElement("section"); document.body.replaceChildren(parent);
    const receipt: CommandReceipt = { client_ref: "cmd-1", command: "usage", phase: "observed", detail: "Usage shown", output: "Usage", interactive: false };
    updateCommandReceipts(parent, [receipt], () => {});
    const details = parent.querySelector("details")!; details.open = true;
    parent.querySelector("summary")!.focus();
    updateCommandReceipts(parent, [{ ...receipt, output: "Usage updated" }], () => {});
    expect(parent.querySelector("details")!.open).toBe(true);
    expect(document.activeElement).toBe(parent.querySelector("summary"));
  });

  it("settles Exit after disconnect, timeout or late status without claiming other commands completed", async () => {
    vi.useFakeTimers();
    try {
      for (const command of ["exit", "finish", "usage"] as const) {
        for (const ordering of ["ended", "reject-ended", "ended-reject", "ended-status"] as const) {
          const send = vi.fn().mockReturnValue(true);
          const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {} });
          await session.run(command, true);
          const ref = [...session.receipts.keys()][0];
          const disconnected = "The session is reconnecting, so the message was not sent. Retry once live.";
          if (ordering === "reject-ended") session.reject(ref, disconnected);
          session.ended("prompt_input_exit");
          if (ordering === "ended-reject") session.reject(ref, disconnected);
          if (ordering === "ended-status") session.receive({ client_ref: ref, command, phase: "submitted", detail: "Awaiting harness", output: "", interactive: false });
          const receipt = session.receipts.get(ref)!;
          expect(receipt.phase, `${command}/${ordering}`).toBe(command === "exit" ? "observed" : "unconfirmed");
          expect(receipt.detail).toContain("prompt_input_exit");
          expect(receipt.detail).not.toMatch(/not sent|Retry/);
          const sent = send.mock.calls.length;
          await vi.advanceTimersByTimeAsync(125_000);
          expect(send).toHaveBeenCalledTimes(sent);
          expect(send.mock.calls.filter(([message]) => "SupervisorCommand" in message)).toHaveLength(1);
        }
      }
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });

  it("does not attribute an unrelated later session end to a timed-out Exit", async () => {
    vi.useFakeTimers();
    try {
      const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send: () => true, changed: () => {} });
      await session.run("exit", true);
      const ref = [...session.receipts.keys()][0];
      await vi.advanceTimersByTimeAsync(96_000);
      const timedOut = { ...session.receipts.get(ref)! };
      expect(timedOut.phase).toBe("unconfirmed");
      session.ended("daemon_shutdown");
      expect(session.receipts.get(ref)).toEqual(timedOut);
      session.receive({ ...timedOut, detail: "Late status; execution not confirmed." });
      expect(session.receipts.get(ref)?.phase).toBe("unconfirmed");
      expect(session.receipts.get(ref)?.detail).not.toContain("Session ended after Exit");
      session.reject(ref, "Still unconfirmed after reconnect.");
      expect(session.receipts.get(ref)?.phase).toBe("unconfirmed");
      expect(session.receipts.get(ref)?.detail).toBe("Still unconfirmed after reconnect.");
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });

  it("does not borrow session-end evidence for a refused, unsent or foreign command", async () => {
    vi.useFakeTimers();
    try {
      const send = vi.fn().mockReturnValue(true);
      const options = { unavailable: () => undefined, control: async () => {}, send, changed: () => {} };
      const ended = new SupervisorCommandSession(options);
      const other = new SupervisorCommandSession(options);
      await ended.run("exit", true);
      await other.run("exit", true);
      const ref = [...ended.receipts.keys()][0];
      const refused: CommandReceipt = { client_ref: ref, command: "exit", phase: "refused", detail: "Not sent: control was refused. Retry after taking control.", output: "", interactive: false };
      ended.receive({ ...refused, command: "usage" });
      expect(ended.receipts.get(ref)?.phase).toBe("submitted");
      ended.receive(refused);
      ended.ended("prompt_input_exit");
      ended.receive({ ...refused, phase: "submitted", detail: "Late pending status" });
      expect(ended.receipts.get(ref)).toEqual(refused);
      expect([...other.receipts.values()][0].phase).toBe("submitted");
      const unsent = new SupervisorCommandSession({ ...options, send: () => false });
      await expect(unsent.run("exit", true)).rejects.toThrow("not sent");
      unsent.ended("prompt_input_exit");
      expect(unsent.receipts.size).toBe(0);
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });

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
