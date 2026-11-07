// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { COMMAND_ERROR_PREFIX, SlashCommands, SupervisorCommandSession, commandsForHarness, commandReceiptNode, updateCommandReceipts, type CommandReceipt } from "./supervisor-commands";

describe("supervisor slash commands", () => {
  it("routes /mcp to Reload Cassy only for Claude and names unsupported harnesses", async () => {
    for (const harness of ["claude", "codex", "grok", "other", undefined]) {
      document.body.innerHTML = '<section><textarea></textarea></section>';
      const input = document.querySelector("textarea")!;
      const run = vi.fn().mockResolvedValue(undefined);
      const commands = new SlashCommands(input, { harness: () => harness, unavailable: () => undefined, label: () => "test supervisor", run });
      input.value = " /mcp \n";
      expect(commands.submit()).toBe(true);
      if (harness === "claude") {
        await Promise.resolve();
        expect(run).toHaveBeenCalledWith("mcp", false);
      } else {
        expect(run).not.toHaveBeenCalled();
        if (harness) expect(document.querySelector('[role="status"]')?.textContent).toContain(harness);
        else expect(document.querySelector('[role="status"]')?.textContent).toBeTruthy();
      }
      commands.dispose();
    }
  });
  it("filters command suggestions but submits only an exact supported command", async () => {
    document.body.innerHTML = '<section><textarea aria-label="Message"></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn().mockResolvedValue(undefined);
    const commands = new SlashCommands(input, { harness: () => "codex", unavailable: () => undefined, label: () => "test supervisor", run });
    input.value = "/us"; input.dispatchEvent(new Event("input"));
    expect(document.querySelectorAll("[data-supervisor-command]")).toHaveLength(1);
    expect(document.querySelector("[data-supervisor-command]")?.textContent).toBeTruthy();
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

  it("runs the confirmation by keyboard: Cancel first, arrows rove, Tab stays inside, Esc returns to the composer (cas-c009)", () => {
    document.body.innerHTML = '<section><textarea></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn().mockResolvedValue(undefined);
    const commands = new SlashCommands(input, { harness: () => "claude", unavailable: () => undefined, label: () => "test supervisor", run });
    const key = (key: string, shiftKey = false) => {
      const event = new KeyboardEvent("keydown", { key, shiftKey, bubbles: true, cancelable: true });
      document.activeElement!.dispatchEvent(event);
      return event;
    };
    input.value = "/finish"; input.focus();
    commands.submit();
    const dialog = document.querySelector<HTMLElement>('[role="alertdialog"]')!;
    expect(document.getElementById(dialog.getAttribute("aria-labelledby")!)?.textContent).toBe("test supervisor");
    const [cancel, finish] = [...dialog.querySelectorAll("button")];
    expect([cancel.hasAttribute("data-command-confirm"), finish.getAttribute("data-command-confirm")]).toEqual([false, "finish"]);
    expect(document.activeElement).toBe(cancel);
    for (const [press, shift, focused] of [["ArrowRight", false, finish], ["ArrowDown", false, cancel], ["ArrowLeft", false, finish], ["ArrowUp", false, cancel], ["Tab", true, finish], ["Tab", false, cancel]] as const) {
      expect(key(press, shift).defaultPrevented, press).toBe(true);
      expect(document.activeElement, `${press}${shift ? "+Shift" : ""}`).toBe(focused);
    }
    key("Escape");
    expect(document.querySelector<HTMLElement>("#supervisor-command-palette")!.hidden).toBe(true);
    expect(document.activeElement).toBe(input);
    expect(run).not.toHaveBeenCalled();
    commands.dispose();
  });

  it("keeps submission separate from execution and allows only Close for uncertain Usage", async () => {
    const row = commandReceiptNode({ client_ref: "cmd-1", command: "compact", phase: "submitted", detail: "Command submitted; harness execution is not confirmed yet.", output: "<unsafe>", interactive: false });
    expect(row.dataset.phase).toBe("submitted");
    expect(row.querySelector("p")?.textContent).toBe("Command submitted; harness execution is not confirmed yet.");
    expect(row.querySelector("unsafe")).toBeNull();
    expect(row.querySelector("pre")?.textContent).toBe("<unsafe>");
    const send = vi.fn().mockReturnValue(true);
    const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {} });
    await session.run("usage", false);
    const ref = [...session.receipts.keys()][0];
    const uncertain: CommandReceipt = { client_ref: ref, command: "usage", phase: "unconfirmed", detail: "Outcome unknown", output: "", interactive: false };
    session.receive(uncertain);
    const pending: Promise<void>[] = [];
    const recovery = commandReceiptNode(uncertain, key => { pending.push(session.step(ref, key)); });
    const close = recovery.querySelector("button");
    expect(close?.textContent).toBeTruthy();
    expect(recovery.querySelectorAll("button")).toHaveLength(1);
    close!.click();
    await Promise.all(pending);
    expect(send).toHaveBeenLastCalledWith({ SupervisorCommandStep: { client_ref: ref, key: "cancel" } });
    session.receive({ ...uncertain, phase: "observed" });
    expect(commandReceiptNode({ ...uncertain, command: "compact" }, () => {}).querySelector("button")).toBeNull();
  });
});

