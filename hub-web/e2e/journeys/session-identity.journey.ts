import { test, expect, journeyPart } from "./journey";
import { mkdir } from "node:fs/promises";
import { journeyDay, journeyNow, journeyStamp } from "./clock";
import { SCOPES, type Machine } from "./hub-double";
import { showConversationList } from "./responsive-goals";
import type { Locator, Page } from "@playwright/test";

const conversationList = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" });
// A row is named "<project> on <machine>, …"; the row's own controls are not.
const projectRows = (page: Page, project: string) => conversationList(page).getByRole("button", { name: new RegExp(`^${project} on `) });
const connection = (page: Page) => page.getByRole("status", { name: "Connection", exact: true, includeHidden: true });
const composer = (page: Page) => page.getByRole("textbox", { name: "Your message", exact: true });
const sendButton = (page: Page) => page.getByRole("button", { name: /^Send to the .+ supervisor$/ });
const turn = (page: Page, text: string) => page.getByRole("log").getByTestId("conversation-turn").filter({ hasText: text });
// A held send's waiting line, by its test id.
const heldLine = (page: Page) => page.getByRole("log").getByTestId("conversation-held");
const backButton = (page: Page) => page.getByRole("button", { name: "‹ Conversations", exact: true });
const heading = (page: Page) => page.getByTestId("conversation-heading");
const rawOutput = (page: Page) => heading(page).getByRole("button", { name: "Raw output", exact: true });
const interruptButton = (page: Page) => heading(page).getByRole("button", { name: "Interrupt the gabber-studio supervisor", exact: true });
const emptyCard = (page: Page) => page.getByTestId("conversation-thread").getByTestId("empty");

for (const identity of ["resume", "replacement", "unknown"] as const) {
  test(`HUB-J14 held messages and drafts respect ${identity} session identity`, journeyPart, async ({ page, journey }) => {
    const machine: Machine = { id: "atlas", label: "Atlas", sessions: [{ name: "birth-test", project_dir: "/projects/cas-src", supervisor: "supervisor", workers: [], liveness: "live", started_at: journeyStamp(-60000) }] };
    const hub = await journey.hub({ machines: [machine], paired: [machine.id], multiplex: true });
    if (identity === "unknown") delete machine.sessions[0]!.started_at;
    await journey.open(); await showConversationList(page);
    await projectRows(page, "cas-src").click();
    await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
    if (identity === "unknown") {
      await composer(page).fill("Explicit send to the current live attach");
      await page.getByRole("button", { name: /^Send to the .+ supervisor$/ }).click();
      await expect.poll(() => hub.sends.length).toBe(1);
    }
    await hub.down(machine.id, { sockets: "close" });
    await expect(connection(page)).toHaveAttribute("data-phase", /^(?:backoff|resolving|dialing|auth|attaching|failed)$/);
    await composer(page).fill("Held for the original session");
    await page.getByRole("button", { name: /^Send to the .+ supervisor$/ }).click();
    await expect(heldLine(page)).toHaveCount(1);
    await composer(page).fill("Draft for the original session");
    if (identity === "replacement") machine.sessions[0]!.started_at = journeyStamp();
    await hub.up(machine.id);
    await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
    await journey.stage("Only the same session birth can keep its words", async () => {
      if (identity === "replacement") {
        await expect(composer(page)).toHaveValue("");
        expect(hub.sends).toHaveLength(0);
        await showConversationList(page);
        const saved = page.getByRole("region", { name: "Saved messages" });
        await expect(saved).toBeVisible();

        for (const summary of await saved.getByText("Details", { exact: true }).all()) await summary.click();
        await expect(saved).toContainText("Held for the original session");
        await expect(saved).toContainText("Draft for the original session");
      } else if (identity === "resume") {
        await expect.poll(() => hub.sends.map(send => send.text)).toEqual(["Held for the original session"]);
        await expect(composer(page)).toHaveValue("Draft for the original session");
      } else {
        expect(hub.sends.map(send => send.text)).toEqual(["Explicit send to the current live attach"]);
        await expect(heldLine(page)).toHaveCount(1);
        await expect(composer(page)).toHaveValue("Draft for the original session");
      }
    });
  });
}

for (const order of ["catalog-first", "journal-first"] as const) {
test(`HUB-J14 a stopped catalog ${order === "catalog-first" ? "before" : "after"} journal restoration still exposes the saved message`, journeyPart, async ({ page, journey }) => {
  const machine: Machine = { id: "atlas", label: "Atlas", sessions: [{ name: "restore-test", project_dir: "/projects/cas-src", supervisor: "supervisor", workers: [], liveness: "live", started_at: journeyStamp(-60000) }] };
  const hub = await journey.hub({ machines: [machine], paired: [machine.id], multiplex: true });
  await journey.open(); await showConversationList(page); await projectRows(page, "cas-src").click();
  await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
  await hub.down(machine.id, { sockets: "close" });
  await expect(connection(page)).toHaveAttribute("data-phase", /^(?:backoff|resolving|dialing|auth|attaching|failed)$/);
  await composer(page).fill("Restore before retiring my words");
  await page.getByRole("button", { name: /^Send to the .+ supervisor$/ }).click();
  await expect(heldLine(page)).toHaveCount(1);
  machine.sessions[0]!.liveness = "stale_metadata";
  await page.addInitScript((order) => {
    // A cold catalog makes the initial empty list observable before the reply.
    if (order === "catalog-first") localStorage.removeItem("cas-commander-conversation:catalog:v1");
    const fetch = window.fetch;
    let releaseCatalog!: () => void;
    const catalogReady = new Promise<void>(resolve => { releaseCatalog = resolve; });
    (window as unknown as { releaseCatalog: () => void }).releaseCatalog = releaseCatalog;
    window.fetch = async (...args: Parameters<typeof fetch>) => {
      const url = new URL(args[0] instanceof Request ? args[0].url : String(args[0]), location.href);
      if (url.pathname === "/v1/sessions") await catalogReady;
      return fetch(...args);
    };
    const original = IDBFactory.prototype.open;
    const pending: (() => void)[] = [];
    let released = false;
    (window as unknown as { releaseDelivery: () => void }).releaseDelivery = () => { released = true; pending.splice(0).forEach(run => run()); };
    IDBFactory.prototype.open = function(...args: Parameters<IDBFactory["open"]>) {
      const request = original.apply(this, args);
      if (args[0] === "cas-commander-delivery-v1") {
        const setter = Object.getOwnPropertyDescriptor(IDBRequest.prototype, "onsuccess")!.set!;
        Object.defineProperty(request, "onsuccess", { set(callback) {
          setter.call(request, (event: Event) => { const run = () => callback.call(request, event); if (released) run(); else pending.push(run); });
        } });
      }
      return request;
    };
  }, order);
  await hub.up(machine.id); await page.reload();
  if (order === "journal-first") {
    await page.evaluate(() => (window as unknown as { releaseDelivery: () => void }).releaseDelivery());
    // The cached conversation proves restoration finished before the catalog.
    await expect(heldLine(page)).toHaveCount(1);
    await showConversationList(page);
  }
  const stoppedCatalog = page.waitForResponse(response => response.url() === "https://atlas.test/v1/sessions" && response.ok());
  await page.evaluate(() => (window as unknown as { releaseCatalog: () => void }).releaseCatalog());
  await stoppedCatalog;
  // Zero rows alone also matches the initial render before a catalog arrives.
  await expect(page.getByTestId("conversation-empty")).toHaveAttribute("data-state", "text");
  await expect(projectRows(page, "cas-src")).toHaveCount(0);
  // Later polls fail: the first positive stopped response must be enough.
  await page.route("https://atlas.test/v1/sessions", route => route.abort("failed"));
  if (order === "catalog-first") await page.evaluate(() => (window as unknown as { releaseDelivery: () => void }).releaseDelivery());
  await journey.stage("Saved words survive both catalog and journal arrival orders", async () => {
    const saved = page.getByRole("region", { name: "Saved messages" });
    await expect(saved).toBeVisible();
    await saved.getByText("Details", { exact: true }).click();
    await expect(saved).toContainText("Restore before retiring my words");
    expect(hub.sends).toHaveLength(0);
  });
});
}

