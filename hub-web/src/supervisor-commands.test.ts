// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { COMMAND_ERROR_PREFIX, SlashCommands, SupervisorCommandSession, commandsForHarness, commandReceiptNode, updateCommandReceipts, type CommandReceipt } from "./supervisor-commands";
import { sendsOnEnter } from "./supervisor-message";

function commandComposer(harness: string | undefined, send: (message: unknown) => void, options: {
  unavailable?: () => string | undefined;
  control?: () => Promise<void>;
  birth?: () => string;
} = {}) {
  let input: HTMLTextAreaElement;
  let palette: SlashCommands | undefined;
  let storedDraft = "";
  const unavailable = options.unavailable ?? (() => undefined);
  const session = new SupervisorCommandSession({
    unavailable,
    control: options.control ?? (async () => {}),
    birth: options.birth,
    send: message => {
      send(message);
      return true;
    },
    changed: () => palette?.reconcile(),
    clearDraft: original => {
      if (storedDraft === original) storedDraft = "";
      if (input.value === original) input.value = "";
    },
    completed: origin => palette?.returnFocus(origin),
  });
  return {
    session,
    get draft() { return storedDraft; },
    mount(next: HTMLTextAreaElement) {
      input = next;
      input.addEventListener("input", () => { storedDraft = input.value; });
      palette = new SlashCommands(input, { harness: () => harness, unavailable, label: () => "test supervisor", session });
      return palette;
    },
  };
}

function holdControl() {
  let resolve!: () => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<void>((release, refuse) => { resolve = release; reject = refuse; });
  return { promise, resolve, reject };
}

