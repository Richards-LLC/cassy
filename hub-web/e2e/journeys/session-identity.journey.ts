import { test, expect } from "./journey";
import { journeyDay, journeyStamp } from "./clock";
import { SCOPES, type Machine } from "./hub-double";

// cas-55a4: three live gabber-studio sessions on one machine, as the
// operator's Pixel saw them on 2026-09-30. noble-cheetah-84 wrote to Commander
// yesterday and is idle; calm-puma-34 is the busy one and has never written
// to Commander; wild-shark-68 has its own short conversation.
const NOBLE = "gabber-studio-noble-cheetah-84";
const PUMA = "gabber-studio-calm-puma-34";
const SHARK = "gabber-studio-wild-shark-68";
const session = (name: string, supervisor: string, lastActivityAt: string, lastActivity: string) => ({
  name, supervisor, project_dir: "/projects/gabber-studio", workers: [], liveness: "live" as const, last_activity_at: lastActivityAt, last_activity: lastActivity,
});
const ATLAS: Machine = {
  id: "atlas",
  label: "Atlas · Linux",
  sessions: [
    session(NOBLE, "noble-cheetah-84", journeyDay(1, 21, 41), "supervisor → Commander"),
    session(PUMA, "calm-puma-34", journeyStamp(-2 * 60_000), "supervisor → bright-robin-85"),
    session(SHARK, "wild-shark-68", journeyStamp(-40 * 60_000), "Commander → supervisor"),
  ],
};