test("HUB-J14 a live explicit send does not depend on a catalog read", journeyPart, async ({ page, journey }) => {
  const machine: Machine = { id: "atlas", label: "Atlas", sessions: [{ name: "live-send", project_dir: "/projects/cas-src", supervisor: "supervisor", workers: [], liveness: "live", started_at: journeyStamp(-60000) }] };
  const hub = await journey.hub({ machines: [machine], paired: [machine.id], multiplex: true });
  await journey.open(); await showConversationList(page); await projectRows(page, "cas-src").click();
  await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
  await page.route("https://atlas.test/v1/sessions", route => route.abort("failed"));
  await composer(page).fill("Send through the current live attach");
  await sendButton(page).click();
  await journey.stage("A catalog failure cannot block this explicit send", async () => {
    await expect.poll(() => hub.sends.map(send => send.text)).toEqual(["Send through the current live attach"]);
  });
});

test("HUB-J14 upgrading a running session preserves a legacy draft", journeyPart, async ({ page, journey }) => {
  const machine: Machine = { id: "atlas", label: "Atlas", sessions: [{ name: "legacy", project_dir: "/projects/cas-src", supervisor: "supervisor", workers: [], liveness: "live", started_at: journeyStamp(-60000) }] };
  const hub = await journey.hub({ machines: [machine], paired: [machine.id], multiplex: true });
  await journey.open(); await showConversationList(page); await projectRows(page, "cas-src").click();
  await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
  await composer(page).fill("Words written before birth tracking");
  await page.addInitScript(() => {
    const key = "cas-commander-conversation:drafts:v1";
    const drafts = JSON.parse(localStorage.getItem(key) ?? "{}");
    for (const row of Object.values(drafts) as { value: { id?: string; sessionStartedAt?: string } }[]) { delete row.value.sessionStartedAt; delete row.value.id; }
    localStorage.setItem(key, JSON.stringify(drafts));
  });
  await page.reload();
  await journey.stage("Missing birth is unknown, not evidence of a stopped session", async () => {
    await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
    await expect(composer(page)).toHaveValue("Words written before birth tracking");
    // No notice anywhere is a stopped draft: notices carry their identity in data-attention-id.
    const noticeIds = await page.getByRole("article", { includeHidden: true }).evaluateAll(nodes => nodes.map(node => (node as HTMLElement).dataset.attentionId ?? ""));
    expect(noticeIds.filter(id => id.includes(":stopped-draft:"))).toEqual([]);
    expect(hub.sends).toHaveLength(0);
  });
});

test("HUB-J14 an old rejected send cannot settle the replacement's in-flight message", journeyPart, async ({ page, journey }) => {
  const machine: Machine = { id: "atlas", label: "Atlas", sessions: [{ name: "replacement", project_dir: "/projects/cas-src", supervisor: "supervisor", workers: [], liveness: "live", started_at: journeyStamp(-60000) }] };
  const hub = await journey.hub({ machines: [machine], paired: [machine.id], multiplex: true });
  await journey.open(); await showConversationList(page); await projectRows(page, "cas-src").click();
  await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
  await hub.down(machine.id, { sockets: "close" });
  await expect(connection(page)).toHaveAttribute("data-phase", /^(?:backoff|resolving|dialing|auth|attaching|failed)$/);
  await composer(page).fill("Old held words"); await sendButton(page).click();
  await expect(heldLine(page)).toHaveCount(1);
  machine.sessions[0]!.started_at = journeyStamp(); await hub.up(machine.id);
  await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
  await expect(turn(page, "Old held words")).toHaveAttribute("data-state", "error");
  await composer(page).fill("New send A"); await sendButton(page).click();
  await expect.poll(() => hub.sends.length).toBe(1);
  await composer(page).fill("New send B"); await sendButton(page).click();
  await journey.stage("New sends keep their own receipt state", async () => {
    await expect.poll(() => hub.sends.length).toBe(2);
    for (const text of ["New send A", "New send B"]) await expect(turn(page, text)).toHaveAttribute("data-state", "sending");
    // None of the three sends is unconfirmed.
    for (const text of ["Old held words", "New send A", "New send B"]) await expect(turn(page, text)).not.toHaveAttribute("data-state", "unconfirmed");
  });
});