describe("supervisor slash commands", () => {
  it("offers Standby with one confirmation on every supported harness", () => {
    for (const harness of ["claude", "codex"]) {
      const standby = commandsForHarness(harness).find(command => command.id === "standby");
      expect(standby?.confirm).toBeTruthy();
    }
    expect(commandsForHarness("grok")).toEqual([]);
  });
  it("routes /mcp to Reload Cassy for Claude, confirms Codex's supervisor restart, and names unsupported harnesses", async () => {
    for (const [harness, typed] of [["claude", " /mcp \n"], ["claude", "/reload-cassy"], ["codex", "/mcp"], ["codex", " /reload-cassy\n"], ["grok", "/mcp"], ["other", "/mcp"], [undefined, "/mcp"]] as const) {
      document.body.innerHTML = '<section><textarea></textarea></section>';
      const input = document.querySelector("textarea")!;
      const run = vi.fn();
      const commands = commandComposer(harness, run).mount(input);
      input.value = typed;
      const typeAndSubmit = () => {
        input.dispatchEvent(new Event("input", { bubbles: true }));
        const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
        input.dispatchEvent(enter);
        expect(enter.defaultPrevented).toBe(true);
      };
      typeAndSubmit();
      if (harness === "claude") {
        await Promise.resolve();
        expect(run).toHaveBeenCalledWith({ SupervisorCommand: { client_ref: expect.any(String), command: "mcp", confirmed: false } });
        await vi.waitFor(() => expect(input.value).toBe(""));
      } else if (harness === "codex") {
        const panel = document.querySelector<HTMLElement>("#supervisor-command-palette")!;
        const reload = () => panel.querySelector<HTMLButtonElement>('[data-command-confirm="mcp"]');
        const expectConfirmation = () => {
          expect(panel.querySelector('[role="alertdialog"]')?.closest("[hidden]")).toBeNull();
          expect(reload()?.closest("[hidden]")).toBeNull();
          expect(reload()?.disabled).toBe(false);
        };
        expectConfirmation();
        input.dispatchEvent(new Event("input", { bubbles: true }));
        expectConfirmation();
        commands.sync();
        expectConfirmation();
        panel.querySelector<HTMLButtonElement>('[role="alertdialog"] button.secondary')!.click();
        expect(panel.hidden).toBe(true);
        expect(run).not.toHaveBeenCalled();
        typeAndSubmit();
        reload()!.click();
        await Promise.resolve();
        expect(run).toHaveBeenCalledTimes(1);
        expect(run).toHaveBeenCalledWith({ SupervisorCommand: { client_ref: expect.any(String), command: "mcp", confirmed: true } });
        await vi.waitFor(() => expect(input.value).toBe(""));
      } else {
        expect(run).not.toHaveBeenCalled();
        if (harness) expect(document.querySelector('[role="status"]')?.textContent).toContain(harness);
        else expect(document.querySelector('[role="status"]')?.textContent).toBeTruthy();
      }
      commands.dispose();
    }
    // An alias needs its slash: "\\reload-cassy" is ordinary chat text.
    document.body.innerHTML = '<section><textarea></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn();
    const commands = commandComposer("claude", run).mount(input);
    input.value = "\\reload-cassy";
    expect(commands.submit()).toBe(false);
    await Promise.resolve();
    expect(run).not.toHaveBeenCalled();
    commands.dispose();
  });
  it("filters command suggestions but submits only an exact supported command", async () => {
    document.body.innerHTML = '<section><textarea aria-label="Message"></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn();
    const { mount, session } = commandComposer("codex", run);
    const commands = mount(input);
    input.value = "/us"; input.dispatchEvent(new Event("input"));
    expect(document.querySelectorAll("[data-supervisor-command]")).toHaveLength(1);
    expect(document.querySelector("[data-supervisor-command]")?.textContent).toBeTruthy();
    const partial = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    input.dispatchEvent(partial);
    expect(partial.defaultPrevented).toBe(false);
    expect(run).not.toHaveBeenCalled();
    document.querySelector<HTMLButtonElement>("[data-supervisor-command]")!.click();
    await Promise.resolve();
    expect(run).toHaveBeenCalledWith({ SupervisorCommand: { client_ref: expect.any(String), command: "usage", confirmed: false } });
    await vi.waitFor(() => {
      expect(input.value).toBe("");
      expect(document.querySelector<HTMLElement>("#supervisor-command-palette")!.hidden).toBe(true);
    });
    const { client_ref } = run.mock.calls[0][0].SupervisorCommand;
    const receipt = {
      device_id: "journey-device", client_ref, command: "usage", phase: "observed", detail: "Usage displayed by the supervisor's harness.",
      output: "• Model: gpt-6.1-sol (reasoning high)\n• Context window: 72% left\n› Ask Codex to do anything",
      interactive: false,
    } satisfies CommandReceipt & { device_id: string };
    session.receive(receipt);
    run.mockClear();
    input.value = "/usage"; input.dispatchEvent(new Event("input"));
    const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    input.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(true);
    await Promise.resolve();
    expect(run).toHaveBeenCalledWith({ SupervisorCommand: { client_ref: expect.any(String), command: "usage", confirmed: false } });
    expect(commandsForHarness("grok")).toEqual([]);
    commands.dispose();
  });

  it("sends an ordinary chat message with a command confirmation open", () => {
    document.body.innerHTML = '<section><textarea></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn();
    const sendChat = vi.fn();
    const commands = commandComposer("codex", run).mount(input);
    input.onkeydown = event => {
      if (event.defaultPrevented || !sendsOnEnter(event)) return;
      event.preventDefault();
      if (!commands.submit()) sendChat(input.value);
    };
    const pressEnter = (text: string) => {
      input.focus();
      input.value = text;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    };
    try {
      pressEnter("/reload-cassy");
      expect(document.querySelector('[data-command-confirm="mcp"]')).not.toBeNull();
      pressEnter("Continue with the review.");
      expect(sendChat.mock.calls).toEqual([["Continue with the review."]]);
      expect(run).not.toHaveBeenCalled();
    } finally {
      commands.dispose();
    }
  });

  it("lets pasted paths and unsupported slash text reach the normal composer send", () => {
    document.body.innerHTML = '<section><textarea aria-label="Message"></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn();
    const commands = commandComposer("codex", run, { unavailable: () => "No command control" }).mount(input);
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
    const run = vi.fn();
    const commands = commandComposer("claude", run).mount(input);
    input.value = "/clear";
    commands.submit();
    expect(run).not.toHaveBeenCalled();
    const confirm = document.querySelector<HTMLButtonElement>("[data-command-confirm]")!;
    expect(document.querySelector('[role="alertdialog"]')?.textContent).toContain("test supervisor");
    confirm.click(); await Promise.resolve();
    expect(run).toHaveBeenCalledWith({ SupervisorCommand: { client_ref: expect.any(String), command: "clear", confirmed: true } });
    await vi.waitFor(() => expect(input.value).toBe(""));
    commands.dispose();
  });

  it("runs the confirmation by keyboard: Cancel first, arrows rove, Tab stays inside, Esc returns to the composer (cas-c009)", () => {
    document.body.innerHTML = '<section><textarea></textarea></section>';
    const input = document.querySelector("textarea")!;
    const run = vi.fn();
    const commands = commandComposer("claude", run).mount(input);
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

  it.each(["confirming", "sending", "refused", "outside press", "Commands toggle", "Escape", "pre-confirm Cancel"])("reconciles reload through %s", async scenario => {
    const lease = holdControl();
    const send = vi.fn();
    const controller = commandComposer("codex", send, { control: () => lease.promise });
    const fromHeader = !["confirming", "sending", "refused"].includes(scenario);
    const textarea = () => {
      document.body.innerHTML = '<button id="conversation-commands"></button><section><textarea></textarea></section>';
      const input = document.querySelector("textarea")!;
      input.value = "/reload-cassy";
      return input;
    };
    const panel = () => document.querySelector<HTMLElement>("#supervisor-command-palette")!;
    const anchor = () => document.querySelector<HTMLButtonElement>("#conversation-commands")!;
    const confirm = () => document.querySelector<HTMLButtonElement>('[data-command-confirm="mcp"]')!;
    const first = textarea();
    const original = controller.mount(first);
    first.dispatchEvent(new Event("input"));
    if (fromHeader) {
      original.show(anchor());
      panel().querySelector<HTMLButtonElement>('[data-supervisor-command="mcp"]')!.click();
    } else original.submit();
    confirm().focus();
    if (scenario === "pre-confirm Cancel") {
      panel().querySelector<HTMLButtonElement>('[role="alertdialog"] button.secondary')!.click();
    } else if (scenario !== "confirming") confirm().click();
    original.dispose();
    const input = textarea();
    const replacement = controller.mount(input);
    try {
      if (scenario === "confirming") {
        expect(document.activeElement).toBe(confirm());
        expect(confirm().disabled).toBe(false);
        return;
      }
      if (scenario !== "pre-confirm Cancel") {
        expect(confirm().disabled).toBe(true);
        expect(replacement.submit()).toBe(true);
      }
      if (scenario === "refused") {
        const refusal = new Error("Studio iPad");
        lease.reject(refusal);
        await vi.waitFor(() => expect(document.querySelector('[role="status"]')?.textContent).toBe(refusal.message));
        anchor().onclick = () => replacement.show(anchor());
        anchor().click();
        expect(anchor().getAttribute("aria-expanded")).toBe("true");
        expect(input.value).toBe("/reload-cassy");
        expect(controller.draft).toBe("/reload-cassy");
        expect(send).not.toHaveBeenCalled();
        return;
      }
      if (scenario === "outside press") document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      if (scenario === "Commands toggle") replacement.show(anchor());
      if (scenario === "Escape") panel().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      lease.resolve();
      await lease.promise;
      if (scenario === "pre-confirm Cancel") expect(send).not.toHaveBeenCalled();
      else expect(send.mock.calls).toEqual([[{ SupervisorCommand: { client_ref: expect.any(String), command: "mcp", confirmed: true } }]]);
      if (scenario === "sending") {
        await vi.waitFor(() => expect(input.value).toBe(""));
        expect(controller.draft).toBe("");
        expect(document.activeElement).toBe(input);
      }
    } finally {
      replacement.dispose();
    }
  });

  it("dismisses the header confirmation by re-click or outside press and removes the body panel on dispose", () => {
    document.body.innerHTML = '<button id="conversation-commands"></button><section><textarea></textarea></section>';
    const anchor = document.querySelector<HTMLButtonElement>("#conversation-commands")!;
    const send = vi.fn();
    const { mount, session } = commandComposer("codex", send);
    const palette = mount(document.querySelector("textarea")!);
    const confirmFromHeader = () => {
      palette.show(anchor);
      document.querySelector<HTMLButtonElement>('[data-supervisor-command="mcp"]')!.click();
      expect(document.querySelector("body > #supervisor-command-palette [role=alertdialog]")).not.toBeNull();
    };
    confirmFromHeader();
    palette.show(anchor);
    expect(session.intent).toBeUndefined();
    confirmFromHeader();
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(session.intent).toBeUndefined();
    confirmFromHeader();
    palette.dispose();
    expect(document.querySelector("#supervisor-command-palette")).toBeNull();
    expect(send).not.toHaveBeenCalled();
  });

  it("drops a sending intent on a new session birth before any frame is sent", async () => {
    const lease = holdControl();
    const send = vi.fn();
    let birth = "first";
    document.body.innerHTML = '<section><textarea></textarea></section>';
    const controller = commandComposer("claude", send, { control: () => lease.promise, birth: () => birth });
    const input = document.querySelector("textarea")!;
    const palette = controller.mount(input);
    input.value = "/mcp";
    palette.submit();
    birth = "replacement";
    palette.reconcile();
    expect(controller.session.intent).toBeUndefined();
    lease.resolve();
    await lease.promise;
    expect(send).not.toHaveBeenCalled();
    expect(input.value).toBe("/mcp");
    palette.dispose();
  });

  it("keeps submission separate from execution and allows only Close for uncertain Usage", async () => {
    const detail = "Command submitted; harness execution is not confirmed yet.";
    const row = commandReceiptNode({ client_ref: "cmd-1", command: "compact", phase: "submitted", detail, output: "<unsafe>", interactive: false });
    expect(row.dataset.phase).toBe("submitted");
    expect(row.querySelector("p")?.textContent).toBe(detail);
    expect(row.querySelector("unsafe")).toBeNull();
    expect(row.querySelector("pre")?.textContent).toBe("<unsafe>");
    // cas-8c4f: Reload Cassy animates only while it waits for a quiet moment or its proof.
    for (const [phase, waiting] of [["queued", true], ["submitted", true], ["observed", false], ["unconfirmed", false]] as const) {
      expect(Boolean(commandReceiptNode({ client_ref: "r", command: "mcp", phase, detail: "", output: "", interactive: false }).querySelector(".reload-wait"))).toBe(waiting);
    }
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
  it("settles a finished /mcp into one toast with no card, while a failure keeps its card until Dismiss (cas-95ea)", async () => {
    vi.useFakeTimers();
    try {
      const settled = vi.fn();
      const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send: () => true, changed: () => {}, settled });
      await session.run("mcp", false);
      const ok = [...session.receipts.keys()][0]!;
      const observed: CommandReceipt = { client_ref: ok, command: "mcp", phase: "observed", detail: "Cassy reloaded: PID 2202, version 9.99.2 (previous PID 1101).", output: "", interactive: false };
      session.receive(observed); session.receive(observed);
      expect(settled).toHaveBeenCalledExactlyOnceWith(observed);
      expect(session.visibleReceipts).toEqual([]);

      await session.run("mcp", false);
      const failed = [...session.receipts.keys()][1]!;
      session.reject(failed, `${COMMAND_ERROR_PREFIX} The supervisor is busy.`);
      expect(session.visibleReceipts.map(receipt => [receipt.client_ref, receipt.phase])).toEqual([[failed, "refused"]]);
      await vi.advanceTimersByTimeAsync(10_000);
      expect(session.visibleReceipts).toHaveLength(1);
      await session.dismiss(failed);
      expect(session.visibleReceipts).toEqual([]);
      expect(settled).toHaveBeenCalledTimes(1);
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });
  it("cancels queued or interactive receipts on Dismiss and waits for the daemon before allowing Exit (cas-f1a2)", async () => {
    vi.useFakeTimers();
    try {
      for (const interactive of [false, true]) {
        const send = vi.fn().mockReturnValue(true);
        const commands = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {} });
        await commands.run(interactive ? "model" : "finish", true);
        const ref = [...commands.receipts.keys()][0]!;
        const receipt: CommandReceipt = { client_ref: ref, command: interactive ? "model" : "finish", phase: interactive ? "observed" : "queued", detail: "Pending", output: "", interactive };
        commands.receive(receipt);
        const row = commandReceiptNode(receipt, () => {}, () => {});
        expect(row.querySelectorAll("button")).toHaveLength(interactive ? 5 : 1);
        send.mockReturnValueOnce(false);
        await expect(Promise.resolve().then(() => commands.dismiss(ref))).rejects.toThrow(Error);
        expect(commands.visibleReceipts).toHaveLength(1);
        await commands.dismiss(ref);
        expect(send).toHaveBeenLastCalledWith({ SupervisorCommandCancel: { client_ref: ref } });
        expect(commands.visibleReceipts).toHaveLength(1);
        await expect(commands.run("exit", true)).rejects.toThrow(Error);
        await vi.advanceTimersByTimeAsync(1_000);
        expect(send).toHaveBeenLastCalledWith({ SupervisorCommandStatus: { client_ref: ref } });
        commands.receive({ ...receipt, phase: "cancelled", interactive: false });
        expect(commands.visibleReceipts).toEqual([]);
        await commands.run("exit", true);
        expect(send).toHaveBeenLastCalledWith({ SupervisorCommand: { client_ref: expect.any(String), command: "exit", confirmed: true } });
        const exit = [...commands.receipts.keys()].at(-1)!;
        send.mockClear();
        await commands.dismiss(exit);
        expect(send).not.toHaveBeenCalled();
        await expect(commands.run("finish", true)).rejects.toThrow(Error);
      }
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });
  it("resumes outcome polling after a refused or timed-out Cancel (cas-5b23)", async () => {
    vi.useFakeTimers();
    try {
      for (const cancellation of ["refused", "timed-out"] as const) {
        const send = vi.fn().mockReturnValue(true);
        const commands = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {} });
        await commands.run("finish", true);
        const ref = [...commands.receipts.keys()][0]!;
        const receipt: CommandReceipt = { client_ref: ref, command: "finish", phase: "queued", detail: "Pending", output: "", interactive: false };
        commands.receive(receipt);
        await commands.dismiss(ref);
        expect(send).toHaveBeenLastCalledWith({ SupervisorCommandCancel: { client_ref: ref } });
        // The daemon advanced Finish before Cancel reached it, but its
        // submitted receipt has not reached the client yet.
        if (cancellation === "refused") commands.reject(ref, `${COMMAND_ERROR_PREFIX} already submitted`);
        else await vi.advanceTimersByTimeAsync(96_000);
        await expect(commands.run("exit", true)).rejects.toThrow(Error);
        send.mockClear();
        await vi.advanceTimersByTimeAsync(1_000);
        expect(send).toHaveBeenCalledExactlyOnceWith({ SupervisorCommandStatus: { client_ref: ref } });
        commands.receive({ ...receipt, phase: "submitted" });
        await vi.advanceTimersByTimeAsync(1_000);
        expect(send).toHaveBeenCalledTimes(2);
        commands.receive({ ...receipt, phase: "observed" });
        await vi.advanceTimersByTimeAsync(1_000);
        expect(send).toHaveBeenCalledTimes(2);
        await commands.run("exit", true);
        expect(send).toHaveBeenLastCalledWith({ SupervisorCommand: { client_ref: expect.any(String), command: "exit", confirmed: true } });
        vi.clearAllTimers();
      }
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

  it("keeps a held Reload Cassy's own status through a reconnecting status read (cas-6b21)", async () => {
    vi.useFakeTimers();
    try {
      const send = vi.fn().mockReturnValue(true);
      const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {} });
      await session.run("mcp", true);
      const ref = [...session.receipts.keys()][0];
      const reconnecting = "The session's daemon connection is reconnecting, so the message was not sent. Retry once the session is live again.";
      for (const phase of ["queued", "submitted"] as const) {
        const held = { client_ref: ref, command: "mcp" as const, phase, detail: `daemon ${phase}`, output: "", interactive: false };
        session.receive(held);
        expect(session.reject(ref, reconnecting, true)).toBe(true);
        expect(session.receipts.get(ref)).toEqual(held);
      }
      const reads = send.mock.calls.length;
      await vi.advanceTimersByTimeAsync(1_000);
      expect(send.mock.calls.length).toBeGreaterThan(reads);
      session.receive({ client_ref: ref, command: "mcp", phase: "observed", detail: "Cassy reloaded", output: "", interactive: false });
      expect(session.receipts.get(ref)?.phase).toBe("observed");
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });

  it("settles a recent Exit after disconnect or late status without claiming other commands completed", async () => {
    vi.useFakeTimers();
    try {
      for (const command of ["exit", "finish", "usage"] as const) {
        for (const phase of ["queued", "submitted"] as const) {
        for (const ordering of ["ended", "reject-ended", "ended-reject", "ended-status", "ended-observed"] as const) {
          const send = vi.fn().mockReturnValue(true);
          const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {} });
          await session.run(command, true);
          const ref = [...session.receipts.keys()][0];
          session.receive({ client_ref: ref, command, phase, detail: "Pending", output: "", interactive: false });
          const disconnected = "The session is reconnecting, so the message was not sent. Retry once live.";
          if (ordering === "reject-ended") session.reject(ref, disconnected);
          session.ended("prompt_input_exit");
          if (ordering === "ended-reject") session.reject(ref, disconnected);
          if (ordering === "ended-status") {
            await vi.advanceTimersByTimeAsync(125_000);
            session.receive({ client_ref: ref, command, phase: "submitted", detail: "Awaiting harness", output: "", interactive: false });
          }
          if (ordering === "ended-observed") session.receive({ client_ref: ref, command, phase: "observed", detail: "Native outcome", output: "", interactive: false });
          const receipt = session.receipts.get(ref)!;
          const submitted = phase === "submitted" || ordering === "ended-status";
          expect(receipt.phase, `${command}/${phase}/${ordering}`).toBe(ordering === "ended-observed" || command === "exit" && submitted ? "observed" : "unconfirmed");
          const sent = send.mock.calls.length;
          await vi.advanceTimersByTimeAsync(125_000);
          expect(send).toHaveBeenCalledTimes(sent);
          expect(send.mock.calls.filter(([message]) => "SupervisorCommand" in message)).toHaveLength(1);
        }
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
      ended.receive({ ...refused, phase: "queued" });
      ended.ended("prompt_input_exit");
      expect(ended.receipts.get(ref)?.phase).toBe("unconfirmed");
      // Only a per-command daemon refusal proves this queued command was
      // abandoned without typing; generic session-end evidence cannot.
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

  it("reads a stopped session's retained outcome without resending, and stays Unconfirmed without one (cas-b521)", async () => {
    vi.useFakeTimers();
    try {
      for (const retained of ["refused", "missing"] as const) {
        const send = vi.fn().mockReturnValue(true);
        let answers: (CommandReceipt | "retry" | undefined)[] = [];
        const readRetained = vi.fn(async () => answers.shift());
        const session = new SupervisorCommandSession({ unavailable: () => undefined, control: async () => {}, send, changed: () => {}, readRetained });
        await session.run("compact", false);
        const ref = [...session.receipts.keys()][0];
        const queued: CommandReceipt = { client_ref: ref, command: "compact", phase: "queued", detail: "Queued", output: "", interactive: false };
        session.receive(queued);
        // The daemon may still be tearing down: the hub answers 409 first.
        answers = ["retry", retained === "refused" ? { ...queued, phase: "refused", detail: "Command not sent." } : undefined];
        session.ended("daemon_stopped");
        session.ended("daemon_stopped");
        await vi.advanceTimersByTimeAsync(10_000);
        expect(session.receipts.get(ref)?.phase, retained).toBe(retained === "refused" ? "refused" : "unconfirmed");
        expect(readRetained, retained).toHaveBeenCalledTimes(2);
        expect(send.mock.calls.filter(([message]) => "SupervisorCommand" in message)).toHaveLength(1);
      }
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
      await vi.advanceTimersByTimeAsync(97_000);
      expect(session.receipts.get(ref)?.phase).toBe("unconfirmed");
      session.receive({ ...session.receipts.get(ref)!, phase: "unconfirmed" });
      const reads = send.mock.calls.filter(([message]) => "SupervisorCommandStatus" in message).length;
      await vi.advanceTimersByTimeAsync(8_000);
      expect(send.mock.calls.filter(([message]) => "SupervisorCommand" in message)).toHaveLength(0);
      expect(send.mock.calls.filter(([message]) => "SupervisorCommandStatus" in message).length).toBeGreaterThan(reads);
      const sent = send.mock.calls.length;
      session.receive({ client_ref: ref, command: "finish", phase: "observed", detail: "Harness completed Finish", output: "", interactive: false });
      expect(session.visibleReceipts).toEqual([]);
      await vi.advanceTimersByTimeAsync(5_000);
      expect(send).toHaveBeenCalledTimes(sent);
      await session.run("finish", true);
      await vi.advanceTimersByTimeAsync(10 * 60_000 + 5_000);
      const expired = send.mock.calls.length;
      await vi.advanceTimersByTimeAsync(10_000);
      expect(send).toHaveBeenCalledTimes(expired);
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });
});
