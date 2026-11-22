import { clickConversationHeaderAction, test, expect, journeyPart } from "./journey";
import { SCOPES, type FleetWorld, type Machine } from "./hub-double";
import { ATLAS, PELICAN } from "./world";
import type { Page } from "@playwright/test";

// A row is named "<project> on <machine>, …"; the row's own controls are not.
const conversationRow = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /^cas-src on / });
const statusView = (page: Page) => page.getByTestId("status-view");
const grantResult = (page: Page) => statusView(page).getByRole("status", { name: "Write access result", exact: true });

// cas-ab04 (GH #1169 part 2): from a paired device the operator grants the
// agents on one task write access to a folder outside their worktree, until
// the task closes, and revokes it. The hub double answers like the hub's
// device-authenticated POST /v1/sessions/<s>/write-grants.
const fleet = (): FleetWorld => ({
  agents: [
    { name: "swift-lark-3", status: "active", current_task: "cas-1234", generation: 2 },
  ],
  tasks: [
    { id: "cas-1234", title: "Ingest request files", status: "in_progress", assignee: "swift-lark-3", updated_at: "2026-10-10T18:00:00Z" },
    { id: "cas-2001", title: "Footer copy", status: "open", assignee: null, updated_at: "2026-10-10T18:05:00Z" },
  ],
  epics: [{ id: "cas-978f" }],
  focused_epic: "cas-978f",
  spawnNames: [],
});

const fleetMachine = (machine: Machine): Machine => ({
  ...machine,
  sessions: machine.sessions.map((session) => ({ ...session, workers: fleet().agents.map((agent) => agent.name) })),
});