test("HUB-J14 a failed stopped-draft archive retains its durable words", journeyPart, async ({ page, journey }) => {
  const machine: Machine = { id: "atlas", label: "Atlas", sessions: [{ name: "archive", project_dir: "/projects/cas-src", supervisor: "supervisor", workers: [], liveness: "live", started_at: journeyStamp(-60000) }] };
  const hub = await journey.hub({ machines: [machine], paired: [machine.id], multiplex: true });
  await journey.open(); await showConversationList(page); await projectRows(page, "cas-src").click();
  await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
  await composer(page).fill("Keep these words if storage fails");
  await page.evaluate(() => {
    const put = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function(...args: Parameters<IDBObjectStore["put"]>) {
      if (this.name === "attention") throw new DOMException("controlled archive failure", "QuotaExceededError");
      return put.apply(this, args);
    };
  });
  machine.sessions[0]!.started_at = journeyStamp(); await hub.announceCatalog(machine.id);
  await expect(page.getByRole("region", { name: "Saved messages" })).toBeVisible();
  await journey.stage("Failed archival leaves the original durable draft available after reload", async () => {
    await expect.poll(() => page.evaluate(() => localStorage.getItem("cas-commander-conversation:drafts:v1"))).toContain("Keep these words if storage fails");
    await page.reload();
    const saved = page.getByRole("region", { name: "Saved messages" });
    await expect(saved).toBeVisible(); await saved.getByText("Details", { exact: true }).click();
    await expect(saved).toContainText("Keep these words if storage fails");
    expect(hub.sends).toHaveLength(0);
  });
});

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
// Stamped per test, never at file load: a stamp taken between tests picks up
// the previous test's elapsed time and lands late (cas-4e52 mechanism).
const atlas = (): Machine => ({
  id: "atlas",
  label: "Atlas · Linux",
  sessions: [
    session(NOBLE, "noble-cheetah-84", journeyDay(1, 21, 41), "supervisor → Commander"),
    session(PUMA, "calm-puma-34", journeyStamp(-2 * 60_000), "supervisor → bright-robin-85"),
    session(SHARK, "wild-shark-68", journeyStamp(-40 * 60_000), "Commander → supervisor"),
  ],
});

