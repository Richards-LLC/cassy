import { test, expect } from "./journey";
import { journeyStamp } from "./clock";
import { SCOPES, type Machine } from "./hub-double";

// cas-d6bf: seven live gabber-studio sessions on one machine, on the
// operator's phone. Before the fix, an End session line under every row left
// room for about three and a half sessions. With a conversation open, End
// session also emptied the list for about 4.4 s and dropped focus to the page.
const NAMES = ["calm-puma-34", "wild-shark-68", "noble-cheetah-84", "quiet-otter-12", "brave-lynx-7", "swift-heron-51", "amber-fox-29"];
const ATLAS: Machine = {
  id: "atlas",
  label: "Atlas · Linux",
  sessions: NAMES.map((supervisor, index) => ({
    name: `gabber-studio-${supervisor}`, supervisor, project_dir: "/projects/gabber-studio", workers: [], liveness: "live" as const,
    last_activity_at: journeyStamp(-(index + 1) * 7 * 60_000), last_activity: "supervisor → worker",
  })),
};

test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

test("HUB-J16 end a session from my phone", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], scopes: { atlas: [...SCOPES, "factory-manage"] } });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const row = (codename: string) => list.locator(".conversation-row", { hasText: codename });
  /** Rows whose whole box is inside the list's visible area. */
  const rowsInView = () => list.evaluate((nav) => {
    const box = nav.getBoundingClientRect();
    return [...nav.querySelectorAll(".conversation-row")].filter((node) => { const r = node.getBoundingClientRect(); return r.top >= box.top - 0.5 && r.bottom <= box.bottom + 0.5; }).length;
  });

  await journey.stage("See six sessions at once", async () => {
    await journey.open();
    await expect(list.locator(".conversation-group-head")).toHaveText("gabber-studio · 7 conversations on Atlas");
    await expect(list.locator(".conversation-row")).toHaveCount(7);
    expect(await rowsInView(), "at least six sessions fit the phone list").toBeGreaterThanOrEqual(6);
    // End session is in each row's corner, not a line of its own.
    await expect(list.getByRole("button", { name: "End session calm-puma-34 on Atlas" })).toBeVisible();
    // cas-339a: each row's time belongs to the row, and End is its own 44px column.
    const hits = await list.evaluate((nav) => [...nav.querySelectorAll<HTMLElement>(".conversation-row")].map((node) => {
      const time = node.querySelector(".conversation-when")!.getBoundingClientRect();
      const end = node.nextElementSibling!.querySelector(".conversation-end-ask")!.getBoundingClientRect();
      const atTime = document.elementFromPoint(time.left + time.width / 2, time.top + time.height / 2);
      const atEnd = document.elementFromPoint(end.left + end.width / 2, end.top + end.height / 2);
      return { time: atTime?.closest(".conversation-row") === node, end: Boolean(atEnd?.closest(".conversation-end-ask")), tall: end.height >= 44, apart: end.left >= time.right };
    }));
    expect(hits.slice(0, 6)).toEqual(Array(6).fill({ time: true, end: true, tall: true, apart: true }));
  });

  await journey.stage("Tap a session's time to open it, then come back", async () => {
    await row("swift-heron-51").locator(".conversation-when").tap();
    await expect(page.locator("#conversation-back")).toBeVisible();
    await page.locator("#conversation-back").tap();
    await expect(list.locator(".conversation-row")).toHaveCount(7);
  });

  await journey.stage("End session asks at once, focused on Cancel", async () => {
    const last = list.locator(".conversation-end").last();
    // The confirmation is there in the same task as the tap: no wait for a catalog poll.
    const at = await last.evaluate((control) => {
      control.querySelector<HTMLButtonElement>(".conversation-end-ask")!.click();
      const nav = control.closest("nav")!;
      const box = nav.getBoundingClientRect();
      const inView = [...control.querySelectorAll("button")].every((button) => { const r = button.getBoundingClientRect(); return r.top >= box.top - 0.5 && r.bottom <= box.bottom + 0.5; });
      return {
        question: control.querySelector(".conversation-end-question")?.textContent,
        focused: document.activeElement?.textContent,
        rows: nav.querySelectorAll(".conversation-row").length,
        inView,
      };
    });
    expect(at).toEqual({ question: "End amber-fox-29 on Atlas? Its supervisor and workers stop.", focused: "Cancel", rows: 7, inView: true });
    await expect(last.getByRole("button", { name: "Cancel" })).toBeFocused();
    await expect(last.getByRole("button", { name: "End session", exact: true })).toBeInViewport({ ratio: 1 });
    await expect(last.getByRole("button", { name: "Cancel" })).toBeInViewport({ ratio: 1 });
  });

  await journey.stage("A failed End session keeps its row and returns focus for retry (cas-a549)", async () => {
    const last = list.locator(".conversation-end").last();
    // One refused request, then the normal protocol double handles the retry.
    await page.route("**/v1/sessions/gabber-studio-amber-fox-29", async (route) => {
      if (route.request().method() !== "DELETE") return route.fallback();
      await route.fulfill({ status: 500, json: { error: "internal_error" } });
    }, { times: 1 });
    // cas-9ae6: what a screen reader says. Each change of text is attributed
    // to its nearest live region (role status/alert/log or aria-live, an
    // aria-live="off" ancestor silencing it); the control focus lands on is
    // read with its description.
    await page.evaluate(() => {
      const w = window as unknown as { __heard: string[] };
      w.__heard = [];
      const region = (node: Node): Element | null => {
        for (let element = node instanceof Element ? node : node.parentElement; element; element = element.parentElement) {
          const live = element.getAttribute("aria-live");
          if (live === "off") return null;
          if (live || ["status", "alert", "log"].includes(element.getAttribute("role") ?? "")) return element;
        }
        return null;
      };
      new MutationObserver((records) => {
        const seen = new Map<Element, string>();
        for (const record of records) {
          for (const node of record.type === "characterData" ? [record.target] : [...record.addedNodes]) {
            const speaker = region(node);
            const words = node.textContent?.trim() ?? "";
            if (!speaker || !words || seen.get(speaker) === words) continue;
            seen.set(speaker, words);
            w.__heard.push(words);
          }
        }
      }).observe(document.body, { subtree: true, childList: true, characterData: true });
      document.addEventListener("focusin", (event) => {
        const target = event.target as HTMLElement;
        const described = (target.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean).map((id) => document.getElementById(id)?.textContent?.trim() ?? "").join(" ");
        if (described) w.__heard.push(described);
      });
    });
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    const failure = "Could not end amber-fox-29 on Atlas. Try End session again. If it still fails, check the session on Atlas.";
    await expect(last.locator(".conversation-end-error")).toHaveText(failure);
    await expect(last.getByRole("button", { name: "End session amber-fox-29 on Atlas" })).toBeFocused();
    // Focus came back to End session, which reads the failure as its
    // description, so the failure is not also an alert: said once (cas-9ae6).
    await page.waitForTimeout(500);
    const heard = await page.evaluate(() => (window as unknown as { __heard: string[] }).__heard);
    expect(heard.filter((words) => words.includes("Could not end")), "the failure is said once").toEqual([failure]);
    await expect(row("amber-fox-29")).toHaveCount(1);
    await expect(list.locator(".conversation-row")).toHaveCount(7);
    expect(hub.ends).toEqual([]);
    // Enter on the returned control deliberately opens a fresh confirmation.
    await page.keyboard.press("Enter");
    await expect(last.getByRole("button", { name: "Cancel" })).toBeFocused();
  });

  await journey.stage("Cancel, then end it from the keyboard", async () => {
    const last = list.locator(".conversation-end").last();
    await last.getByRole("button", { name: "Cancel" }).tap();
    await expect(last.getByRole("button", { name: "End session amber-fox-29 on Atlas" })).toBeFocused();
    expect(hub.ends).toEqual([]);
    // cas-e634: keyboard only — Enter on End, Tab from Cancel to confirm, Enter.
    // cas-f60a: Cancel comes first, so the confirm is the next stop.
    await page.keyboard.press("Enter");
    await expect(last.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(last.getByRole("button", { name: "End session", exact: true })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(row("amber-fox-29")).toHaveCount(0);
    // The ended row was the last: focus lands on the row before it, never on the page.
    await expect(row("swift-heron-51")).toBeFocused();
    expect(hub.ends).toEqual([{ machine: "atlas", session: "gabber-studio-amber-fox-29", scopes: [...SCOPES, "factory-manage"] }]);
    await expect(list.locator(".conversation-group-head")).toHaveText("gabber-studio · 6 conversations on Atlas");
    // cas-f60a: told it ended, on screen where the row was and to a screen reader.
    await expect(list.locator(".conversation-ended")).toHaveText("amber-fox-29 on Atlas ended.");
    await expect(list.locator(".conversation-ended")).toBeInViewport();
    await expect(page.locator(".conversation-ended-status[role=status]")).toHaveText("amber-fox-29 on Atlas ended.");
  });
});
