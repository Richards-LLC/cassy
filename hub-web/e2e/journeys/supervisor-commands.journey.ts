import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { SCOPES } from "./hub-double";
import { journeyStamp } from "./clock";
import { mkdirSync, writeFileSync } from "node:fs";
import type { Page } from "@playwright/test";

// Located by role and accessible name (cas-81e3): the command palette is the
// "Supervisor commands" region, and each command button is named "/<id> · <label>".
const commandPalette = (page: Page) => page.getByRole("region", { name: "Supervisor commands", exact: true });
const commandButton = (page: Page, id: string) => commandPalette(page).getByRole("button", { name: new RegExp(`^/${id} · `) });
const commandButtons = (page: Page) => commandPalette(page).getByRole("button", { name: /^\/[a-z]+ · / });
const composer = (page: Page) => page.getByRole("textbox", { name: "Your message", exact: true });
const sendButton = (page: Page) => page.getByRole("button", { name: /^Send to the .+ supervisor$/ });
const conversationList = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" });
// A row is named "<project> on <machine>, …"; the row's own controls are not.
const conversationRow = (page: Page) => conversationList(page).getByRole("button", { name: /^cas-src on / });
const receiptRegion = (page: Page) => page.getByRole("region", { name: "Supervisor command receipts", exact: true });
const commandReceipts = (page: Page) => receiptRegion(page).getByRole("article");
const conversationLog = (page: Page) => page.getByRole("log");
const toast = (page: Page, text: string) => page.getByRole("status").filter({ hasText: text });

/** cas-0d58: whether two elements' visible ink overlaps. Text is measured by
 * its own range, clipped by every overflow-clipping box around it, so text
 * that spills past a shrunk box counts and text an ellipsis hides does not. */
async function inkOverlaps(page: Page, a: string, b: string): Promise<boolean> {
  return page.evaluate(([a, b]) => {
    const ink = (selector: string) => {
      const el = document.querySelector(selector);
      if (!el) throw new Error(`missing ${selector}`);
      const range = document.createRange(); range.selectNodeContents(el);
      const text = range.getBoundingClientRect(), box = el.getBoundingClientRect();
      let { left, top, right, bottom } = { left: Math.min(text.left, box.left), top: Math.min(text.top, box.top), right: Math.max(text.right, box.right), bottom: Math.max(text.bottom, box.bottom) };
      for (let node: Element | null = el; node && node !== document.body; node = node.parentElement) {
        if (getComputedStyle(node).overflowX === "visible") continue;
        const clip = node.getBoundingClientRect();
        left = Math.max(left, clip.left); right = Math.min(right, clip.right); top = Math.max(top, clip.top); bottom = Math.min(bottom, clip.bottom);
      }
      return { left, top, right, bottom };
    };
    const [x, y] = [ink(a), ink(b)];
    return x.left < x.right && y.left < y.right && x.left < y.right - 0.5 && y.left < x.right - 0.5 && x.top < y.bottom - 0.5 && y.top < x.bottom - 0.5;
  }, [a, b] as const);
}

/** cas-0d58: a badge is read whole: nothing clipped or ellipsised, and inside the viewport. */
async function wholeInView(page: Page, selector: string): Promise<boolean> {
  return page.locator(selector).evaluate(el => {
    const box = el.getBoundingClientRect();
    return el.scrollWidth <= el.clientWidth && box.width > 0 && box.left >= 0 && box.right <= window.innerWidth;
  });
}