const you = (id: number, text: string, from: string, at: string) => ({ notification_id: id, target: "supervisor", text, state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Pixel 10", session: from, at });
const sup = (id: number, message: string, from: string, at: string, kind = "answer") => ({ notification_id: id, reply_to: null, message, summary: "", device_id: "journey-device", kind, attachments: [], session: from, at });

// cas-6faf, cas-b6ed: minute ties hold the badge; later minutes move it in place.
test("HUB-J14 Most recent moves without shifting sibling rows (cas-6faf)", journeyPart, async ({ page, journey }) => {
  const machine = atlas();
  machine.sessions = machine.sessions.slice(0, 2);
  machine.sessions[0]!.supervisor = "thoughtful-chinchilla-84";
  const firstMinute = Math.floor(journeyNow() / 60_000) * 60_000 - 240_000;
  for (const sibling of machine.sessions) sibling.last_activity_at = new Date(firstMinute).toISOString();
  const hub = await journey.hub({ machines: [machine], paired: ["atlas"] });
  await journey.open();
  await showConversationList(page);
  const rows = projectRows(page, "gabber-studio");
  const row = (codename: string) => rows.filter({ hasText: codename });
  const rects = () => rows.evaluateAll(nodes => nodes.map(node => {
    const { x, y, width, height } = node.getBoundingClientRect();
    return { key: node.getAttribute("data-thread-key"), x, y, width, height };
  }));
  const capture = process.env.SESSION_RECENCY_CAPTURE_DIR;
  if (capture) await mkdir(capture, { recursive: true });
  for (const [index, width] of [390, 1280].entries()) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
    await expect(rows).toHaveCount(2);
    // Advance past each row's activity high-water mark, with a later minute
    // still in the past. No stage rewinds activity seen by the live list.
    const minute = firstMinute + index * 120_000;
    for (const sibling of machine.sessions) sibling.last_activity_at = new Date(minute + 1_000).toISOString();
    machine.sessions[1]!.last_activity = `supervisor → ready-${width}`;
    await hub.announceCatalog("atlas");
    await expect(row("calm-puma-34")).toHaveAccessibleName(new RegExp(`ready-${width}`));
    await journey.stage(`Alternating activity holds the badge at ${width}px`, async () => {
      const before = await rects();
      await page.evaluate(() => {
        const changes: { old: string | null; value: string | null }[] = [];
        const observer = new MutationObserver(records => {
          for (const record of records) {
            const value = (record.target as Element).getAttribute("data-most-recent");
            if (value !== record.oldValue) changes.push({ old: record.oldValue, value });
          }
        });
        observer.observe(document.getElementById("conversation-list")!, { subtree: true, attributes: true, attributeFilter: ["data-most-recent"], attributeOldValue: true });
        (window as unknown as { recencyProbe: { changes: typeof changes; observer: MutationObserver } }).recencyProbe = { changes, observer };
      });
      for (let frame = 0; frame < 8; frame++) {
        const sibling = machine.sessions[frame % 2]!;
        sibling.last_activity_at = new Date(minute + 10_000 + frame * 400).toISOString();
        sibling.last_activity = `supervisor → frame-${width}-${frame}`;
        await hub.announceCatalog("atlas");
        await expect(row(sibling.supervisor!)).toHaveAccessibleName(new RegExp(`frame-${width}-${frame}`));
      }
      const changes = await page.evaluate(() => {
        const probe = (window as unknown as { recencyProbe: { changes: unknown[]; observer: MutationObserver } }).recencyProbe;
        probe.observer.disconnect();
        return probe.changes;
      });
      expect(changes, "eight alternating catalog frames cause zero badge value changes").toEqual([]);
      await expect(row("calm-puma-34")).toHaveAccessibleName(/most recent/i);
      await expect(row("thoughtful-chinchilla-84")).not.toHaveAccessibleName(/most recent/i);
      expect(await rects()).toEqual(before);
    });
    // The Most recent row is the one marked data-most-recent="true".
    const currentKey = await rows.evaluateAll(nodes => nodes.find(node => (node as HTMLElement).dataset.mostRecent === "true")?.getAttribute("data-thread-key") ?? null);
    const next = machine.sessions.find(session => `atlas:${session.name}` !== currentKey)!;
    const nextRow = row(next.supervisor!);
    const previousRow = rows.filter({ hasText: machine.sessions.find(session => `atlas:${session.name}` === currentKey)!.supervisor! });
    const before = await rects();
    if (capture) await page.screenshot({ path: `${capture}/initial-${width}.png` });
    next.last_activity_at = new Date(minute + 60_000).toISOString();
    await hub.announceCatalog("atlas");
    await expect(nextRow).toHaveAttribute("data-most-recent", "true");
    if (capture) await page.screenshot({ path: `${capture}/moved-${width}.png` });
    await journey.stage(`Recency moves, every row stays in place at ${width}px`, async () => {
      expect(await rects()).toEqual(before);
      await expect(nextRow).toHaveAccessibleName(/most recent/i);
      await expect(previousRow).not.toHaveAccessibleName(/most recent/i);
    });
  }
});

test("HUB-J14 tell a project's live sessions apart", async ({ page, journey }) => {
  const hub = await journey.hub({
    machines: [atlas()],
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
  // Each earlier session is a disclosure; its summary leads with this label.
  const earlierLabels = earlier.getByTestId("earlier-label");
  /** The list sits beside the thread on a desktop; a narrow layout goes back to it. */
  const backToList = async () => { const back = backButton(page); if (await back.isVisible()) await back.click(); };
  const rows = projectRows(page, "gabber-studio");
  const row = (codename: string) => rows.filter({ hasText: codename });

  await journey.stage("See a project's live sessions together", async () => {
    await journey.open();
    await expect(list.getByTestId("conversation-group-head")).toContainText("3");
    await expect(list.getByRole("group", { name: "gabber-studio · 3 conversations on Atlas", exact: true }).getByRole("button", { name: /^gabber-studio on / })).toHaveCount(3);
    // cas-ade2: rows sort by supervisor name; Most recent marks the newest where it stands.
    const names = rows.getByTestId("conversation-role");
    await expect(names).toContainText(["calm-puma-34", "noble-cheetah-84", "wild-shark-68"]);
    await expect(row("calm-puma-34").getByTestId("conversation-session-mark")).toBeVisible();
    await expect(row("noble-cheetah-84").getByTestId("conversation-session-mark")).toBeHidden();
    await expect(row("calm-puma-34")).toHaveAttribute("data-most-recent", "true");
    // Each row's time is its own session's last activity, not one shared time.
    await expect(rows.getByTestId("conversation-when")).toHaveText(["2m", /^\d+h$/, "40m"]);
    await expect(row("calm-puma-34").getByTestId("conversation-when")).toHaveAttribute("title", /2m[\s\S]*bright-robin-85/);
    // cas-da546: one line a session; siblings on one machine read apart by
    // codename, and what each last did is spoken, not shown.
    await expect(rows.getByTestId("conversation-preview")).toHaveCount(0);
    await expect(row("calm-puma-34")).toHaveAccessibleName(/bright-robin-85/);
    await expect(row("noble-cheetah-84").getByLabel("1 unread")).toBeVisible();
    await expect.poll(async () => ((await page.getByTestId("hub-footer-badges").getByTestId("hub-footer-sessions").innerText()).match(/\d+/g) ?? []).map(Number)).toEqual([3]);
    // A useful incoming preview can grow the last row beyond the list's
    // available height. Keep it reachable in the list's own scroll region.
    const incoming = rows.last();
    await incoming.scrollIntoViewIfNeeded();
    await expect(incoming).toBeInViewport({ ratio: 1 });
  });

  await journey.stage("Open a session that has not written yet", async () => {
    await row("calm-puma-34").click();
    const empty = emptyCard(page);
    // cas-010f: plain words, no product codename or queue jargon. The card
    // offers no way out of the conversation (cas-0546): the header carries
    // Raw output and Interrupt.
    await expect(empty.getByRole("status")).toBeVisible();
    await expect(empty.getByTestId("empty-activity")).toContainText("2m");
    await expect(empty).not.toContainText("Commander");
    await expect(empty).not.toContainText("→");
    await expect(empty.getByRole("button")).toHaveCount(0);
    await expect(page.getByTestId("conversation-thread").getByRole("log", { includeHidden: true })).not.toContainText("Mixdown preview rendered");
    await expect(page.getByRole("region", { name: /^Waiting on you: question from / })).toBeHidden();
    // The older session's thread is a collapsed, labelled section, with dates.
    await expect(earlierLabels).toHaveText([/^Earlier session noble-cheetah-84, Yesterday/, /^Earlier messages with no session recorded, Yesterday/]);
    for (const group of await earlier.getByRole("group").all()) await expect(group).not.toHaveAttribute("open");
  });

  await journey.stage("Read an earlier session's messages", async () => {
    const noble = earlier.getByRole("group").filter({ has: page.getByTestId("earlier-label").filter({ hasText: "noble-cheetah-84" }) });
    await expect(noble).toHaveAttribute("data-session", "gabber-studio-noble-cheetah-84");
    const nobleLabel = noble.getByTestId("earlier-label");
    await nobleLabel.click();
    await expect(noble.getByRole("listitem")).toHaveCount(2);
    await expect(noble.getByRole("listitem").nth(1)).toContainText("Mixdown preview rendered: 3 stems.");
    await expect(noble.getByRole("listitem").nth(1).getByRole("time")).toHaveText("Sep 29, 9:41 PM");
    await expect(noble.getByRole("button")).toHaveCount(0);
    await nobleLabel.click();
  });

  await journey.stage("Read the empty session's raw output", async () => {
    // The session has not written to the operator, but its supervisor is
    // running: Raw output shows what its terminal shows (cas-0546).
    const raw = rawOutput(page);
    await raw.click();
    const drawer = page.getByRole("dialog", { name: "Raw output" });
    await expect(drawer).toBeVisible();
    await expect(drawer).toContainText("The supervisor is ready.");
    await page.keyboard.press("Escape");
    await expect(drawer).toBeHidden();
    await expect(raw).toBeFocused();
    await expect(emptyCard(page).getByRole("status")).toBeVisible();
  });

  await journey.stage("The empty thread follows the connection", async () => {
    // cas-010f: off the network, the card says why nothing new can arrive;
    // back on, it is the plain live copy again.
    const empty = emptyCard(page);
    const header = connection(page);
    // cas-0546: the header's Raw output and Interrupt stay in place and say
    // why they wait, to the eye (dashed, title) and to a screen reader
    // (description); pressing one explains instead of acting.
    const raw = rawOutput(page);
    const interrupt = interruptButton(page);
    for (const action of [raw, interrupt]) await expect(action).not.toHaveAttribute("aria-disabled", "true");
    await hub.down("atlas", { sockets: "close" });
    await expect(header).toHaveAttribute("data-phase", /^(?:backoff|resolving|dialing|auth|attaching|failed)$/);
    await expect(empty.getByRole("status")).toBeVisible();
    await expect(empty.getByRole("button")).toHaveCount(0);
    const unavailable = "Lost connection to Atlas · Linux. Interrupt and raw output return when it reconnects.";
    for (const action of [raw, interrupt]) {
      await expect(action).toHaveAttribute("aria-disabled", "true");
      await expect(action).toHaveAccessibleDescription(unavailable);
      await expect(action).toHaveAttribute("title", unavailable);
    }
    await expect(raw).toHaveAccessibleName(/\S/);
    await expect(interrupt).toHaveAccessibleName(/\S/);
    await raw.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("status", { name: "Notice" })).toHaveText(unavailable);
    await expect(page.getByRole("dialog", { name: "Raw output" })).toBeHidden();
    await expect(empty.getByRole("status")).toBeVisible();
    await hub.up("atlas");
    await expect(header).toContainText("Live", { timeout: 20_000 });
    await expect(empty.getByRole("status")).toBeVisible();
    for (const action of [raw, interrupt]) {
      await expect(action).not.toHaveAttribute("aria-disabled", "true");
      await expect(action).toHaveAccessibleDescription("");
    }
  });

  await journey.stage("On a phone, the header's Raw output and Interrupt are full-size targets", async () => {
    // cas-6b75 (F01), carried to the header actions (cas-0546): at 390 each
    // is at least 44 px each way, the coarse-pointer minimum, and Raw output
    // still opens.
    const viewport = page.viewportSize()!;
    await page.setViewportSize({ width: 390, height: 844 });
    for (const action of [rawOutput(page), interruptButton(page)]) {
      await expect(action).toBeVisible();
      const box = (await action.boundingBox())!;
      expect(box.height, "at least 44 px tall").toBeGreaterThanOrEqual(44);
      expect(box.width, "at least 44 px wide").toBeGreaterThanOrEqual(44);
    }
    await rawOutput(page).click();
    const drawer = page.getByRole("dialog", { name: "Raw output" });
    await expect(drawer).toBeVisible();
    await drawer.getByRole("button", { name: "Close raw output" }).click();
    await expect(drawer).toBeHidden();
    await expect(emptyCard(page).getByRole("status")).toBeVisible();
    await page.setViewportSize(viewport);
  });

  await journey.stage("Each session shows its own conversation", async () => {
    await backToList();
    // cas-010f: a conversation with history never claims, even for a frame,
    // that it has no messages while its first page is on its way.
    await page.evaluate(() => {
      const claims: string[] = [];
      (window as unknown as { emptyClaims: string[] }).emptyClaims = claims;
      // Only wild-shark-68's own card counts: calm-puma-34's honest empty card
      // can still be on screen for a frame while the switch mounts the next thread.
      const check = () => {
        for (const card of document.querySelectorAll<HTMLElement>(".thread .empty:not([hidden])")) {
          if (!card.querySelector(".proj2")?.textContent?.includes("wild-shark-68")) continue;
          const said = card.querySelector(".said")?.textContent ?? "";
          if (/^No (Commander )?messages/.test(said)) claims.push(said);
        }
      };
      new MutationObserver(check).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ["hidden"] });
    });
    await row("wild-shark-68").click();
    await expect(log).toContainText("Stem export is at 60%.");
    expect(await page.evaluate(() => (window as unknown as { emptyClaims: string[] }).emptyClaims), "no empty claim while history loads").toEqual([]);
    await expect(log).not.toContainText("Mixdown preview rendered");
    await expect(earlier).toBeHidden();
    await backToList();
    // cas-5d2c: a lower row keeps its place when it opens; the list does
    // not jump back to its top and leave it below the fold.
    await row("noble-cheetah-84").scrollIntoViewIfNeeded();
    await row("noble-cheetah-84").click();
    await expect(row("noble-cheetah-84")).toHaveAttribute("aria-current", "true");
    await expect(row("noble-cheetah-84")).toBeInViewport({ ratio: 1 });
    await expect(log).toContainText("Mixdown preview rendered: 3 stems.");
    // The old daemon's project-wide page: wild-shark-68's turns are beside the thread.
    await expect(log).not.toContainText("Stem export is at 60%.");
    await expect(earlierLabels).toHaveText([/^Earlier session wild-shark-68, Today/]);
  });

  await journey.stage("End a stale session", async () => {
    await backToList();
    // cas-ade2: noble-cheetah-84 is the second row by supervisor name.
    const end = list.getByTestId("conversation-end").nth(1);
    // cas-f60a: a double-click on End session opens the confirmation and
    // nothing more. Its destructive button is never where the pointer was.
    const ask = end.getByRole("button", { name: "End session noble-cheetah-84 on Atlas" });
    const box = (await ask.boundingBox())!;
    const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    await ask.dblclick();
    await expect(end.getByTestId("conversation-end-question")).toContainText("noble-cheetah-84");
    expect(hub.ends).toEqual([]);
    // cas-488f: ask the hit test a yes/no question. Whatever is under the
    // pointer (Cancel, the question, or nothing), it must not be End session.
    const underPointer = await page.evaluate(({ x, y }) => {
      const hit = document.elementFromPoint(x, y);
      return { hit: hit ? `${hit.tagName.toLowerCase()}.${hit.className}` : "nothing", confirm: Boolean(hit?.closest(".conversation-end-confirm")) };
    }, point);
    expect(underPointer.confirm, `the confirm is not under the pointer (hit: ${underPointer.hit})`).toBe(false);
    // Cancel comes first and End session's confirmation directly after it.
    await expect(end.getByRole("button")).toHaveText(["Cancel", "End session"]);
    // cas-d6bf: a lower row's confirmation is in view and focused, and the list stays whole.
    await expect(end.getByRole("button", { name: "Cancel" })).toBeFocused();
    await expect(end.getByRole("button", { name: "End session", exact: true })).toBeInViewport({ ratio: 1 });
    await expect(rows).toHaveCount(3);
    expect(hub.ends).toEqual([]);
    await end.getByRole("button", { name: "End session", exact: true }).click();
    await expect(row("noble-cheetah-84")).toHaveCount(0);
    expect(hub.ends).toEqual([{ machine: "atlas", session: NOBLE, scopes: [...SCOPES, "factory-manage"] }]);
    await expect(list.getByTestId("conversation-group-head")).toContainText("2");
    // cas-f60a: the list says it ended where the row was, and so does a polite live region.
    await expect(list.getByTestId("conversation-ended")).toContainText("noble-cheetah-84");
    await expect(page.getByRole("status", { name: "Session ended" })).toContainText("noble-cheetah-84");
  });
});

