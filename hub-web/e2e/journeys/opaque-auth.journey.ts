import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { HubDouble, type HistoryPage } from "./hub-double";
import { ProtocolClock } from "./protocol-clock";

test("HUB-J12 opaque authenticated requests recover without re-pairing (cas-b85a)", journeyPart, async ({ page, journey }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const clock = new ProtocolClock(page);
  const history: Record<string, HistoryPage[]> = {};
  const hub = new HubDouble(page, { machines: [ATLAS], paired: ["atlas"], multiplex: true, time: clock, history });
  await page.addInitScript(() => { Math.random = () => 0.5; });
  await hub.install();
  let blocked = false;
  await page.route("https://atlas.test/v1/**", route => {
    // Chrome exposes a blocked preflight/LNA request as an opaque fetch
    // rejection. The credential-free health check can still succeed.
    if (blocked && new URL(route.request().url()).pathname !== "/v1/health") return route.abort("failed");
    return route.fallback();
  });
  await page.goto("./");
  await hub.seedPaired();
  await clock.start();
  await page.goto("./");
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
  const header = page.locator("#conversation-connection");
  await expect(header).toHaveText(" · Live");
  await journey.stage("A blocked authenticated route keeps the active pairing and retries", async () => {
    blocked = true;
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await clock.advance(1_000);
    await expect(header).toHaveText(" · Reconnecting");
    await expect(page.getByText(/needs pairing|was revoked|no longer paired/i).filter({ visible: true })).toHaveCount(0);
  });
  await journey.stage("Once the browser allows the route, the conversation comes back by itself", async () => {
    history[PELICAN] = [{ messages: [], has_earlier: false, replies: [{
      notification_id: 3539905, reply_to: null, session: PELICAN, kind: "status", attachments: [],
      message: "Queued while the connection was down.", summary: "", device_id: "journey-device",
      at: new Date(clock.now()).toISOString(),
    }] }];
    blocked = false;
    await clock.advance(10_000);
    await expect(header).toHaveText(" · Live");
    await expect(page.locator("#hub-footer-badges .machine-badge-state")).not.toHaveText("Reconnecting");
    await expect(page.getByRole("log").getByText("Queued while the connection was down.", { exact: true })).toBeVisible();
    const accepted = hub.nextSend();
    await page.getByRole("textbox", { name: "Your message" }).fill("After the browser block clears");
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    expect((await accepted).text).toBe("After the browser block clears");
    hub.answerLatest(PELICAN, "The connection recovered.");
    await expect(page.getByRole("log").getByText("The connection recovered.", { exact: true })).toBeVisible();
    expect(hub.sends).toHaveLength(1);
    expect(errors).toEqual([]);
  });
});

