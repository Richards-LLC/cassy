import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

const at = (hoursAgo: number) => new Date(Date.now() - hoursAgo * 3_600_000).toISOString();
const you = (id: number, text: string, hoursAgo: number) => ({ notification_id: id, target: PELICAN, text, state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Daniel", at: at(hoursAgo) });
const sup = (id: number, replyTo: number, message: string, hoursAgo: number) => ({ notification_id: id, reply_to: replyTo, message, summary: "", device_id: "journey-device", kind: "answer", attachments: [], at: at(hoursAgo) });

test("HUB-J4 read the conversation history", async ({ page, journey }) => {
  const hub = await journey.hub({
    machines: [ATLAS],
    paired: ["atlas"],
    history: {
      [PELICAN]: [
        { has_earlier: true, next_before: 20, messages: [you(21, "Is the release ready to cut?", 2)], replies: [sup(22, 21, "Yes. The gate is green on the release branch.", 1.9)] },
        { has_earlier: false, messages: [you(11, "Start the QA epic tomorrow morning.", 30)], replies: [sup(12, 11, "Scheduled for 09:00 with three workers.", 29.9)] },
      ],
    },
  });
  const log = page.getByRole("log");

  await journey.stage("Open the conversation and see the recent turns", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(log.getByText("Yes. The gate is green on the release branch.")).toBeVisible();
    await expect(log.getByText("Is the release ready to cut?")).toBeVisible();
  });

  await journey.stage("Load earlier turns", async () => {
    await page.getByRole("button", { name: "Load earlier" }).click();
    await expect(log.getByText("Scheduled for 09:00 with three workers.")).toBeVisible();
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 20 });
  });

  await journey.stage("Reach the start of the conversation", async () => {
    await expect(page.getByText("No earlier history")).toBeVisible();
    await expect(page.getByRole("button", { name: "Load earlier" })).toBeHidden();
    await expect(log.getByText("Yesterday")).toBeVisible();
  });
});
