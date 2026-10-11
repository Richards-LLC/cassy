import { test, expect, journeyPart } from "./journey";
import { SCOPES, type FleetWorld, type Machine } from "./hub-double";
import { ATLAS, PELICAN } from "./world";

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
      const rail = page.locator("#status-view");
      const panel = rail.locator(".write-grant");
      const announcer = page.locator("#fleet-ops-announcer");
      await journey.stage("Open the fleet", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        if (width === 390) await page.getByRole("button", { name: "Tasks & progress", exact: true }).click();
        await expect(rail.getByRole("button", { name: "Write access…" })).toBeVisible();
      });
      await journey.stage("Open Write access on the current task", async () => {
        await rail.getByRole("button", { name: "Write access…" }).click();
        await expect(panel).toBeVisible();
        await expect(panel.getByRole("combobox", { name: "Task" })).toHaveValue("cas-1234");
        await expect(panel.getByRole("checkbox", { name: "create" })).toBeChecked();
        await expect(panel.getByRole("checkbox", { name: "edit" })).toBeChecked();
        await expect(panel.getByRole("checkbox", { name: "delete" })).not.toBeChecked();
      });
      await journey.stage("An incomplete grant says what is missing", async () => {
        await panel.getByRole("button", { name: "Review grant" }).click();
        await expect(rail.locator(".write-grant-result")).toHaveText("Enter the folder to grant, as an absolute path or ~/…");
        expect(hub.writeGrants).toHaveLength(0);
      });
      await journey.stage("Review, confirm and see the receipt", async () => {
        await panel.getByRole("textbox", { name: "Folder" }).fill("~/soundwave-config/docs/requests");
        await panel.getByRole("textbox", { name: "Reason" }).fill("INGEST request files");
        await panel.getByRole("button", { name: "Review grant" }).click();
        const confirm = rail.getByRole("alertdialog", { name: "Confirm write access" });
        await expect(confirm).toContainText("Grant agents on cas-1234 create+edit in ~/soundwave-config/docs/requests until the task closes?");
        await expect(confirm.getByRole("button")).toHaveText(["Cancel", "Grant"]);
        await confirm.getByRole("button", { name: "Grant", exact: true }).click();
        const receipt = "Write access granted for cas-1234: /home/operator/soundwave-config/docs/requests (create+edit) until the task closes.";
        await expect(rail.locator(".write-grant-result")).toHaveText(receipt);
        await expect(announcer).toHaveText(receipt);
        expect(hub.writeGrants.map((call: any) => [call.status, call.body])).toEqual([[200, {
          action: "grant", task: "cas-1234", path: "~/soundwave-config/docs/requests", mode: "create+edit", reason: "INGEST request files",
        }]]);
      });
      await journey.stage("Revoke it after confirming", async () => {
        await panel.getByRole("button", { name: "Revoke…" }).click();
        const confirm = rail.getByRole("alertdialog", { name: "Confirm revoke" });
        await expect(confirm).toContainText("Revoke every write grant for cas-1234?");
        await confirm.getByRole("button", { name: "Revoke", exact: true }).click();
        await expect(rail.locator(".write-grant-result")).toHaveText("Write access revoked for cas-1234: 1 grant removed.");
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
      await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
      const grant = page.locator("#status-view").getByRole("button", { name: "Write access…" });
      await expect(grant).toHaveAttribute("aria-disabled", "true");
      // A disabled control still says why; activating it does nothing.
      await grant.focus();
      await page.keyboard.press("Enter");
      await expect(page.locator("#status-view .write-grant")).toHaveCount(0);
      await expect(page.locator("#fleet-reason-header-grant")).toContainText("Not allowed on this pairing");
      expect(hub.writeGrants).toHaveLength(0);
    });
  });
});