const you = (id: number, text: string, from: string, at: string) => ({ notification_id: id, target: "supervisor", text, state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Pixel 10", session: from, at });
const sup = (id: number, message: string, from: string, at: string, kind = "answer") => ({ notification_id: id, reply_to: null, message, summary: "", device_id: "journey-device", kind, attachments: [], session: from, at });

test("HUB-J14 tell a project's live sessions apart", async ({ page, journey }) => {
  const hub = await journey.hub({
    machines: [ATLAS],
    paired: ["atlas"],
    scopes: { atlas: [...SCOPES, "factory-manage"] },
    history: {
      // A daemon with session-bound history: no turns of its own, and the
      // older session's thread beside the page, never in it.
      [PUMA]: [{ has_earlier: false, messages: [], replies: [],
        earlier_messages: [you(101, "Render the mixdown preview.", NOBLE, journeyDay(1, 21, 30))],
        earlier_replies: [
          sup(102, "Mixdown preview rendered: 3 stems.", NOBLE, journeyDay(1, 21, 41)),
          sup(103, "worker died: daring-robin-43", "", journeyDay(1, 17, 20), "blocker"),
        ] }],
      [SHARK]: [{ has_earlier: false, messages: [you(201, "Status on the stem export?", SHARK, journeyStamp(-45 * 60_000))], replies: [sup(202, "Stem export is at 60%.", SHARK, journeyStamp(-40 * 60_000))] }],
      // A daemon from before cas-55a4 still sends the project-wide page:
      // the other session's turn in it is filed beside the thread.
      [NOBLE]: [{ has_earlier: false,
        messages: [you(101, "Render the mixdown preview.", NOBLE, journeyDay(1, 21, 30)), you(201, "Status on the stem export?", SHARK, journeyStamp(-45 * 60_000))],
        replies: [sup(102, "Mixdown preview rendered: 3 stems.", NOBLE, journeyDay(1, 21, 41)), sup(202, "Stem export is at 60%.", SHARK, journeyStamp(-40 * 60_000))] }],
    },
  });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const log = page.getByRole("log");
  const earlier = page.getByRole("region", { name: "Earlier sessions" });
  /** The list sits beside the thread on a desktop; a narrow layout goes back to it. */
  const backToList = async () => { const back = page.locator("#conversation-back"); if (await back.isVisible()) await back.click(); };
  const row = (codename: string) => list.locator(".conversation-row", { hasText: codename });

  await journey.stage("See a project's live sessions together", async () => {
    await journey.open();
    await expect(list.locator(".conversation-group-head")).toHaveText("gabber-studio · 3 sessions on Atlas");
    const rows = list.locator(".conversation-row");
    await expect(rows.locator(".conversation-supervisor")).toHaveText(["calm-puma-34Most recent", "wild-shark-68", "noble-cheetah-84"]);
    await expect(row("calm-puma-34")).toHaveAttribute("data-most-recent", "true");
    // Each row's time is its own session's last activity, not one shared time.
    await expect(rows.locator(".conversation-when")).toHaveText(["2m", "40m", /^\d+h$/]);
    await expect(row("calm-puma-34").locator(".conversation-when")).toHaveAttribute("title", "Last activity 2m ago · supervisor → bright-robin-85");
  });

  await journey.stage("Open a session that has not written yet", async () => {
    await row("calm-puma-34").click();
    const empty = page.locator(".thread .empty");
    await expect(empty.locator(".said")).toHaveText("No Commander messages from this session yet. The supervisor (calm-puma-34) will write here when it needs a decision.");
    await expect(empty.locator(".empty-activity")).toHaveText("Last activity 2m ago · supervisor → bright-robin-85");
    await expect(page.locator(".thread .msgs")).not.toContainText("Mixdown preview rendered");
    await expect(page.locator(".pinned-ask")).toBeHidden();
    // The older session's thread is a collapsed, labelled section, with dates.
    await expect(earlier.locator("summary")).toHaveText([/^Earlier session noble-cheetah-84, Yesterday/, /^Earlier messages with no session recorded, Yesterday/]);
    await expect(earlier.locator("details[open]")).toHaveCount(0);
  });

  await journey.stage("Read an earlier session's messages", async () => {
    const noble = earlier.locator('details[data-session="gabber-studio-noble-cheetah-84"]');
    await noble.locator("summary").click();
    await expect(noble.locator(".earlier-turn")).toHaveCount(2);
    await expect(noble.locator(".earlier-turn").nth(1)).toContainText("Mixdown preview rendered: 3 stems.");
    await expect(noble.locator(".earlier-turn time").nth(1)).toHaveText("Sep 29, 21:41");
    await expect(noble.getByRole("button")).toHaveCount(0);
    await noble.locator("summary").click();
  });

  await journey.stage("Open the Terminal from the empty session", async () => {
    await page.locator(".thread .empty").getByRole("button", { name: "Open Terminal" }).click();
    await expect(page.locator("#conversation-return")).toBeVisible();
    await page.locator("#conversation-return").click();
    await expect(page.locator(".thread .empty .said")).toBeVisible();
  });

  await journey.stage("Each session shows its own conversation", async () => {
    await backToList();
    await row("wild-shark-68").click();
    await expect(log).toContainText("Stem export is at 60%.");
    await expect(log).not.toContainText("Mixdown preview rendered");
    await expect(earlier).toBeHidden();
    await backToList();
    await row("noble-cheetah-84").click();
    await expect(log).toContainText("Mixdown preview rendered: 3 stems.");
    // The old daemon's project-wide page: wild-shark-68's turns are beside the thread.
    await expect(log).not.toContainText("Stem export is at 60%.");
    await expect(earlier.locator("summary")).toHaveText([/^Earlier session wild-shark-68, Today/]);
  });

  await journey.stage("End a stale session", async () => {
    await backToList();
    const end = list.locator(".conversation-end").nth(2);
    await end.getByRole("button", { name: "End session noble-cheetah-84 on Atlas" }).click();
    await expect(end.locator(".conversation-end-question")).toHaveText("End noble-cheetah-84 on Atlas? Its supervisor and workers stop.");
    expect(hub.ends).toEqual([]);
    await end.getByRole("button", { name: "End session", exact: true }).click();
    await expect(row("noble-cheetah-84")).toHaveCount(0);
    expect(hub.ends).toEqual([{ machine: "atlas", session: NOBLE, scopes: [...SCOPES, "factory-manage"] }]);
    await expect(list.locator(".conversation-group-head")).toHaveText("gabber-studio · 2 sessions on Atlas");
  });
});
