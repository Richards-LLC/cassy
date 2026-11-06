import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { SCOPES } from "./hub-double";
import { journeyStamp } from "./clock";

for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
  test.describe(`${viewport.width}`, () => {
    test.use({ viewport });
    test("HUB-J27 completed Claude turn accepts commands once and shows a compact native command receipt (cas-99e7)", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "claude" as const })) };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN],
        fleet: { [PELICAN]: { agents: [], tasks: [], epics: [], focused_epic: null, spawnNames: [],
          supervisor_phase: { event_id: "completed-turn", kind: "notice", lifecycle: { id: "prior-turn", stage: "end", outcome: "success" }, observed_at: journeyStamp(), stale: false } } },
        history: { [PELICAN]: [{ has_earlier: false, replies: [], messages: [{ notification_id: 9907, target: "terminal-history", device_id: "terminal", operator_label: "Terminal", stamped: false, state: "acknowledged", at: journeyStamp(-1000),
          text: "<command-name>/mcp</command-name><command-message>mcp</command-message><command-args>reconnect cas</command-args>" }] }] },
      });
      await journey.stage("Read the completed turn and a native command receipt without XML", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
        await expect(page.locator(".phase-line")).toContainText("Turn completed");
        const receipt = page.locator(".native-command-receipt");
        await expect(receipt).toContainText("/mcp reconnect cas");
        await expect(page.locator('.thread [role="log"]')).not.toContainText("<command-name>");
        await expect(receipt).not.toHaveClass(/bub/);
      });
      await journey.stage("Reload Cassy once from the idle Claude supervisor", async () => {
        const input = page.locator("#message-text");
        await input.fill("/mcp"); await input.press("Enter");
        const frames = hub.frames.filter(frame => frame.kind === "SupervisorCommand");
        expect(frames).toHaveLength(1);
        const request = frames[0]!.body as { client_ref: string; command: string };
        expect(request.command).toBe("mcp");
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { client_ref: request.client_ref, command: "mcp", phase: "observed", detail: "Cassy reloaded: PID 2202, version 9.99.2 (previous PID 1101).", output: "", interactive: false } } });
        await expect(page.locator('.supervisor-command-receipt[data-command="mcp"]')).toHaveCount(0);
        expect(hub.sends).toHaveLength(0);
      });
      await journey.stage("Exit once after the native turn has completed", async () => {
        const input = page.locator("#message-text");
        await input.fill("/exit"); await input.press("Enter");
        await page.getByRole("alertdialog").getByRole("button", { name: "Exit", exact: true }).click();
        const frames = hub.frames.filter(frame => frame.kind === "SupervisorCommand");
        expect(frames).toHaveLength(2);
        expect(frames[1]!.body).toMatchObject({ command: "exit", confirmed: true });
        expect(hub.sends).toHaveLength(0);
      });
    });
    test("HUB-J27 reload Claude's Cassy server and read its replacement process", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "claude" as const })) };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN] });
      await journey.stage("Reload Cassy from slash input without sending chat text", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
        const input = page.locator("#message-text");
        await input.fill("/mcp");
        await expect(page.locator('[data-supervisor-command="mcp"]')).toContainText("Reload Cassy");
        await input.press("Enter");
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommand").length).toBe(1);
        const request = hub.frames.find(frame => frame.kind === "SupervisorCommand")!.body as { client_ref: string; command: string };
        expect(request.command).toBe("mcp");
        expect(hub.sends).toHaveLength(0);
        const receipt = page.locator(".supervisor-command-receipt");
        await expect(receipt).toHaveAttribute("data-phase", "submitted");
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { client_ref: request.client_ref, command: "mcp", phase: "observed", detail: "Cassy reloaded: PID 2202, version 9.99.2 (previous PID 1101).", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "observed");
        await expect(receipt).toContainText("PID 2202, version 9.99.2");
        expect(hub.frames.filter(frame => frame.kind === "SupervisorCommand")).toHaveLength(1);
      });
    });
    test("HUB-J27 run a supervisor command and read its actual outcome", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "codex" as const })) };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN] });
      const input = page.locator("#message-text");
      const mutations = () => hub.frames.filter(frame => frame.kind === "SupervisorCommand");
      let ref = "";
      await journey.stage("Choose Usage from the slash palette with the keyboard", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
        await expect(page.getByRole("button", { name: "Supervisor commands", exact: true })).toBeVisible();
        await input.fill("/us");
        await expect(page.locator("[data-supervisor-command]")).toHaveCount(1);
        await input.press("ArrowDown");
        await expect(page.locator('[data-supervisor-command="usage"]')).toBeFocused();
        await page.keyboard.press("Enter");
        await expect.poll(() => mutations().length).toBe(1);
        const request = mutations()[0].body as { client_ref: string; command: string; confirmed: boolean };
        expect(request.command).toBe("usage"); ref = request.client_ref;
        await expect(page.locator(".supervisor-command-receipt")).toContainText("not confirmed");
        expect(hub.sends).toHaveLength(0);
        await expect(input).toBeFocused();
        expect(await page.evaluate(() => {
          const days: Record<string, Record<string, number>> = JSON.parse(localStorage.getItem("commander.usage.v1") ?? "{}").days ?? {};
          return Object.values(days).reduce((sum, day) => sum + (day["supervisor.command"] ?? 0), 0);
        })).toBe(1);
      });
      await journey.stage("Read the observed native result without resending the command", async () => {
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { client_ref: ref, command: "usage", phase: "observed", detail: "Usage displayed by the supervisor's harness.", output: "Model: GPT-6.1-Sol\nContext window: 93% left\n<untrusted>", interactive: false } } });
        const receipt = page.locator(".supervisor-command-receipt");
        await expect(receipt).toHaveAttribute("data-phase", "observed");
        await receipt.getByText("Harness output", { exact: true }).click();
        await expect(receipt.locator("pre")).toContainText("<untrusted>");
        await expect(receipt.locator("untrusted")).toHaveCount(0);
        expect(mutations()).toHaveLength(1);
      });
      await journey.stage("Send pasted paths and unknown slash text as messages (cas-62ea)", async () => {
        const messages = ["/home/pippenz/notes.md", "/mnt/notes.md review this", "/shell whoami", "//usage"];
        for (const [index, text] of messages.entries()) {
          await input.fill(text);
          await expect(page.locator("#supervisor-command-palette")).toBeHidden();
          if (index % 2 === 0) await input.press("Enter");
          else await page.locator("#message-send").click();
          await expect.poll(() => hub.sends.length).toBe(index + 1);
          expect(hub.sends[index].text).toBe(text);
          await expect(input).toHaveValue("");
        }
        expect(mutations()).toHaveLength(1);
      });
      await journey.stage("Submit an exact Usage token from the composer (cas-62ea)", async () => {
        await input.fill("/usage");
        await page.locator("#message-send").click();
        await expect.poll(() => mutations().length).toBe(2);
        const request = mutations()[1].body as { client_ref: string; command: string };
        expect(request.command).toBe("usage");
        expect(hub.sends).toHaveLength(4);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { client_ref: request.client_ref, command: "usage", phase: "observed", detail: "Usage displayed by the supervisor's harness.", output: "Usage", interactive: false } } });
        await expect(page.locator(".supervisor-command-receipt").last()).toHaveAttribute("data-phase", "observed");
      });
      await journey.stage("Cancel Finish by keyboard from a right-aligned confirmation (cas-c009)", async () => {
        await input.fill("/finish"); await input.press("Enter");
        const dialog = page.getByRole("alertdialog", { name: "the cas-src supervisor" });
        const cancel = dialog.getByRole("button", { name: "Cancel", exact: true });
        const finish = dialog.getByRole("button", { name: "Finish", exact: true });
        await expect(cancel).toBeFocused();
        const [sheet, left, right] = await Promise.all([dialog.boundingBox(), cancel.boundingBox(), finish.boundingBox()]);
        if (!sheet || !left || !right) throw new Error("confirmation is not laid out");
        if (viewport.width > 500) {
          // Content-width actions side by side, primary last at the right edge.
          expect(Math.abs(left.y - right.y)).toBeLessThan(2);
          expect(right.x).toBeGreaterThan(left.x + left.width);
          expect(sheet.x + sheet.width - (right.x + right.width)).toBeLessThan(24);
          expect(left.width + right.width).toBeLessThan(sheet.width / 2);
        } else {
          // A phone stacks full-width actions, the primary above Cancel.
          expect(right.y + right.height).toBeLessThanOrEqual(left.y + 1);
          expect(left.width).toBeGreaterThan(sheet.width * 0.8);
        }
        await page.keyboard.press("ArrowRight");
        await expect(finish).toBeFocused();
        await page.keyboard.press("Tab");
        await expect(cancel).toBeFocused();
        await page.keyboard.press("Escape");
        await expect(dialog).toBeHidden();
        await expect(input).toBeFocused();
        await expect(input).toHaveValue("/finish");
        await input.fill("");
        expect(mutations()).toHaveLength(2);
      });
      await journey.stage("Finish waits for the active turn, then its reply reaches the conversation", async () => {
        await input.fill("/finish"); await input.press("Enter");
        await page.getByRole("alertdialog").getByRole("button", { name: "Finish", exact: true }).click();
        await expect.poll(() => mutations().length).toBe(3);
        const request = mutations()[2].body as { client_ref: string; command: string; confirmed: boolean };
        expect(request).toMatchObject({ command: "finish", confirmed: true });
        const receipt = page.locator(".supervisor-command-receipt").filter({ hasText: "/finish" });
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { client_ref: request.client_ref, command: "finish", phase: "queued", detail: "Finish is queued until the supervisor's current turn ends and its prompt is empty. Nothing has been typed yet.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "queued");
        await expect(receipt).toContainText("Nothing has been typed yet");
        expect(mutations()).toHaveLength(3);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { client_ref: request.client_ref, command: "finish", phase: "submitted", detail: "Command submitted; harness execution is not confirmed yet.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "submitted");
        hub.send(PELICAN, { OperatorReply: { notification_id: 88001, reply_to: null, message: "Handoff saved and background monitor stopped.", summary: "Finish reply", device_id: "journey-device", kind: "answer", session: PELICAN } });
        await expect(page.locator('.thread [role="log"]')).toContainText("Handoff saved and background monitor stopped.");
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { client_ref: request.client_ref, command: "finish", phase: "observed", detail: "The harness completed the Finish request. Review its reply for the wrap-up result; this does not imply the session exited.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "observed");
        expect(mutations()).toHaveLength(3);
      });
      await journey.stage("Cancel Clear, then confirm Exit for the named supervisor", async () => {
        await page.getByRole("button", { name: "Supervisor commands", exact: true }).click();
        await page.locator('[data-supervisor-command="clear"]').click();
        const dialog = page.getByRole("alertdialog");
        await expect(dialog).toContainText("cas-src");
        await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
        expect(mutations()).toHaveLength(3);
        await expect(input).toBeFocused();
        await input.fill("/exit"); await input.press("Enter");
        await expect(dialog).toContainText("Its harness will end");
        await expect(dialog.getByRole("button", { name: "Cancel", exact: true })).toBeFocused();
        await page.keyboard.press("ArrowRight");
        await expect(dialog.getByRole("button", { name: "Exit", exact: true })).toBeFocused();
        await page.keyboard.press("Enter");
        await expect.poll(() => mutations().length).toBe(4);
        expect(mutations()[3].body).toMatchObject({ command: "exit", confirmed: true });
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
      });
      await journey.stage("Remove the stopped session after Exit without claiming an unobserved command result", async () => {
        const request = mutations()[3].body as { client_ref: string };
        const receipt = page.locator(".supervisor-command-receipt").filter({ hasText: "/exit" });
        hub.send(PELICAN, { Error: { client_ref: request.client_ref, message: "The session's daemon connection is reconnecting, so the message was not sent. Retry once the session is live again." } });
        await expect(receipt).toHaveAttribute("data-phase", "unconfirmed");
        const session = machine.sessions[0]!;
        session.ended = { at: journeyStamp(), reason: "prompt_input_exit", workers_stopped: [...session.workers], orphans: [], unverified: [] };
        session.liveness = "stale_metadata"; session.workers = [];
        await hub.announceCatalog(machine.id);
        await expect(page.locator("#conversation-list .conversation-row")).toHaveCount(0);
        await expect(page.locator("#message-text")).toBeHidden();
        await expect(page.locator(".terminal-disconnected-banner")).toHaveCount(0);
        // The catalog's stop removes navigation, without inferring harness
        // execution from a missing row or displaying an endless command retry.
        expect(mutations()).toHaveLength(4);
      });
    });
  });
}

test("HUB-J27 explain commands without control rights and keep Raw output usable", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [{ ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "claude" as const })) }], paired: ["atlas"], commands: [PELICAN], scopes: { atlas: SCOPES.filter(scope => scope !== "pane-input") } });
  await journey.stage("Open the quick controls and read the permission explanation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
    await page.getByRole("button", { name: "Supervisor commands", exact: true }).click();
    await expect(page.locator("#supervisor-command-palette")).toContainText("paired without command control");
    expect(hub.frames.filter(frame => frame.kind === "SupervisorCommand")).toHaveLength(0);
    await page.keyboard.press("Escape");
  });
  await journey.stage("Read adjacent Raw output and return by keyboard", async () => {
    const raw = page.getByRole("button", { name: "Raw output", exact: true });
    await raw.click();
    const dialog = page.getByRole("dialog", { name: "Raw output" });
    await expect(dialog.getByRole("log")).toContainText("The supervisor is ready.");
    await page.keyboard.press("Escape");
    await expect(raw).toBeFocused();
  });
});