async function captureReload(page: Page, name: string, width: number): Promise<void> {
  const dir = process.env.QA_ARTIFACTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const cssUrl = new URL("app.css", page.url());
  let css = await (await page.request.get(cssUrl.href)).text();
  for (const match of [...css.matchAll(/url\(([^)]+)\)/g)]) {
    const path = match[1].replace(/["\']/g, "");
    if (path.startsWith("data:") || path.startsWith("#")) continue;
    const response = await page.request.get(new URL(path, cssUrl).href);
    css = css.replaceAll(match[0], `url(data:${response.headers()["content-type"] ?? "application/octet-stream"};base64,${(await response.body()).toString("base64")})`);
  }
  const html = await page.evaluate(() => {
    const clone = document.documentElement.cloneNode(true) as HTMLElement;
    clone.removeAttribute("data-scheme");
    clone.querySelectorAll("script, link, style").forEach(node => node.remove());
    return clone.outerHTML;
  });
  writeFileSync(`${dir}/${name}-${width}.html`, "<!doctype html>" + html.replace("</head>", `<style>${css}</style></head>`));
  await page.screenshot({ path: `${dir}/${name}-${width}.png` });
}

for (const viewport of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
  test.describe(`${viewport.width}`, () => {
    test.use({ viewport, colorScheme: viewport.width === 390 ? "dark" : "light" });
    // cas-bcf0: a phone header keeps Commands in its overflow menu.
    const phone = viewport.width === 390;
    const openCommands = async (page: Page) => {
      if (phone) await page.getByRole("button", { name: "More conversation actions", exact: true }).click();
      await page.getByRole("button", { name: "Supervisor commands", exact: true }).click();
    };
    test("HUB-J27 autocomplete Finish clears the composer like the Commands menu", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "claude" as const })) };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN] });
      const input = composer(page);
      const mutations = () => hub.frames.filter(frame => frame.kind === "SupervisorCommand");
      await journey.stage("Run Finish from the header menu", async () => {
        await journey.open();
        await conversationRow(page).click();
        await openCommands(page);
        await commandButton(page, "finish").click();
        await page.getByRole("alertdialog").getByRole("button", { name: "Finish", exact: true }).click();
        await expect.poll(() => mutations().length).toBe(1);
        const request = mutations()[0].body as { client_ref: string; command: string };
        expect(request.command).toBe("finish");
        const receipt = commandReceipts(page);
        await expect(receipt).toHaveAttribute("data-phase", "submitted");
        if (!phone) {
          const [status, transcript] = await Promise.all([receipt.boundingBox(), receiptRegion(page).boundingBox()]);
          expect(status!.width).toBeLessThan(transcript!.width * 0.9);
        }
        await captureReload(page, "finish-menu-requested", viewport.width);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "finish", phase: "observed", detail: "Completed", output: "", interactive: false } } });
        await expect(commandReceipts(page)).toHaveCount(0);
      });
      await journey.stage("Pick Finish from /fin autocomplete and settle its outcome", async () => {
        await input.fill("/fin");
        await commandButton(page, "finish").click();
        await page.getByRole("alertdialog").getByRole("button", { name: "Finish", exact: true }).click();
        await expect.poll(() => mutations().length).toBe(2);
        const request = mutations()[1].body as { client_ref: string; command: string; confirmed: boolean };
        expect(request).toMatchObject({ command: "finish", confirmed: true });
        expect(hub.sends).toHaveLength(0);
        await captureReload(page, "finish-autocomplete-requested", viewport.width);
        await expect(input).toHaveValue("");
        await expect(commandPalette(page)).toBeHidden();
        const statusReads = () => hub.frames.filter(frame => frame.kind === "SupervisorCommandStatus" && (frame.body as { client_ref: string }).client_ref === request.client_ref).length;
        // Replay the daemon's short timeout before the incident's 103-second completion.
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "finish", phase: "unconfirmed", detail: "Outcome not observed yet", output: "", interactive: false } } });
        await expect(commandReceipts(page)).toHaveAttribute("data-phase", "unconfirmed");
        const before = statusReads();
        await expect.poll(statusReads).toBeGreaterThan(before);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "finish", phase: "observed", detail: "Completed", output: "", interactive: false } } });
        await expect(commandReceipts(page)).toHaveCount(0);
        expect(mutations()).toHaveLength(2);
        await captureReload(page, "finish-autocomplete-completed", viewport.width);
      });
    });
    // cas-0699: /standby parks the session, the row says Standby, and Resume brings it back. The page drives
    // the real command path; the double answers as the daemon and the supervisor's hook would.
    test("HUB-J27 standby shows the badge and Resume clears it (cas-0699)", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "claude" as const })) };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN] });
      const row = conversationRow(page);
      const standbyBadge = conversationList(page).getByText("Standby", { exact: true });
      const resume = conversationList(page).getByRole("button", { name: "Resume", exact: true });
      await journey.stage("Go on standby from the header menu", async () => {
        await journey.open();
        await conversationRow(page).click();
        await expect(standbyBadge).toHaveCount(0);
        await openCommands(page);
        await commandButton(page, "standby").click();
        await page.getByRole("alertdialog").getByRole("button", { name: "Standby", exact: true }).click();
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommand").length).toBe(1);
        expect(hub.frames.find(frame => frame.kind === "SupervisorCommand")!.body).toMatchObject({ command: "standby", confirmed: true });
        if (viewport.width < 600) await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
        await expect(row.getByText("Standby", { exact: true })).toBeVisible();
        await expect(resume).toBeVisible();
      });
      await journey.stage("Resume clears the badge", async () => {
        await resume.click();
        await expect.poll(() => hub.sends.length).toBe(1);
        expect(hub.sends[0]).toMatchObject({ session: PELICAN, target: machine.sessions[0]!.supervisor });
        await expect(standbyBadge).toHaveCount(0);
        await expect(resume).toHaveCount(0);
      });
    });
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
        await conversationRow(page).click();
        await expect(conversationLog(page).getByRole("status")).toHaveAttribute("data-active", "false");
        // The terminal's command reads as one compact line, not a sent bubble "from Terminal".
        await expect(conversationLog(page).getByText("/mcp reconnect cas", { exact: true })).toBeVisible();
        await expect(conversationLog(page)).not.toContainText("<command-name>");
        await expect(conversationLog(page).getByText("from Terminal")).toHaveCount(0);
      });
      await journey.stage("Reload Cassy once from the idle Claude supervisor", async () => {
        const input = composer(page);
        await input.fill("/mcp");
        await expect(commandButton(page, "mcp")).toBeVisible();
        await input.press("Enter");
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommand").length).toBe(1);
        const frames = hub.frames.filter(frame => frame.kind === "SupervisorCommand");
        expect(frames).toHaveLength(1);
        const request = frames[0]!.body as { client_ref: string; command: string };
        expect(request.command).toBe("mcp");
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "observed", detail: "Cassy reloaded: PID 2202, version 9.99.2 (previous PID 1101).", output: "", interactive: false } } });
        // A finished command leaves a self-clearing toast, not a card (cas-95ea).
        await expect(toast(page, "Cassy reloaded: PID 2202")).toBeVisible();
        await expect(commandReceipts(page)).toHaveCount(0);
        await expect(toast(page, "Cassy reloaded: PID 2202")).toHaveCount(0, { timeout: 5_000 });
        expect(hub.sends).toHaveLength(0);
      });
      await journey.stage("Dismiss a stuck queued Finish through the daemon before Exit (cas-f1a2)", async () => {
        const input = composer(page);
        await input.fill("/finish"); await input.press("Enter");
        await page.getByRole("alertdialog").getByRole("button", { name: "Finish", exact: true }).click();
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommand").length).toBe(2);
        const request = hub.frames.filter(frame => frame.kind === "SupervisorCommand")[1]!.body as { client_ref: string; command: string };
        expect(request.command).toBe("finish");
        const receipt = commandReceipts(page).filter({ hasText: "/finish" });
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "finish", phase: "queued", detail: "Waiting for an empty prompt", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "queued");
        await receipt.getByRole("button", { name: "Dismiss command status", exact: true }).click();
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommandCancel").length).toBe(1);
        expect(hub.frames.find(frame => frame.kind === "SupervisorCommandCancel")!.body).toEqual({ client_ref: request.client_ref });
        await expect(receipt).toBeVisible();
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "finish", phase: "cancelled", detail: "Cancelled before submission", output: "", interactive: false } } });
        await expect(receipt).toHaveCount(0);
        expect(hub.sends).toHaveLength(0);
      });
      await journey.stage("Exit once after the native turn has completed", async () => {
        const input = composer(page);
        await input.fill("/exit"); await input.press("Enter");
        await page.getByRole("alertdialog").getByRole("button", { name: "Exit", exact: true }).click();
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommand").length).toBe(3);
        const frames = hub.frames.filter(frame => frame.kind === "SupervisorCommand");
        expect(frames).toHaveLength(3);
        expect(frames[2]!.body).toMatchObject({ command: "exit", confirmed: true });
        expect(hub.sends).toHaveLength(0);
      });
    });
    test("HUB-J27 reload Claude's Cassy server and read its replacement process", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "claude" as const, mcp_version: "9.99.1", installed_version: "9.99.2" })) };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN] });
      const mutations = () => hub.frames.filter(frame => frame.kind === "SupervisorCommand");
      const versionOnRow = (version: string | RegExp) => page.getByRole("navigation", { name: "Choose a supervisor", includeHidden: true })
        .getByRole("button", { name: /^cas-src on /, includeHidden: true }).getByText(version);
      await journey.stage("See Reload Cassy on the older supervisor card without opening a menu", async () => {
        await journey.open();
        await expect(conversationList(page).getByRole("button", { name: "Reload Cassy", exact: true })).toBeVisible();
        await expect(conversationRow(page).getByText("9.99.1")).toBeVisible();
        await captureReload(page, "card", viewport.width);
      });
      await journey.stage("Reload from the card and wait for the replacement receipt and catalog", async () => {
        await conversationList(page).getByRole("button", { name: "Reload Cassy", exact: true }).click();
        await expect.poll(() => mutations().length).toBe(1);
        const request = mutations()[0].body as { client_ref: string; command: string };
        expect(request.command).toBe("mcp");
        const receipt = commandReceipts(page);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "queued", detail: "Reload is queued until this turn ends.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "queued");
        // cas-8c4f: a busy supervisor shows the wait instead of a refusal.
        await expect(receipt.locator(".reload-wait")).toBeVisible();
        await captureReload(page, "waiting", viewport.width);
        // A phone now shows the conversation; the list row behind it keeps the old version.
        await expect(versionOnRow("9.99.1")).toHaveCount(1);
        machine.sessions[0]!.mcp_version = "9.99.2";
        (machine.sessions[0] as { mcp_loaded_at?: string }).mcp_loaded_at = "2026-11-23T22:44:00Z";
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "observed", detail: "Cassy reloaded: PID 2202, version 9.99.2 (previous PID 1101).", output: "", interactive: false } } });
        await expect(toast(page, "Cassy reloaded: PID 2202")).toBeVisible();
        await expect(receipt).toHaveCount(0);
        await captureReload(page, "reloaded-toast", viewport.width);
        // cas-8c4f: the card and the open conversation keep the loaded build and its Eastern load time.
        await expect(versionOnRow("9.99.2")).toHaveCount(1);
        await expect(page.getByTestId("conversation-cassy")).toContainText("9.99.2");
        await expect(page.getByTestId("conversation-cassy")).toContainText("5:44 PM");
        await captureReload(page, "loaded-badge", viewport.width);
        await expect(conversationList(page).getByRole("button", { name: "Reload Cassy", exact: true })).toBeHidden();
        expect(hub.sends).toHaveLength(0);
      });
      await journey.stage("The loaded-build badge survives a Commander refresh", async () => {
        await page.reload();
        await expect(versionOnRow("9.99.2")).toHaveCount(1);
        // A phone restores the open conversation over the list; a desktop shows both.
        if (await conversationRow(page).isVisible()) await conversationRow(page).click();
        await expect(page.getByTestId("conversation-cassy")).toContainText("9.99.2");
        // cas-0d58: the badges stay in their own columns, clear of the label and actions.
        expect(await inkOverlaps(page, "[data-testid=conversation-cassy]", "[data-testid=conversation-header-actions]")).toBe(false);
        expect(await inkOverlaps(page, "[data-testid=conversation-cassy]", "[data-testid=host-machine]")).toBe(false);
        expect(await wholeInView(page, "[data-testid=conversation-cassy]")).toBe(true);
        if (phone) await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
        await expect(versionOnRow("9.99.2")).toBeVisible();
        expect(await inkOverlaps(page, "[data-testid=supervisor-version]", ".conversation-row:has([data-testid=supervisor-version]) [data-testid=conversation-machine-name]")).toBe(false);
        expect(await wholeInView(page, "[data-testid=supervisor-version]")).toBe(true);
        await captureReload(page, "badges-clear", viewport.width);
        if (phone) await conversationRow(page).click();
      });
      await journey.stage("Open the header dropdown beside its button, then submit typed reload-cassy without chat", async () => {
        // On a phone the palette opens under the overflow toggle it was chosen from.
        const toggle = page.getByRole("button", { name: phone ? "More conversation actions" : "Supervisor commands", exact: true });
        await openCommands(page);
        const palette = commandPalette(page);
        await expect(commandButton(page, "mcp")).toContainText(/\S/);
        await expect(commandButton(page, "mcp")).toBeVisible();
        const [buttonBox, paletteBox] = await Promise.all([toggle.boundingBox(), palette.boundingBox()]);
        if (!buttonBox || !paletteBox) throw new Error("Supervisor command dropdown is not laid out");
        expect(paletteBox.y).toBeGreaterThanOrEqual(buttonBox.y + buttonBox.height);
        expect(paletteBox.y - buttonBox.y - buttonBox.height).toBeLessThan(24);
        expect(paletteBox.x).toBeGreaterThanOrEqual(0);
        expect(paletteBox.x + paletteBox.width).toBeLessThanOrEqual(viewport.width);
        await captureReload(page, "dropdown", viewport.width);
        await page.keyboard.press("Escape");
        await expect(toggle).toBeFocused();
        const input = composer(page);
        // cas-8c4f: /reload-cassy is the same semantic command as /mcp.
        await input.fill("/reload-cassy");
        await expect(commandButton(page, "mcp")).toContainText(/\S/);
        await input.press("Enter");
        await expect.poll(() => mutations().length).toBe(2);
        const request = mutations()[1].body as { client_ref: string; command: string };
        expect(request.command).toBe("mcp");
        expect(hub.sends).toHaveLength(0);
        // A second Enter must not reload again: the alias leaves the composer.
        await expect(input).toHaveValue("");
        const receipt = commandReceipts(page);
        await expect(receipt).toHaveAttribute("data-phase", "submitted");
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "observed", detail: "Cassy reloaded: PID 3303, version 9.99.2 (previous PID 2202).", output: "", interactive: false } } });
        await expect(toast(page, "PID 3303, version 9.99.2")).toBeVisible();
        await expect(receipt).toHaveCount(0);
        expect(mutations()).toHaveLength(2);
      });
    });
    test("HUB-J27 reload Cassy on a Codex supervisor only after confirming its restart", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: [{ ...ATLAS.sessions[0]!, harness: "codex" as const, mcp_version: "9.99.2", installed_version: "9.99.2", mcp_reload_needed: true, epic_id: null, ws_port: 48101, dormant: false, standby: false, dismissed: false, started_at: journeyStamp(-60_000) }] };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN] });
      const mutations = () => hub.frames.filter(frame => frame.kind === "SupervisorCommand");
      await journey.stage("The Codex card names the restart before anything is sent", async () => {
        await journey.open();
        await conversationList(page).getByRole("button", { name: "Reload Cassy", exact: true }).click();
        await expect(conversationList(page).getByRole("button", { name: "Restart and reload", exact: true })).toBeVisible();
        expect(mutations()).toHaveLength(0);
        await captureReload(page, "codex-confirm", viewport.width);
      });
      await journey.stage("Confirming sends one confirmed reload and shows the wait, then a mismatch names the next step", async () => {
        await conversationList(page).getByRole("button", { name: "Restart and reload", exact: true }).click();
        await expect.poll(() => mutations().length).toBe(1);
        const request = mutations()[0].body as { client_ref: string; command: string; confirmed: boolean };
        expect(request.command).toBe("mcp");
        expect(request.confirmed).toBe(true);
        const receipt = commandReceipts(page);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "submitted", detail: "Restarting the Codex supervisor on the installed build; workers keep running.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "submitted");
        await expect(receipt.locator(".reload-wait")).toBeVisible();
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "unconfirmed", detail: "Cassy reconnected but runs 9.99.2 (c140f3f), not the installed 9.99.2 (d140f3f). Check which cas is first on PATH, then Reload Cassy again.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "unconfirmed");
        await expect(receipt.locator(".reload-wait")).toHaveCount(0);
        await captureReload(page, "codex-mismatch", viewport.width);
        expect(hub.sends).toHaveLength(0);
      });
      const reload = page.locator('[data-command-confirm="mcp"]');
      if (phone) await journey.stage("Back and the header toggle dismiss a header confirmation", async () => {
        const openReload = async () => { await openCommands(page); await commandButton(page, "mcp").click(); };
        await openReload();
        await expect(reload).toBeVisible();
        await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
        await expect(reload).toHaveCount(0);
        await conversationRow(page).click();
        await openReload();
        await page.getByRole("button", { name: "More conversation actions", exact: true }).press("Enter");
        await page.getByRole("button", { name: "Supervisor commands", exact: true }).click();
        await expect(reload).toHaveCount(0);
        expect(mutations()).toHaveLength(1);
      });
      await journey.stage("Typed Reload Cassy keeps its focused action when the session list changes", async () => {
        hub.releaseOwnLease(PELICAN);
        await page.reload();
        const input = composer(page);
        await input.fill("/reload-cassy");
        await expect(commandButton(page, "mcp")).toBeVisible();
        await input.press("Enter");
        await expect(reload).toBeVisible();
        await page.keyboard.press("ArrowRight");
        await expect(reload).toBeFocused();
        const peer = { ...machine.sessions[0]!, name: "calm-wren-2", supervisor: "calm-wren-2", project_dir: "/projects/peer-project", workers: [], ws_port: 48102, started_at: journeyStamp(-30_000) };
        machine.sessions.push(peer);
        await hub.announceCatalog(machine.id, { added: [peer.name] });
        const navigation = page.getByRole("navigation", { name: "Choose a supervisor", includeHidden: true });
        const peerRow = navigation.getByRole("button", { name: /^peer-project on /, includeHidden: true });
        await expect(peerRow).toHaveCount(1);
        await expect(reload).toBeFocused();
        await captureReload(page, "typed-confirm-after-catalog", viewport.width);
        await reload.hover();
        await page.mouse.down();
        const arriving = { ...peer, name: "bright-robin-3", supervisor: "bright-robin-3", project_dir: "/projects/arriving-project", ws_port: 48103, started_at: journeyStamp(-10_000) };
        machine.sessions.push(arriving);
        await hub.announceCatalog(machine.id, { added: [arriving.name] });
        await expect(navigation.getByRole("button", { name: /^arriving-project on /, includeHidden: true })).toHaveCount(1);
        expect(mutations()).toHaveLength(1);
        const leaseTake = hub.holdNextLeaseTake(PELICAN);
        try {
          await page.mouse.up();
          await leaseTake.started;
          const incoming = { ...peer, name: "settled-swift-4", supervisor: "settled-swift-4", project_dir: "/projects/incoming-project", ws_port: 48104, started_at: journeyStamp() };
          machine.sessions.push(incoming);
          await hub.announceCatalog(machine.id, { added: [incoming.name] });
          await expect(navigation.getByRole("button", { name: /^incoming-project on /, includeHidden: true })).toHaveCount(1);
        } finally {
          leaseTake.release();
        }
        await expect.poll(() => mutations().length).toBe(2);
        expect(mutations()[1].body).toMatchObject({ command: "mcp", confirmed: true });
        await expect(input).toHaveValue("");
        await expect(input).toBeFocused();
        expect(hub.sends).toHaveLength(0);
      });
      await journey.stage("A refused take names the pane controller without sending Reload", async () => {
        hub.holdLease(PELICAN, "Studio iPad");
        await page.reload();
        const input = composer(page);
        await expect(input).toHaveValue("");
        await input.fill("/reload-cassy");
        await expect(commandButton(page, "mcp")).toBeVisible();
        await input.press("Enter");
        await reload.click();
        await expect(commandPalette(page).getByRole("status")).toContainText("Studio iPad");
        await openCommands(page);
        await expect(page.locator("#conversation-commands")).toHaveAttribute("aria-expanded", "true");
        await expect(commandButton(page, "mcp")).toBeVisible();
        await expect(input).toHaveValue("/reload-cassy");
        expect(mutations()).toHaveLength(2);
        await captureReload(page, "codex-refused-then-commands", viewport.width);
      });
    });
    test("HUB-J27 dismiss reload refusal and resolve a later observed receipt", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "claude" as const })) };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN] });
      await journey.stage("Dismiss a refused busy reload without sending chat or retrying it", async () => {
        await journey.open();
        await conversationRow(page).click();
        await composer(page).fill("/mcp");
        // Until the session reports command support the palette says so and
        // Send runs nothing; wait for the command to be offered, as a user would.
        await expect(commandButton(page, "mcp")).toBeVisible();
        await sendButton(page).click();
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommand").length).toBe(1);
        const request = hub.frames.find(frame => frame.kind === "SupervisorCommand")!.body as { client_ref: string };
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "refused", detail: "The supervisor is busy or its empty prompt is not confirmed. Wait until its turn ends.", output: "", interactive: false } } });
        const receipt = commandReceipts(page);
        await expect(receipt).toHaveAttribute("data-phase", "refused");
        await receipt.getByRole("button", { name: "Dismiss command status" }).click();
        await expect(receipt).toHaveCount(0);
        expect(hub.sends).toHaveLength(0);
        expect(hub.frames.filter(frame => frame.kind === "SupervisorCommand")).toHaveLength(1);
      });
      await journey.stage("A later queued reload resolves and leaves no stale busy panel", async () => {
        await composer(page).fill("/mcp");
        await sendButton(page).click();
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommand").length).toBe(2);
        const request = hub.frames.filter(frame => frame.kind === "SupervisorCommand")[1].body as { client_ref: string };
        const receipt = commandReceipts(page);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "queued", detail: "Reload queued for the end of this turn.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "queued");
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "mcp", phase: "observed", detail: "Cassy reloaded: PID 2202, version 9.99.2.", output: "", interactive: false } } });
        await expect(toast(page, "Cassy reloaded: PID 2202")).toBeVisible();
        await expect(receipt).toHaveCount(0);
        expect(hub.sends).toHaveLength(0);
      });
    });
    test("HUB-J27 run a supervisor command and read its actual outcome", journeyPart, async ({ page, journey }) => {
      const machine = { ...ATLAS, sessions: ATLAS.sessions.map(session => ({ ...session, harness: "codex" as const })) };
      const hub = await journey.hub({ machines: [machine], paired: ["atlas"], commands: [PELICAN] });
      const input = composer(page);
      const mutations = () => hub.frames.filter(frame => frame.kind === "SupervisorCommand");
      let ref = "";
      await journey.stage("Choose Usage from the slash palette with the keyboard", async () => {
        // Type before the conversation's socket is live: the palette lists the
        // command the moment it connects, without the text changing.
        const releaseAttach = hub.holdAttach(PELICAN);
        await journey.open();
        await conversationRow(page).click();
        await expect(page.getByRole("button", { name: phone ? "More conversation actions" : "Supervisor commands", exact: true })).toBeVisible();
        await input.fill("/us");
        await expect(commandPalette(page)).toBeVisible();
        await expect(commandButtons(page)).toHaveCount(0);
        releaseAttach();
        await expect(commandButtons(page)).toHaveCount(1);
        await input.press("ArrowDown");
        await expect(commandButton(page, "usage")).toBeFocused();
        await page.keyboard.press("Enter");
        await expect.poll(() => mutations().length).toBe(1);
        const request = mutations()[0].body as { client_ref: string; command: string; confirmed: boolean };
        expect(request.command).toBe("usage"); ref = request.client_ref;
        await expect(commandReceipts(page)).toHaveAttribute("data-phase", "submitted");
        expect(hub.sends).toHaveLength(0);
        await expect(input).toBeFocused();
        expect(await page.evaluate(() => {
          const days: Record<string, Record<string, number>> = JSON.parse(localStorage.getItem("commander.usage.v1") ?? "{}").days ?? {};
          return Object.values(days).reduce((sum, day) => sum + (day["supervisor.command"] ?? 0), 0);
        })).toBe(1);
      });
      await journey.stage("Read the observed native result without resending the command", async () => {
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: ref, command: "usage", phase: "observed", detail: "Usage displayed by the supervisor's harness.", output: "Model: GPT-6.1-Sol\nContext window: 93% left\n<untrusted>", interactive: false } } });
        const receipt = commandReceipts(page);
        await expect(receipt).toHaveAttribute("data-phase", "observed");
        await receipt.getByText("Harness output", { exact: true }).click();
        // The literal tag is text: markup in harness output never becomes an element.
        await expect(receipt.getByRole("group")).toContainText("<untrusted>");
        expect(mutations()).toHaveLength(1);
      });
      await journey.stage("Send pasted paths and unknown slash text as messages (cas-62ea)", async () => {
        const messages = ["/home/pippenz/notes.md", "/mnt/notes.md review this", "/shell whoami", "//usage"];
        for (const [index, text] of messages.entries()) {
          await input.fill(text);
          await expect(commandPalette(page)).toBeHidden();
          if (index % 2 === 0) await input.press("Enter");
          else await sendButton(page).click();
          await expect.poll(() => hub.sends.length).toBe(index + 1);
          expect(hub.sends[index].text).toBe(text);
          await expect(input).toHaveValue("");
        }
        expect(mutations()).toHaveLength(1);
      });
      await journey.stage("Submit an exact Usage token from the composer (cas-62ea)", async () => {
        await input.fill("/usage");
        await sendButton(page).click();
        await expect.poll(() => mutations().length).toBe(2);
        const request = mutations()[1].body as { client_ref: string; command: string };
        expect(request.command).toBe("usage");
        expect(hub.sends).toHaveLength(4);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "usage", phase: "observed", detail: "Usage displayed by the supervisor's harness.", output: "Usage", interactive: false } } });
        await expect(commandReceipts(page).last()).toHaveAttribute("data-phase", "observed");
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
      await journey.stage("Finish keeps polling after a refused Cancel, then its reply reaches the conversation (cas-5b23)", async () => {
        await input.fill("/finish"); await input.press("Enter");
        await page.getByRole("alertdialog").getByRole("button", { name: "Finish", exact: true }).click();
        await expect.poll(() => mutations().length).toBe(3);
        const request = mutations()[2].body as { client_ref: string; command: string; confirmed: boolean };
        expect(request).toMatchObject({ command: "finish", confirmed: true });
        const receipt = commandReceipts(page).filter({ hasText: "/finish" });
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "finish", phase: "queued", detail: "Finish is queued until the supervisor's current turn ends and its prompt is empty. Nothing has been typed yet.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "queued");
        await expect(receipt).toContainText("Nothing has been typed yet");
        expect(mutations()).toHaveLength(3);
        await receipt.getByRole("button", { name: "Dismiss command status" }).click();
        await expect.poll(() => hub.frames.filter(frame => frame.kind === "SupervisorCommandCancel").length).toBe(1);
        const cancelFailure = "supervisor command failed: already submitted";
        hub.send(PELICAN, { Error: { client_ref: request.client_ref, message: cancelFailure } });
        // The daemon's error payload must reach the row before counting polls.
        await expect(receipt.getByRole("paragraph")).toHaveText(cancelFailure);
        const statusReads = () => hub.frames.filter(frame => frame.kind === "SupervisorCommandStatus" && (frame.body as { client_ref: string }).client_ref === request.client_ref).length;
        const before = statusReads();
        await expect.poll(statusReads).toBeGreaterThan(before);
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "finish", phase: "submitted", detail: "Command submitted; harness execution is not confirmed yet.", output: "", interactive: false } } });
        await expect(receipt).toHaveAttribute("data-phase", "submitted");
        hub.send(PELICAN, { OperatorReply: { notification_id: 88001, reply_to: null, message: "Handoff saved and background monitor stopped.", summary: "Finish reply", device_id: "journey-device", kind: "answer", session: PELICAN } });
        await expect(conversationLog(page)).toContainText("Handoff saved and background monitor stopped.");
        hub.send(PELICAN, { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: request.client_ref, command: "finish", phase: "observed", detail: "The harness completed the Finish request. Review its reply for the wrap-up result; this does not imply the session exited.", output: "", interactive: false } } });
        await expect(toast(page, "The harness completed the Finish request.")).toBeVisible();
        await expect(receipt).toHaveCount(0);
        expect(mutations()).toHaveLength(3);
      });
      await journey.stage("Cancel Clear, then confirm Exit for the named supervisor", async () => {
        await openCommands(page);
        await commandButton(page, "clear").click();
        const dialog = page.getByRole("alertdialog");
        await expect(dialog).toContainText("cas-src");
        const clearConsequence = await dialog.getByRole("paragraph").innerText();
        expect(clearConsequence).toMatch(/\S/);
        await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
        expect(mutations()).toHaveLength(3);
        await expect(input).toBeFocused();
        await input.fill("/exit"); await input.press("Enter");
        await expect(dialog).toBeVisible();
        // Exit states its own consequence (not Clear's), and the dialog is described by it.
        const exitConsequence = dialog.getByRole("paragraph");
        await expect(exitConsequence).toHaveText(/\S/);
        expect(await exitConsequence.innerText()).not.toBe(clearConsequence);
        await expect(dialog).toHaveAccessibleDescription(await exitConsequence.innerText());
        await expect(dialog).toContainText("cas-src");
        await expect(dialog.getByRole("button", { name: "Exit", exact: true })).toBeVisible();
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
        const receipt = commandReceipts(page).filter({ hasText: "/exit" });
        hub.send(PELICAN, { Error: { client_ref: request.client_ref, message: "The session's daemon connection is reconnecting, so the message was not sent. Retry once the session is live again." } });
        await expect(receipt).toHaveAttribute("data-phase", "unconfirmed");
        const session = machine.sessions[0]!;
        session.ended = { at: journeyStamp(), reason: "prompt_input_exit", workers_stopped: [...session.workers], orphans: [], unverified: [] };
        session.liveness = "stale_metadata"; session.workers = [];
        const catalog = page.waitForResponse(response => new URL(response.url()).pathname === "/v1/sessions" && response.request().method() === "GET");
        await hub.announceCatalog(machine.id);
        expect(await (await catalog).json()).toMatchObject({
          schema_version: 1,
          freshness_threshold_secs: 30,
          sessions: [],
          stopped_sessions: [{ ...session, epic_id: null, ws_port: null, dormant: false, standby: false, dismissed: false, mcp_reload_needed: false }],
        });
        await expect(conversationRow(page)).toHaveCount(0);
        await expect(composer(page)).toBeHidden();
        // No banner reports the stopped session's machine connection as lost.
        await expect(page.getByRole("status").filter({ hasText: machine.label })).toHaveCount(0);
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
    await conversationRow(page).click();
    await page.getByRole("button", { name: "Supervisor commands", exact: true }).click();
    // Without command control the palette explains why and offers no command to run.
    const palette = commandPalette(page);
    await expect(palette.getByRole("status")).toHaveText(/\S/);
    await expect(commandButtons(page)).toHaveCount(0);
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