// cas-f50f: on a phone the list and the thread take turns. Back from a lower
// row's conversation returns the list to where it was, with that row in view
// and focused, not the list's top with focus on its first row.
test.describe("on a phone", () => {
  test.use({ viewport: { width: 390, height: 600 }, hasTouch: true, isMobile: true });
  test("HUB-J14 Back from a lower grouped row returns the list to that row (cas-f50f)", journeyPart, async ({ page, journey }) => {
    // Enough live sessions in one project that a phone's list scrolls.
    const crowded = atlas();
    for (const [index, name] of ["amber-heron-11", "brisk-lynx-22", "copper-wren-33", "dusky-otter-44", "eager-finch-55", "fable-moth-66"].entries()) {
      crowded.sessions!.push(session(`gabber-studio-${name}`, name, journeyStamp(-(5 + index) * 60_000), "supervisor → Commander"));
    }
    await journey.hub({ machines: [crowded], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-manage"] } });
    const list = page.getByRole("navigation", { name: "Choose a supervisor" });
    const scroller = list;
    const rows = projectRows(page, "gabber-studio");
    const row = (codename: string) => rows.filter({ hasText: codename });
    await journey.stage("Open the lowest grouped row from a list scrolled down to it", async () => {
      await journey.open();
      await expect(rows).toHaveCount(9);
      await row("noble-cheetah-84").scrollIntoViewIfNeeded();
      const scrolled = await scroller.evaluate((node) => node.scrollTop);
      expect(scrolled, "the list scrolls on a phone, so its place matters").toBeGreaterThan(0);
      await row("noble-cheetah-84").tap();
      await expect(backButton(page)).toBeVisible();
    });
    await journey.stage("Back returns to that row, in view and focused", async () => {
      await backButton(page).tap();
      await expect(row("noble-cheetah-84")).toBeFocused();
      await expect(row("noble-cheetah-84")).toBeInViewport({ ratio: 1 });
      expect(await scroller.evaluate((node) => node.scrollTop), "the list did not jump back to its top").toBeGreaterThan(0);
    });
  });
});

test("HUB-J14 a project group ends before the next project's conversation (cas-6024)", journeyPart, async ({ page, journey }) => {
  const machine = atlas();
  machine.sessions = [...machine.sessions!.slice(0, 2), {
    name: "pulse-card-solo", supervisor: "solo-otter-12", project_dir: "/projects/pulse-card",
    workers: [], liveness: "live", last_activity_at: journeyStamp(-60_000), last_activity: "supervisor → Commander",
  }];
  await journey.hub({ machines: [machine], paired: ["atlas"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const rows = list.getByRole("button", { name: /^(gabber-studio|pulse-card) on / });
  const groupRows = (project: string) => list.getByRole("group", { name: new RegExp(`^${project} · `) }).getByRole("button", { name: new RegExp(`^${project} on `) });
  await journey.stage("A bounded project sits above an independent project on desktop and phone", async () => {
    await journey.open();
    for (const width of [1280, 390]) for (const scheme of ["light", "dark", "forced-colors"] as const) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await page.emulateMedia({ colorScheme: scheme === "dark" ? "dark" : "light", forcedColors: scheme === "forced-colors" ? "active" : "none" });
      // Cards sort by project name, so gabber-studio's card is the first.
      await expect(list.getByRole("group", { name: "gabber-studio · 2 conversations on Atlas", exact: true }).getByText("gabber-studio", { exact: true })).toBeVisible();
      await expect(rows).toHaveCount(3);
      if (process.env.GROUP_BOUNDARY_CAPTURE_DIR) await page.screenshot({ path: `${process.env.GROUP_BOUNDARY_CAPTURE_DIR}/${width}-${scheme}.png` });
    }
    const group = list.getByRole("group", { name: "gabber-studio · 2 conversations on Atlas", exact: true });
    await expect(groupRows("gabber-studio")).toHaveCount(2);
    await expect(list.getByRole("group", { name: "pulse-card · 1 conversation on Atlas", exact: true }).getByRole("button", { name: /^pulse-card on / })).toHaveCount(1);
    // Only the Most recent row shows its mark; the others' slots are hidden.
    await expect(group.getByTestId("conversation-session-mark").filter({ visible: true })).toHaveCount(1);
    await groupRows("gabber-studio").first().focus();
    await page.keyboard.press("Tab");
    await expect(groupRows("gabber-studio").last()).toBeFocused();
    const singleton = list.getByRole("group", { name: "pulse-card · 1 conversation on Atlas", exact: true });
    await singleton.getByRole("button", { name: /^pulse-card\s+1 conversation$/ }).focus();
    await page.keyboard.press("Enter");
    await expect(singleton.getByRole("button", { name: /^pulse-card\s+1 conversation$/ })).toHaveAttribute("aria-expanded", "false");
    await page.keyboard.press("Enter");
    await singleton.getByRole("button", { name: /^pulse-card on / }).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: "Send to the pulse-card supervisor", exact: true })).toBeVisible();
    // Exactly one row is the current conversation: the pulse-card one just
    // opened. A phone hides the list behind the thread, so hidden rows count.
    const listed = page.getByRole("navigation", { name: "Choose a supervisor", includeHidden: true });
    await expect(listed.getByRole("button", { name: /^pulse-card on /, includeHidden: true })).toHaveAttribute("aria-current", "true");
    for (const other of await listed.getByRole("button", { name: /^gabber-studio on /, includeHidden: true }).all()) await expect(other).toHaveAttribute("aria-current", "false");
  });
});

test("HUB-J14 project navigation spans machines, decisions and device collapse (cas-6024)", journeyPart, async ({ page, journey }) => {
  const machine = atlas();
  const endedSession = (name: string, offset: number) => {
    const at = journeyStamp(offset);
    return { name, started_at: journeyStamp(offset - 60_000), supervisor: name, project: "pulse-card", project_dir: "/projects/pulse-card", machine_online: false, workers: [], liveness: "stale_metadata" as const, last_activity: "Session ended", last_activity_at: at, ended: { at, reason: "prompt_input_exit", workers_stopped: [], orphans: [] } };
  };
  machine.sessions = [{ ...machine.sessions[1]!, project: "github.com/acme/studio", project_name: "Studio", harness: "codex", account_email: "operator@example.test", machine_online: true, workers: ["worker-one"], last_activity: "supervisor → Commander", last_activity_at: journeyStamp(-30_000) }, {
    name: "golden-jaguar-44", supervisor: "ready-cobra-72", project: "pulse-card", project_dir: "/projects/pulse-card", machine_online: true, workers: [], liveness: "live", last_activity: "supervisor → Commander", last_activity_at: journeyStamp(-30_000),
  }, endedSession("watchful-heron-16", -3_600_000), endedSession("solid-parrot-40", -2 * 3_600_000), endedSession("patient-heron-27", -3 * 3_600_000)];
  const other: Machine = { id: "borealis", label: "Borealis · macOS", sessions: [{ ...session(SHARK, "wild-shark-68", journeyStamp(-60_000), "supervisor → Commander"), project_dir: "/other/studio", project: "github.com/acme/studio", project_name: "Studio", harness: "claude", machine_online: true }] };
  const hub = await journey.hub({ machines: [machine, other], paired: ["atlas", "borealis"], history: {
    [PUMA]: [{ has_earlier: false, messages: [], replies: [sup(701, "Which mix should I export?", PUMA, journeyStamp(-30_000), "ask")] }],
  } });
  const list = conversationList(page);
  const filters = page.getByRole("group", { name: "Filter conversations", exact: true });
  const studio = () => list.getByRole("group", { name: /^Studio ·/ });
  const studioRows = () => studio().getByRole("button", { name: /^Studio on / });
  const studioToggle = () => studio().getByRole("button", { name: /^Studio\s+\d+ conversations?$/ });
  const pulse = () => list.getByRole("group", { name: /^pulse-card ·/ });
  const pulseRows = () => pulse().getByRole("button", { name: /^pulse-card on / });
  const allRows = () => list.getByRole("button", { name: /^(Studio|pulse-card) on / });
  const machineChip = (card: Locator, name: string, state: "online" | "offline") => card.getByRole("button", { name: `Filter conversations on ${name}, ${state}`, exact: true });
  const back = async () => { if (await backButton(page).isVisible()) await backButton(page).click(); };
  // A machine that comes back is settled when its open conversation is live and
  // its catalog says online again; assert states only after both.
  const settled = async (card: Locator) => {
    await expect(connection(page)).toHaveAttribute("data-phase", "live");
    await expect(connection(page)).toHaveAttribute("data-degraded", "false");
    await expect(machineChip(card, "Atlas", "online")).toBeVisible();
  };

  await journey.stage("A project contains supervisors on both machines and pins its decision", async () => {
    await journey.open();
    await expect(studioRows()).toHaveCount(2);
    // cas-6f79: the card reads the project's display name; its identity stays a detail.
    await expect(studio().getByTestId("project-card-name")).toHaveText("Studio");
    await expect(studio().getByTestId("project-card-name")).toHaveAttribute("title", "github.com/acme/studio");
    await expect(machineChip(studio(), "Atlas", "online")).toBeVisible();
    await expect(machineChip(studio(), "Borealis", "online")).toBeVisible();
    await expect(studio().getByRole("button", { name: /^Studio on .*operator@example\.test/ })).toHaveCount(1);
    // DESIGN.md Avatars: each provider's mark at a legible 24px, not a glyph.
    for (const [harness, provider] of [["codex", "OpenAI"], ["claude", "Claude"]]) {
      const avatar = studio().getByRole("button", { name: new RegExp(`^Studio on [^,]+, [^,]+, ${harness},`) }).getByTestId("conversation-avatar");
      await expect(avatar).toHaveAttribute("data-harness", harness!);
      await expect(avatar).toHaveAttribute("title", provider!);
      expect(await avatar.evaluate(node => node.querySelector("svg.provider-mark")?.getBoundingClientRect().width ?? 0)).toBeGreaterThanOrEqual(20);
    }
    await expect(studio().getByText(/\b1 worker\b/)).toBeVisible();
    await expect(pulse().getByTestId("conversation-avatar")).not.toHaveAttribute("data-harness");
    await expect(pulse().getByText("no workers")).toBeVisible();
    const supervisorRow = pulseRows();
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await expect(supervisorRow.getByTestId("conversation-role")).toHaveText("ready-cobra-72");
      await expect(supervisorRow.getByTestId("conversation-machine-name")).toHaveText("Atlas");
      expect(await supervisorRow.evaluate(node => {
        const who = node.querySelector<HTMLElement>(".conversation-who")!;
        return who.scrollWidth <= who.clientWidth;
      })).toBe(true);
      // cas-be27: every card row, provider-mark avatars included, starts its name clear of the avatar.
      for (const row of await allRows().all()) {
        const role = (await row.getByTestId("conversation-role").boundingBox())!;
        const avatar = (await row.getByTestId("conversation-avatar").boundingBox())!;
        expect(role.x - (avatar.x + avatar.width)).toBeGreaterThanOrEqual(0);
      }
    }
    await page.setViewportSize({ width: 1280, height: 800 });
    const pulseMachine = machineChip(pulse(), "Atlas", "online");
    await expect(pulseMachine).toBeVisible();
    expect(await pulseMachine.evaluate((chip) => getComputedStyle(chip).borderRadius)).toBe(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--radius-pill").trim()));
    await expect(supervisorRow.getByRole("img", { name: "Offline", exact: true })).toHaveCount(0);
    await expect(pulseRows()).toHaveCount(1);
    await expect(list.getByRole("button", { name: /Dismiss ended|Clear ended/ })).toHaveCount(0);
  });

  for (const mode of ["light", "dark", "forced-colors"] as const) {
    await journey.stage(`Running idle stays green in ${mode}`, async () => {
      await page.emulateMedia({ colorScheme: mode === "dark" ? "dark" : "light", forcedColors: mode === "forced-colors" ? "active" : "none" });
      const idle = pulseRows().getByRole("img", { name: "Idle", exact: true });
      await pulseRows().click();
      await expect(page.locator("html")).toHaveAttribute("data-scheme", mode === "dark" ? "dark" : "light");
      await expect(idle).toHaveAttribute("title", "Idle");
      const look = (dot: typeof idle) => dot.evaluate(node => { const s = getComputedStyle(node); return [s.backgroundColor, s.outlineColor, s.outlineWidth, s.borderRadius]; });
      const idleLook = await look(idle);
      if (mode !== "forced-colors") {
        const idleFill = await idle.evaluate(node => {
          const probe = document.body.appendChild(document.createElement("span"));
          probe.style.background = "var(--state-idle)";
          const fill = getComputedStyle(probe).backgroundColor;
          probe.remove();
          return fill;
        });
        expect(idleLook[0]).toBe(idleFill);
      }
      // Gray belongs to an unreachable machine's last-known running row.
      await hub.down("atlas", { sockets: "close" });
      const offline = pulseRows().getByRole("img", { name: "Offline", exact: true });
      await expect(offline).toHaveAttribute("title", "Offline");
      expect(await look(offline)).not.toEqual(idleLook);
      await hub.up("atlas");
      // The row's dot follows the machine, so read it once the machine is back.
      await settled(pulse());
      await expect(idle).toHaveAttribute("title", "Idle");
    });
  }
  await page.emulateMedia({ colorScheme: "light", forcedColors: "none" });

  await journey.stage("Stopped sessions stay hidden on phone and desktop, and a running replacement returns (cas-f8ab)", async () => {
    for (const width of [390, 1280]) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await back();
      await expect(pulseRows()).toHaveCount(1);
      await expect(list.getByRole("button", { name: /Dismiss ended|Clear ended/ })).toHaveCount(0);
    }
    // The metadata and history are retained; navigation is strictly running sessions.
    expect(hub.machine("atlas").sessions.filter(session => session.ended)).toHaveLength(3);
    const replacement = { ...endedSession("watchful-heron-16", 60_000), liveness: "live" as const, ended: undefined, machine_online: true };
    machine.sessions = machine.sessions.map(row => row.name === replacement.name ? replacement : row);
    await hub.announceCatalog("atlas");
    await expect(pulseRows()).toHaveCount(2);
    machine.sessions = machine.sessions.map(row => row.project === "pulse-card" ? { ...row, liveness: "stale_metadata" as const } : row);
    await hub.announceCatalog("atlas");
    await expect(pulse()).toHaveCount(0);
    machine.sessions = machine.sessions.map(row => row.name === "golden-jaguar-44" ? { ...row, liveness: "live" as const } : row);
    await hub.announceCatalog("atlas");
    await expect(pulseRows()).toHaveCount(1);
    await page.reload();
    await expect(pulseRows()).toHaveCount(1);
    const capture = process.env.FULL_NAV_CAPTURE_DIR;
    const widths = capture ? [1280, 390] : [390];
    const schemes = capture ? ["light", "dark", "forced-colors"] as const : ["forced-colors"] as const;
    for (const width of widths) for (const scheme of schemes) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await page.emulateMedia({ colorScheme: scheme === "dark" ? "dark" : "light", forcedColors: scheme === "forced-colors" ? "active" : "none", reducedMotion: "reduce" });
      await back();
      expect(await page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches)).toBe(true);
      await expect(studioToggle()).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      if (capture) await page.screenshot({ path: `${capture}/${width}-${scheme}.png` });
    }
    await page.emulateMedia({ colorScheme: "light", forcedColors: "none", contrast: "more" });
    expect(await page.evaluate(() => matchMedia("(prefers-contrast: more)").matches)).toBe(true);
    if (capture) await page.screenshot({ path: `${capture}/a11y-contrast-more.png` });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.emulateMedia({ colorScheme: "light", forcedColors: "none", contrast: "no-preference" });
  });

  await journey.stage("Filters intersect search, and a decision opens its own session", async () => {
    await filters.getByRole("button", { name: /^Needs you/ }).click();
    await expect(allRows()).toHaveCount(1);
    await machineChip(studio(), "Borealis", "online").click();
    await expect(allRows()).toHaveCount(1);
    await expect(allRows()).toHaveAccessibleName(/wild-shark-68/);
    await page.getByRole("searchbox", { name: "Search conversations" }).fill("calm-puma");
    await expect(allRows()).toHaveCount(0);
    await expect(list.getByRole("group")).toHaveCount(0);
    await filters.getByRole("button", { name: /^Showing Borealis\./ }).click();
    await expect(allRows()).toHaveCount(1);
    await list.getByRole("region", { name: "Needs you", exact: true }).getByRole("button").click();
    await expect(page.getByRole("log")).toContainText("Which mix should I export?");
    await back();
    expect(hub.sends).toHaveLength(0);
  });

  await journey.stage("Keyboard collapse survives reload and footer filters the same register", async () => {
    await studioToggle().focus();
    await page.keyboard.press("Enter");
    await expect(studioToggle()).toBeFocused();
    await expect(studioToggle()).toHaveAttribute("aria-expanded", "false");
    await page.reload();
    await expect(studioToggle()).toHaveAttribute("aria-expanded", "false");
    await expect(studioRows()).toHaveCount(0);
    await studioToggle().click();
    await page.getByTestId("hub-footer-badges").getByRole("button").first().click();
    const dialog = page.getByRole("dialog", { name: "Paired machines", exact: true });
    await dialog.getByRole("group", { name: "Filter conversations" }).getByRole("button", { name: "Borealis", exact: true }).click();
    await expect(dialog).toBeHidden();
    const machineFilter = filters.getByRole("button", { name: /^Showing Borealis\./ });
    await expect(machineFilter).toHaveAttribute("aria-pressed", "true");
    await expect(allRows()).toHaveCount(1);
    await expect(allRows()).toHaveAccessibleName(/wild-shark-68/);
    await expect(list.getByRole("region", { name: "Needs you", exact: true })).toHaveCount(0);
    await machineFilter.click();
    await expect(allRows()).toHaveCount(3);
    // A kept catalog's positive online bit must yield to a lost connection.
    await hub.down("atlas", { sockets: "close" });
    await expect(machineChip(studio(), "Atlas", "offline")).toBeVisible();
    await hub.up("atlas");
    await expect(machineChip(studio(), "Atlas", "online")).toBeVisible();
  });
});