describe("supervisor command receipts", () => {
  it("dismisses status without cancelling queued reloads or admitting a second command", async () => {
    vi.useFakeTimers();
    try {
      const commands = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send: () => true, changed: () => {} });
      await commands.run("mcp", false);
      const ref = [...commands.receipts.keys()][0]!;
      commands.receive({ client_ref: ref, command: "mcp", phase: "queued", detail: "Wait for the current turn", output: "", interactive: false });
      commands.dismiss(ref);
      expect(commands.visibleReceipts).toEqual([]);
      await expect(commands.run("mcp", false)).rejects.toThrow(Error);
      commands.receive({ client_ref: ref, command: "mcp", phase: "observed", detail: "Replacement process observed", output: "", interactive: false });
      expect(commands.receipts.get(ref)?.phase).toBe("observed");
      expect(commands.visibleReceipts).toEqual([]);
      await commands.run("mcp", false);
      expect(commands.visibleReceipts).toHaveLength(1);
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });
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

  it("settles a recent Exit after disconnect or late status without claiming other commands completed", async () => {
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
          if (ordering === "ended-status") {
            await vi.advanceTimersByTimeAsync(125_000);
            session.receive({ client_ref: ref, command, phase: "submitted", detail: "Awaiting harness", output: "", interactive: false });
          }
          const receipt = session.receipts.get(ref)!;
          expect(receipt.phase, `${command}/${ordering}`).toBe(command === "exit" ? "observed" : "unconfirmed");
          expect(receipt.detail).toContain("prompt_input_exit");
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
      const detail = "Still unconfirmed after reconnect.";
      session.reject(ref, detail);
      expect(session.receipts.get(ref)?.phase).toBe("unconfirmed");
      expect(session.receipts.get(ref)?.detail).toBe(detail);
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
      await expect(unsent.run("exit", true)).rejects.toThrow(Error);
      unsent.ended("prompt_input_exit");
      expect(unsent.receipts.size).toBe(0);
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });

  it("keeps one row for a command's refusals, clears it once accepted, and never hides an uncertain outcome", async () => {
    vi.useFakeTimers();
    try {
      const commands = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send: () => true, changed: () => {} });
      const busy = `${COMMAND_ERROR_PREFIX} The supervisor is busy. Wait until its turn ends.`;
      const newest = () => [...commands.receipts.keys()].at(-1)!;
      await commands.run("usage", false); commands.reject(newest(), busy);
      await commands.run("usage", false); commands.reject(newest(), busy);
      await commands.run("model", false); commands.reject(newest(), busy);
      expect([...commands.receipts.values()].map(({ command, phase }) => [command, phase])).toEqual([["usage", "refused"], ["model", "refused"]]);
      const reason = busy.slice(COMMAND_ERROR_PREFIX.length).trim();
      for (const receipt of commands.receipts.values()) expect(receipt.detail).toContain(reason);
      expect([...commands.receipts.values()][0].detail).toMatch(/\b2\b/);

      await commands.run("usage", false);
      const accepted = newest();
      commands.receive({ client_ref: accepted, command: "usage", phase: "submitted", detail: "Command submitted.", output: "", interactive: false });
      expect([...commands.receipts.values()].map(({ command, phase }) => [command, phase])).toEqual([["model", "refused"], ["usage", "submitted"]]);

      // After a receipt the command may have run, so the same error stays uncertain.
      commands.reject(accepted, `${COMMAND_ERROR_PREFIX} The original command receipt is unavailable.`);
      expect(commands.receipts.get(accepted)?.phase).toBe("unconfirmed");
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });

  it("polls only the outcome and stops after observation, without replaying a mutation", async () => {
    vi.useFakeTimers();
    try {
      const send = vi.fn().mockReturnValue(true);
      const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {} });
      await session.run("finish", true);
      const ref = [...session.receipts.keys()][0];
      session.receive({ client_ref: ref, command: "finish", phase: "queued", detail: "Waiting for the native turn to end", output: "", interactive: false });
      await vi.advanceTimersByTimeAsync(120_000);
      expect(session.receipts.get(ref)?.phase).toBe("queued");
      expect(send.mock.calls.filter(([message]) => "SupervisorCommand" in message)).toHaveLength(1);
      session.receive({ client_ref: ref, command: "finish", phase: "submitted", detail: "Command submitted", output: "", interactive: false });
      send.mockClear();
      await vi.advanceTimersByTimeAsync(2_000);
      expect(send.mock.calls.filter(([message]) => "SupervisorCommand" in message)).toHaveLength(0);
      expect(send.mock.calls.filter(([message]) => "SupervisorCommandStatus" in message)).toHaveLength(2);
      session.receive({ client_ref: ref, command: "finish", phase: "observed", detail: "Harness completed Finish", output: "", interactive: false });
      await vi.advanceTimersByTimeAsync(5_000);
      expect(send).toHaveBeenCalledTimes(2);
    } finally { vi.useRealTimers(); }
  });
});