test("HUB-J12 denied Local network access explains site settings without re-pairing (cas-b85a)", journeyPart, async ({ page, journey }) => {
  const hub = new HubDouble(page, { machines: [ATLAS], paired: ["atlas"], multiplex: true });
  await hub.install();
  await page.goto("./");
  await hub.seedPaired();
  // Use the real stored base URL and permission name; block outgoing requests
  // before any test credential can reach the actual hub.
  await page.route("https://soundwave-linux.tailf5a734.ts.net/**", route => route.abort("failed"));
  await page.evaluate(async () => {
    const db: IDBDatabase = await new Promise((resolve, reject) => {
      const request = indexedDB.open("cas-commander-v1");
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    await new Promise<void>((resolve, reject) => {
      const transaction = db.transaction("machines", "readwrite");
      const store = transaction.objectStore("machines");
      const request = store.get("atlas");
      request.onsuccess = () => store.put({ ...request.result, label: "soundwave", baseUrl: "https://soundwave-linux.tailf5a734.ts.net" });
      transaction.oncomplete = () => resolve();
      transaction.onerror = () => reject(transaction.error);
    });
    db.close();
  });
  await page.addInitScript(() => {
    const query = navigator.permissions.query.bind(navigator.permissions);
    navigator.permissions.query = descriptor => descriptor.name === "local-network" as PermissionName
      ? Promise.resolve({ state: "denied" } as PermissionStatus) : query(descriptor);
  });
  await journey.stage("The browser permission has a visible next step, including without a conversation", async () => {
    await page.goto("./");
    await expect(page.locator("#network-access-help")).toHaveText("To reach soundwave, allow Local network access for this page in your browser's site settings.");
    await expect(page.getByText(/needs pairing|was revoked/i).filter({ visible: true })).toHaveCount(0);
    // cas-7c37f: one owner for the outage sentence; the notice carries only the remedy.
    // cas-97d58 F16: the cause is the browser; nothing says to check that the machines are awake.
    await expect(page.locator("#conversation-empty")).toHaveText("This browser is blocking its connection to your paired machines.");
    await expect(page.getByText(/keeps retrying|machines are awake/).filter({ visible: true })).toHaveCount(0);
    expect(await page.locator(".conversation-sidebar").evaluate(aside => ((aside as HTMLElement).innerText.match(/allow Local network access/gi) ?? []).length), "one remedy").toBe(1);
  });
  for (const size of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
    for (const scheme of ["light", "dark"] as const) {
      await journey.stage(`Permission guidance at ${size.width}px in ${scheme}`, async () => {
        await page.setViewportSize(size);
        await page.emulateMedia({ colorScheme: scheme });
        await expect(page.locator("html")).toHaveAttribute("data-scheme", scheme);
        await expect(page.locator("#network-access-help")).toBeInViewport();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        // cas-7c37f: the notice shares the empty copy's 18px column, never the panel edge.
        const edges = await page.evaluate(() => {
          const box = (selector: string) => document.querySelector(selector)!.getBoundingClientRect();
          const aside = box(".conversation-sidebar"), help = box("#network-access-help"), empty = box("#conversation-empty");
          return { left: help.left - aside.left, right: aside.right - help.right, emptyLeft: empty.left - aside.left, emptyRight: aside.right - empty.right };
        });
        expect(edges).toEqual({ left: 18, right: 18, emptyLeft: 18, emptyRight: 18 });
        const contrast = await page.locator("#network-access-help").evaluate(element => {
          const rgba = (color: string) => {
            const channels = color.match(/[\d.]+/g)!.map(Number);
            return [channels[0]!, channels[1]!, channels[2]!, channels[3] ?? 1];
          };
          const over = (foreground: number[], background: number[]) => foreground.slice(0, 3)
            .map((channel, index) => channel * foreground[3]! + background[index]! * (1 - foreground[3]!));
          // The warning tint is translucent. Compare rendered colors after
          // compositing it over its actual ancestors, not the tint's raw RGB.
          const layers: number[][] = [];
          for (let ancestor: Element | null = element; ancestor; ancestor = ancestor.parentElement) {
            layers.push(rgba(getComputedStyle(ancestor).backgroundColor));
          }
          const background = layers.reverse().reduce((surface, layer) => over(layer, surface), [255, 255, 255]);
          const foreground = over(rgba(getComputedStyle(element).color), background);
          const luminance = (color: number[]) => {
            const channels = color.map(value => {
              const channel = value / 255;
              return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
            });
            return channels[0]! * 0.2126 + channels[1]! * 0.7152 + channels[2]! * 0.0722;
          };
          const front = luminance(foreground);
          const back = luminance(background);
          return (Math.max(front, back) + 0.05) / (Math.min(front, back) + 0.05);
        });
        expect(contrast).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
  await journey.stage("Under forced colors the notice keeps a visible edge", async () => {
    await page.emulateMedia({ forcedColors: "active" });
    expect(await page.evaluate(() => matchMedia("(forced-colors: active)").matches)).toBe(true);
    const border = await page.locator("#network-access-help").evaluate(element => getComputedStyle(element).borderTopWidth);
    expect(border).toBe("1px");
  });
});