for (const [width, colorScheme, part] of [[1280, "light", false], [390, "dark", true]] as const) {
  test.describe(`write access ${width} ${colorScheme} cas_ab04`, () => {
    test.use({ viewport: { width, height: 844 }, colorScheme });
    const body = async ({ page, journey }: { page: import("@playwright/test").Page; journey: any }) => {
      const hub = await journey.hub({ machines: [fleetMachine(ATLAS)], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-operate", "factory-manage"] }, fleet: { [PELICAN]: fleet() } });
      const rail = statusView(page);
      const panel = rail.getByTestId("write-grant");
      const announcer = page.getByRole("status", { name: "Fleet results", exact: true });
      await journey.stage("Open the fleet", async () => {
        await journey.open();
        await conversationRow(page).click();
        if (width === 390) await clickConversationHeaderAction(page, "#conversation-fleet");
        await expect(rail.getByRole("button", { name: "Write access…" })).toBeVisible();
      });
      await journey.stage("Open Write access on the current task", async () => {
        await rail.getByRole("button", { name: "Write access…" }).click();
        await expect(panel).toBeVisible();
        // cas-4cf2: a visible title matching the accessible name, what the
        // grant does, and on a phone where it acts and a close named for it.
        await expect(panel.getByRole("heading", { level: 3, name: "Write access outside the worktree" })).toBeVisible();
        // cas-68d0 F01: the name is announced once: by the group at 1280, by
        // the phone sheet's dialog at 390, never also by an inner group.
        await expect(page.getByRole("group", { name: "Write access outside the worktree" })).toHaveCount(width === 390 ? 0 : 1);
        await expect(panel.getByText("Lets the agents on one task write to a folder outside their worktree until the task closes.", { exact: true })).toBeVisible();
        if (width === 390) {
          await expect(panel.getByText(/^cas-src on /)).toBeVisible();
          await expect(page.getByRole("dialog", { name: "Write access outside the worktree" }).getByRole("button", { name: "Close write access", exact: true })).toBeVisible();
        } else {
          await expect(panel.getByText(/^cas-src on /)).toHaveCount(0);
        }
        await expect(panel.getByRole("combobox", { name: "Task" })).toHaveValue("cas-1234");
        await expect(panel.getByRole("checkbox", { name: "create" })).toBeChecked();
        await expect(panel.getByRole("checkbox", { name: "edit" })).toBeChecked();
        await expect(panel.getByRole("checkbox", { name: "delete" })).not.toBeChecked();
      });
      await journey.stage("An incomplete grant says what is missing", async () => {
        const missing = "Enter the folder to grant, as an absolute path or ~/…";
        // cas-1380: record what the live region says, as a screen reader hears it.
        await announcer.evaluate((region) => {
          const heard: string[] = [];
          (window as unknown as { heardGrant: string[] }).heardGrant = heard;
          new MutationObserver(() => { if (region.textContent) heard.push(region.textContent); })
            .observe(region, { childList: true, characterData: true, subtree: true });
        });
        const heard = () => page.evaluate(() => (window as unknown as { heardGrant: string[] }).heardGrant);
        await panel.getByRole("button", { name: "Review grant" }).click();
        await expect(grantResult(page)).toHaveText(missing);
        // cas-5020: focus goes to the field that is missing, not the line.
        await expect(panel.getByRole("textbox", { name: "Folder" })).toBeFocused();
        await expect(announcer).toHaveText(missing);
        await expect.poll(heard, { message: "a first refusal is announced once" }).toEqual([missing]);
        // cas-1380: the same refusal again is announced again.
        await panel.getByRole("button", { name: "Review grant" }).click();
        await expect.poll(heard, { message: "a repeated refusal is announced again" }).toEqual([missing, missing]);
        await expect(announcer).toHaveText(missing);
        expect(hub.writeGrants).toHaveLength(0);
      });
      await journey.stage("Review, confirm and see the receipt", async () => {
        await panel.getByRole("textbox", { name: "Folder" }).fill("~/soundwave-config/docs/requests");
        await panel.getByRole("textbox", { name: "Reason" }).fill("INGEST request files");
        await panel.getByRole("button", { name: "Review grant" }).click();
        const confirm = rail.getByRole("alertdialog", { name: "Confirm write access" });
        // cas-42c0: Escape backs out of the confirmation to the form, with
        // focus on Review grant, and sends nothing.
        await expect(confirm).toBeVisible();
        await page.keyboard.press("Escape");
        await expect(confirm).toHaveCount(0);
        await expect(panel.getByRole("button", { name: "Review grant" })).toBeFocused();
        expect(hub.writeGrants).toHaveLength(0);
        await page.keyboard.press("Enter");
        await expect(confirm).toContainText(new RegExp("(?=[\\s\\S]*\\~/soundwave\\-config/docs/requests)(?=[\\s\\S]*cas\\-1234)"));
        await expect(confirm.getByRole("button")).toHaveText(["Cancel", "Grant"]);
        // cas-5020: a keyboard Grant.
        await confirm.getByRole("button", { name: "Grant", exact: true }).focus();
        await page.keyboard.press("Enter");
        const receipt = "Write access granted for cas-1234: /home/operator/soundwave-config/docs/requests (create+edit) until the task closes.";
        await expect(grantResult(page)).toHaveText(receipt);
        await expect(announcer).toHaveText(receipt);
        // cas-5020: focus lands on the receipt's next action (Revoke…), drawn
        // with the token focus ring, never on the status line.
        const revoke = panel.getByRole("button", { name: "Revoke…" });
        await expect(revoke).toBeFocused();
        await expect(grantResult(page)).not.toBeFocused();
        const ring = await revoke.evaluate((node) => {
          const probe = document.createElement("span");
          probe.style.outlineColor = "var(--color-focus)";
          probe.style.outlineWidth = "var(--focus-ring-width)";
          document.body.append(probe);
          const token = getComputedStyle(probe);
          const want = { color: token.outlineColor, width: token.outlineWidth };
          probe.remove();
          const style = getComputedStyle(node);
          return { color: style.outlineColor, width: style.outlineWidth, style: style.outlineStyle, offset: style.outlineOffset, want };
        });
        expect(ring.style).toBe("solid");
        expect(ring.color).toBe(ring.want.color);
        expect(ring.width).toBe(ring.want.width);
        expect(ring.offset).not.toBe("0px");
        // cas-06e8: the sent grant leaves no primed form, and the task id in
        // the receipt stays whole.
        await expect(panel.getByRole("textbox", { name: "Folder" })).toHaveValue("");
        await expect(panel.getByRole("textbox", { name: "Reason" })).toHaveValue("");
        await expect(panel.getByRole("combobox", { name: "Task" })).toHaveValue("cas-1234");
        await expect(grantResult(page).getByText("cas-1234", { exact: true })).toHaveText("cas-1234");
        await expect(grantResult(page).getByText("cas-1234", { exact: true })).toHaveCSS("white-space", "nowrap");
        // cas-a8d9: compare the receipt with the fade's rendered top, including
        // the sticky pseudo-element's negative margins inside the rail padding.
        if (width === 1280) {
          await expect.poll(() => grantResult(page).evaluate((line) => {
            const scroller = line.closest(".conversation-context")!;
            const fade = getComputedStyle(scroller, "::after");
            const probe = document.createElement("span");
            Object.assign(probe.style, {
              position: fade.position,
              display: fade.display,
              height: fade.height,
              marginBlockStart: fade.marginBlockStart,
              marginBlockEnd: fade.marginBlockEnd,
              bottom: fade.bottom,
              visibility: "hidden",
              pointerEvents: "none",
            });
            scroller.append(probe);
            const fadeTop = probe.getBoundingClientRect().top;
            probe.remove();
            return fadeTop - line.getBoundingClientRect().bottom;
          }), "every receipt line clears the rendered fade").toBeGreaterThanOrEqual(0);
        }
        expect(hub.writeGrants.map((call: any) => [call.status, call.body])).toEqual([[200, {
          action: "grant", task: "cas-1234", path: "~/soundwave-config/docs/requests", mode: "create+edit", reason: "INGEST request files",
        }]]);
      });
      await journey.stage("Revoke it after confirming", async () => {
        await panel.getByRole("button", { name: "Revoke…" }).click();
        const confirm = rail.getByRole("alertdialog", { name: "Confirm revoke" });
        await expect(confirm).toBeVisible();
        await page.keyboard.press("Escape");
        await expect(confirm).toHaveCount(0);
        await expect(panel.getByRole("button", { name: "Revoke…" })).toBeFocused();
        await page.keyboard.press("Enter");
        await expect(confirm).toContainText("cas-1234");
        await confirm.getByRole("button", { name: "Revoke", exact: true }).click();
        await expect(grantResult(page)).toContainText("cas-1234");
      });
    };
    if (part) test(`HUB-J20 write access ${width} ${colorScheme} cas_ab04`, journeyPart, body);
    else test(`HUB-J20 write access ${width} ${colorScheme} cas_ab04`, body);
  });
}

test.describe("write access without factory:manage cas_ab04", () => {
  test.use({ viewport: { width: 1280, height: 844 }, colorScheme: "light" });
  test("HUB-J20 a pairing without factory:manage cannot grant", journeyPart, async ({ page, journey }) => {
    const hub = await journey.hub({ machines: [fleetMachine(ATLAS)], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-operate"] }, fleet: { [PELICAN]: fleet() } });
    await journey.stage("A pairing without factory:manage", async () => {
      await journey.open();
      await conversationRow(page).click();
      await expect(statusView(page).getByRole("button", { name: "Write access…" })).toHaveCount(0);
      // The Permissions disclosure is a <details> (role group) opened from its summary.
      const summary = page.getByText(/^(View only|Limited access) · Permissions$/);
      const permissions = statusView(page).getByRole("group").filter({ has: summary });
      await permissions.getByText(/^(View only|Limited access) · Permissions$/).focus(); await page.keyboard.press("Enter");
      await expect(permissions).toBeVisible();
      expect(hub.writeGrants).toHaveLength(0);
      await permissions.getByRole("link", { name: "Open Paired machines" }).click();
      await expect(page.getByRole("dialog", { name: "Paired machines", exact: true })).toBeVisible();
    });
  });
});
