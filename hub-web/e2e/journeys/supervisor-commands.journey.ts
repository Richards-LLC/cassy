import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { SCOPES } from "./hub-double";
import { journeyStamp } from "./clock";

for (const viewport of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
  test.describe(`${viewport.width}`, () => {
    test.use({ viewport });
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
      await journey.stage("Cancel Clear, then confirm Exit for the named supervisor", async () => {
        await page.getByRole("button", { name: "Supervisor commands", exact: true }).click();
        await page.locator('[data-supervisor-command="clear"]').click();
        const dialog = page.getByRole("alertdialog");
        await expect(dialog).toContainText("cas-src");
        await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
        expect(mutations()).toHaveLength(2);
        await expect(input).toBeFocused();
        await input.fill("/exit"); await input.press("Enter");
        await expect(dialog).toContainText("Its harness will end");
        await dialog.getByRole("button", { name: "Exit", exact: true }).click();
        await expect.poll(() => mutations().length).toBe(3);
        expect(mutations()[2].body).toMatchObject({ command: "exit", confirmed: true });
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
      });
      await journey.stage("Remove the stopped session after Exit without claiming an unobserved command result", async () => {
        const request = mutations()[2].body as { client_ref: string };
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
        expect(mutations()).toHaveLength(3);
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
