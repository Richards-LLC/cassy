import { createHash } from "node:crypto";
import { test, expect, journeyPart } from "./journey";
import { journeyDay, journeyNow } from "./clock";
import { operatorCloudDouble, routeOperatorCloud } from "./operator-cloud-route";

const SESSION = "cas-src-amber-fox-29";
const HUB = "hub-soundwave";
const PROJECT = "github.com/richards-llc/cassy";
const sessionId = `s_${createHash("sha256").update(SESSION).digest("base64url")}`;

test("HUB-J19 machine alerts reach a separate device with no hub online (cas-e3dd)", journeyPart, async ({ page, journey, browser }) => {
  // Real browser enrollment/PoP/HPKE/IndexedDB, protocol-double presence.
  // This cannot establish the deployed watchdog's outage detection bound.
  const cloud = operatorCloudDouble();
  const commandKey = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
  const soundwave = await cloud.enrollMachine(HUB, [PROJECT], new Uint8Array(await crypto.subtle.exportKey("raw", commandKey.publicKey)), "soundwave");
  cloud.presence.set(soundwave.id, { monitoring: "disabled", monitoring_generation: "0" });
  await routeOperatorCloud(page.context(), cloud);
  await page.setViewportSize({ width: 390, height: 844 });
  const inbox = page.getByRole("dialog", { name: "Operator inbox" });
  const row = inbox.locator(`[data-machine-presence="${soundwave.id}"]`);
  const notices = inbox.getByRole("list", { name: "Machine alert history" });
  const refresh = async () => inbox.getByRole("button", { name: "Refresh machine status" }).click();
  const peer = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  try {
    const desk = await peer.newPage();
    await desk.clock.install({ time: journeyNow() });
    await routeOperatorCloud(peer, cloud);
    const deskInbox = desk.getByRole("dialog", { name: "Operator inbox" });
    const report = () => ({
      monitoring: "enabled", monitoring_generation: "1", presence: "observed", last_report_at: new Date(journeyNow()).toISOString(),
      lease_expires_at: new Date(journeyNow() + 180_000).toISOString(), deadline_at: new Date(journeyNow() + 240_000).toISOString(),
      components: [
        { component: "hub", state: "up", observed_at: new Date(journeyNow()).toISOString() },
        { component: "serve", state: "degraded", observed_at: new Date(journeyNow()).toISOString() },
        { component: "factory", state: "unknown", observed_at: new Date(journeyNow()).toISOString() },
      ],
    });
    let outageId = "";
    await journey.stage("Enable alerts and confirm the first machine report", async () => {
      await journey.open();
      await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
      await inbox.getByLabel("Name this browser").fill("Presence phone");
      await inbox.getByRole("button", { name: "Sign in" }).click();
      const code = (await inbox.getByLabel("Sign-in code").textContent())!.trim();
      await cloud.approve(code, { capabilities: ["feed:read", "account:manage"] });
      await expect(row.getByText("Monitoring is off", { exact: true })).toBeVisible({ timeout: 15_000 });
      expect(cloud.presence.get(soundwave.id)?.monitoring).toBe("disabled");
      const settings = row.locator("summary");
      await expect(settings).toHaveText("Monitoring settings");
      await expect(settings).toHaveAccessibleName("Monitoring settings for soundwave");
      const chevron = settings.locator(".machine-presence-chevron");
      await expect(chevron).toBeVisible();
      const closedMarker = await chevron.evaluate((node) => getComputedStyle(node).transform);
      await settings.focus();
      await settings.press("Enter");
      await expect(row.getByText(/keeps notices for 90 days/)).toBeVisible();
      expect(await chevron.evaluate((node) => getComputedStyle(node).transform)).not.toBe(closedMarker);
      await expect(row.getByText(/Enable alerts, then run cas hub restart on soundwave/)).toBeVisible();
      const enable = row.getByRole("button", { name: "Enable alerts for soundwave", exact: true });
      await expect(enable).toHaveText("Enable alerts");
      await page.emulateMedia({ forcedColors: "active" });
      expect(await page.evaluate(() => matchMedia("(forced-colors: active)").matches)).toBe(true);
      for (const button of [enable, inbox.getByRole("button", { name: "Refresh machine status" }), inbox.getByRole("button", { name: "Sign out of the inbox", exact: true })]) {
        const edges = await button.evaluate((node) => {
          const style = getComputedStyle(node);
          return ["Top", "Right", "Bottom", "Left"].map((side) => ({
            width: parseFloat(style.getPropertyValue(`border-${side.toLowerCase()}-width`)),
            style: style.getPropertyValue(`border-${side.toLowerCase()}-style`),
            color: style.getPropertyValue(`border-${side.toLowerCase()}-color`),
          }));
        });
        for (const edge of edges) {
          expect(edge.width).toBeGreaterThan(0);
          expect(edge.style).toBe("solid");
          expect(edge.color).not.toBe("rgba(0, 0, 0, 0)");
        }
      }
      if (process.env.QA_ARTIFACTS) {
        await enable.scrollIntoViewIfNeeded();
        await page.screenshot({ path: `${process.env.QA_ARTIFACTS}/presence-enable-forced-colors.png` });
      }
      await page.emulateMedia({ forcedColors: null });
      await enable.focus();
      await enable.press("Enter");
      await expect(row.getByText("Waiting for the first report", { exact: true })).toBeVisible();
      expect(cloud.presence.get(soundwave.id)?.monitoring_generation).toBe("1");
      cloud.presence.set(soundwave.id, report());
      await refresh();
      await expect(row.getByText("Reporting to Cassy Cloud", { exact: true })).toBeVisible();
      await expect(row.getByText(/Serve: degraded/)).toBeVisible();
      await expect(row.getByText(/Factory: not known/)).toBeVisible();
      await expect(row.getByText(/Last report/)).toBeVisible();
      await expect(row.locator("p time")).toHaveText(/^(?:now|\d+[smhd] ago)$/);
      await expect(inbox.locator(".operator-inbox-bubble")).toHaveCount(0);
    });
    await journey.stage("A separate profile sees the status and signed outage without pairing", async () => {
      await desk.goto(page.url());
      await desk.getByRole("button", { name: "Operator inbox", exact: true }).click();
      await deskInbox.getByLabel("Name this browser").fill("Presence desk");
      await deskInbox.getByRole("button", { name: "Sign in" }).click();
      const code = (await deskInbox.getByLabel("Sign-in code").textContent())!.trim();
      await cloud.approve(code);
      await expect(deskInbox.getByText("Reporting to Cassy Cloud", { exact: true })).toBeVisible({ timeout: 15_000 });
      await expect(deskInbox.getByRole("button", { name: /Enable alerts|Disable alerts/ })).toHaveCount(0);
      const outage = await cloud.appendObserverNotice(soundwave, "machine_unobserved");
      outageId = outage.eventId;
      cloud.presence.set(soundwave.id, { ...report(), presence: "unobserved", open_outage: { outage_epoch: "1", opened_at: new Date(journeyNow()).toISOString(), unobserved_event_id: outageId } });
      await refresh();
      await expect(notices.getByText("soundwave unreachable", { exact: true })).toBeVisible({ timeout: 15_000 });
      await expect(inbox.getByRole("heading", { name: "Alert history", exact: true })).toBeVisible();
      expect(await inbox.locator(".machine-presence-history").evaluate((node) => parseFloat(getComputedStyle(node).borderTopWidth))).toBeGreaterThan(0);
      await expect(deskInbox.getByText("soundwave unreachable", { exact: true })).toBeVisible({ timeout: 15_000 });
      const phone = [...cloud.devices.values()].find((device) => device.label === "Presence phone")!;
      const desktop = [...cloud.devices.values()].find((device) => device.label === "Presence desk")!;
      await expect.poll(() => cloud.acks.get(phone.id)?.size ?? 0).toBe(1);
      await expect.poll(() => cloud.acks.get(desktop.id)?.size ?? 0).toBe(1);
      await expect(inbox.locator(".operator-inbox-bubble")).toHaveCount(0);
    });
    await journey.stage("Reload preserves the outage, recovery, and observer state", async () => {
      await page.reload();
      await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
      await expect(notices.locator(`[data-presence-event="${outageId}"]`)).toHaveCount(1);
      await expect(notices.getByText("soundwave unreachable", { exact: true })).toBeVisible();
      await expect(inbox.getByRole("button", { name: "Sign in", exact: true })).toHaveCount(0);
      const recovery = await cloud.appendObserverNotice(soundwave, "machine_recovered", { refEventId: outageId });
      cloud.presence.set(soundwave.id, report());
      await refresh();
      await expect(notices.getByText("soundwave recovered", { exact: true })).toBeVisible({ timeout: 15_000 });
      await expect(notices.locator(`[data-presence-event="${recovery.eventId}"]`)).toHaveAttribute("data-ref-event", outageId);
      await expect(notices.getByText("soundwave unreachable", { exact: true })).toHaveCount(1);
      await expect(deskInbox.getByText("soundwave recovered", { exact: true })).toBeVisible({ timeout: 15_000 });
      const phone = [...cloud.devices.values()].find((device) => device.label === "Presence phone")!;
      await expect.poll(() => cloud.acks.get(phone.id)?.size ?? 0).toBe(2);
      await expect(inbox.locator(".operator-inbox-bubble")).toHaveCount(0);
      cloud.observerStatus = "unavailable";
      await refresh();
      await expect(inbox.getByText(/Cassy Cloud's alert check is unavailable/)).toBeVisible();
      await expect(row.getByText("Reporting to Cassy Cloud", { exact: true })).toBeVisible();
    });
    const cells = [
      { name: "phone-light", width: 390, height: 844, scheme: "light" as const },
      { name: "phone-dark", width: 390, height: 844, scheme: "dark" as const },
      { name: "desktop-light", width: 1280, height: 800, scheme: "light" as const },
      { name: "desktop-dark", width: 1280, height: 800, scheme: "dark" as const },
      { name: "forced-colors", width: 390, height: 844, scheme: "light" as const, media: { forcedColors: "active" as const }, query: "(forced-colors: active)" },
      { name: "more-contrast", width: 390, height: 844, scheme: "light" as const, media: { contrast: "more" as const }, query: "(prefers-contrast: more)" },
      { name: "reduced-motion", width: 390, height: 844, scheme: "light" as const, media: { reducedMotion: "reduce" as const }, query: "(prefers-reduced-motion: reduce)" },
    ];
    await journey.stage("Check machine alerts across accessibility modes", async () => {
      for (const cell of cells) {
        await page.setViewportSize({ width: cell.width, height: cell.height });
        await page.emulateMedia({ colorScheme: cell.scheme, forcedColors: null, contrast: null, reducedMotion: null, ...("media" in cell ? cell.media : {}) });
        await page.evaluate((scheme) => { document.documentElement.dataset.scheme = scheme; }, cell.scheme);
        if ("query" in cell) expect(await page.evaluate((query) => matchMedia(query).matches, cell.query!)).toBe(true);
        await expect(row.getByText(/Serve: degraded/)).toBeVisible();
        await expect(notices.getByText("soundwave recovered", { exact: true })).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        if (cell.name === "forced-colors") {
          await row.locator("summary").click();
          const disable = row.getByRole("button", { name: "Disable alerts for soundwave", exact: true });
          await expect(disable).toBeVisible();
          expect(await disable.evaluate((node) => parseFloat(getComputedStyle(node).borderTopWidth))).toBeGreaterThan(0);
          if (process.env.QA_ARTIFACTS) {
            await disable.scrollIntoViewIfNeeded();
            await page.screenshot({ path: `${process.env.QA_ARTIFACTS}/presence-disable-forced-colors.png` });
          }
          await inbox.getByRole("button", { name: "Refresh machine status" }).scrollIntoViewIfNeeded();
        }
        if (process.env.QA_ARTIFACTS) await page.screenshot({ path: `${process.env.QA_ARTIFACTS}/presence-${cell.name}.png` });
      }
      if (process.env.QA_ARTIFACTS) {
        const { readFile, writeFile } = await import("node:fs/promises");
        const css = await readFile(new URL("../../dist/app.css", import.meta.url), "utf8");
        const dialog = await inbox.evaluate((node) => node.outerHTML);
        await writeFile(`${process.env.QA_ARTIFACTS}/presence.html`, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Machine alerts</title><style>${css}</style></head><body>${dialog}</body></html>`);
      }
    });
    await journey.stage("The phone disables alerts while the hub is unreachable", async () => {
      await page.emulateMedia({ forcedColors: null, contrast: null, reducedMotion: null });
      const settings = row.locator("summary");
      if (!(await row.locator("details").evaluate((node) => (node as HTMLDetailsElement).open))) {
        await settings.focus();
        await settings.press("Enter");
      }
      const disable = row.getByRole("button", { name: "Disable alerts for soundwave", exact: true });
      await expect(disable).toHaveText("Disable alerts");
      await disable.focus();
      await disable.press("Enter");
      await expect(row.getByText("Monitoring is off", { exact: true })).toBeVisible();
      expect(cloud.presence.get(soundwave.id)?.monitoring_generation).toBe("2");
      await deskInbox.getByRole("button", { name: "Refresh machine status" }).click();
      await expect(deskInbox.getByText("Monitoring is off", { exact: true })).toBeVisible();
      await expect(notices.getByText("soundwave recovered", { exact: true })).toHaveCount(1);
      await expect(row.getByRole("button", { name: "Enable alerts for soundwave", exact: true })).toMatchAriaSnapshot('- button "Enable alerts for soundwave"');
      if (process.env.QA_ARTIFACTS) await page.screenshot({ path: `${process.env.QA_ARTIFACTS}/keyboard-disable.png` });
    });
  } finally {
    await peer.close();
  }
});

/** A supervisor turn as the hub's drain seals it: an m263 frozen snapshot. */
function supervisorTurn(promptId: number, prompt: string, at: string) {
  return {
    type: "cas.operator.turn",
    v: 1,
    event_id: `evt-${promptId}`,
    session_name: SESSION,
    snapshot: { schema_version: 1, prompt_id: promptId, source: "supervisor", target: "operator", prompt, summary: "", kind: "answer", created_at: at, factory_session: SESSION, attachments: [] },
  };
}

test("HUB-J19 read my inbox on a new phone while the machine is off", async ({ page, journey, browser }) => {
  const cloud = operatorCloudDouble();
  const commandKey = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
  const soundwave = await cloud.enrollMachine(HUB, [PROJECT], new Uint8Array(await crypto.subtle.exportKey("raw", commandKey.publicKey)), "soundwave");
  for (const [id, text, at] of [
    [101, "Started the release epic. Three lanes.", journeyDay(20, 9)],
    [102, "Lane two is blocked on the cloud contract.", journeyDay(9, 15)],
    [103, "Cloud half shipped. Ready for your go.", journeyDay(1, 18)],
  ] as const) {
    await cloud.appendSessionEvent({ hubId: HUB, projectId: PROJECT, sessionId, plaintext: supervisorTurn(id, text, at) });
  }
  await routeOperatorCloud(page.context(), cloud);
  await page.setViewportSize({ width: 390, height: 844 });
  const inbox = page.getByRole("dialog", { name: "Operator inbox" });

  await journey.stage("Sign in on a brand-new phone with no machine paired", async () => {
    await journey.open();
    await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
    await expect(inbox.getByText("this isn’t end-to-end encryption")).toBeVisible();
    await inbox.getByLabel("Name this browser").fill("Pixel 9");
    // QA F02 (round 1): Enter in the name field signs in.
    await inbox.getByLabel("Name this browser").press("Enter");
    await expect(inbox.getByLabel("Sign-in code")).toHaveText(/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/);
    await expect(inbox.getByRole("link", { name: "Approve on Cassy Cloud" })).toHaveAttribute("href", /\/operator\/approve\?code=/);
    // cas-d043 G05: one brand, the one on the button below it.
    await expect(inbox.locator(".operator-inbox-lead")).toHaveText("Approve this browser from your Cassy Cloud account. Check that the code matches.");
  });

  await journey.stage("Approve it from the account; the weeks of messages are there", async () => {
    const code = (await inbox.getByLabel("Sign-in code").textContent())!.trim();
    await cloud.approve(code, { scopes: [{ hub_id: HUB, project_id: PROJECT, session_id: null, operations: ["operator_message"] }] });
    const thread = inbox.getByRole("button", { name: /soundwave · amber-fox-29/ });
    await expect(thread).toBeVisible({ timeout: 15_000 });
    await thread.click();
    for (const text of ["Started the release epic. Three lanes.", "Lane two is blocked on the cloud contract.", "Cloud half shipped. Ready for your go."]) {
      await expect(inbox.getByText(text)).toBeVisible();
    }
    // Each message was stored on this phone and acknowledged as stored, once.
    const phone = [...cloud.devices.values()].find((device) => device.label === "Pixel 9")!;
    await expect.poll(() => cloud.acks.get(phone.id)?.size ?? 0).toBe(3);
  });

  await journey.stage("Reply while soundwave is off: it waits for soundwave", async () => {
    await inbox.getByLabel(/Reply — soundwave gets it when it’s back/).fill("Go. Cut the release.");
    await inbox.getByRole("button", { name: "Queue reply" }).click();
    await expect(inbox.getByText("Waiting for soundwave")).toBeVisible();
    // QA F01 (round 1): bodies read as messages in the thread's bubble inks,
    // metadata stays quieter, and the reply's machine state is a state line.
    const look = await inbox.evaluate((dialog) => {
      const style = (selector: string) => getComputedStyle(dialog.querySelector(selector)!);
      const supervisor = style(".operator-inbox-reply .operator-inbox-bubble");
      const mine = style(".operator-inbox-command .operator-inbox-bubble");
      const author = style(".operator-inbox-author");
      const state = dialog.querySelector(".operator-inbox-command-state")!;
      return {
        supervisorInk: supervisor.color,
        supervisorFill: supervisor.backgroundColor,
        mineInk: mine.color,
        mineFill: mine.backgroundColor,
        authorInk: author.color,
        stateWeight: Number(getComputedStyle(state).fontWeight),
        stateInk: getComputedStyle(state).color,
        stateKind: state.getAttribute("data-state"),
      };
    });
    expect(look.supervisorInk).not.toBe(look.authorInk);
    expect(look.supervisorFill).not.toBe("rgba(0, 0, 0, 0)");
    expect(look.mineFill).not.toBe(look.supervisorFill);
    expect(look.mineInk).not.toBe(look.authorInk);
    expect(look.stateWeight).toBeGreaterThanOrEqual(600);
    expect(look.stateInk).not.toBe(look.authorInk);
    expect(look.stateKind).toBe("pending_machine");
    expect([...cloud.commands.values()].map((command) => [command.machineId, command.status])).toEqual([[soundwave.id, "pending_machine"]]);
  });

  await journey.stage("A second browser profile sees the history and the queued reply", async () => {
    const desktop = await browser.newContext({ viewport: { width: 1280, height: 800 }, timezoneId: "UTC" });
    const other = await desktop.newPage();
    await other.clock.install({ time: journeyNow() });
    await routeOperatorCloud(desktop, cloud);
    await other.goto(page.url());
    await other.getByRole("button", { name: "Operator inbox", exact: true }).click();
    const otherInbox = other.getByRole("dialog", { name: "Operator inbox" });
    await otherInbox.getByLabel("Name this browser").fill("Desk");
    await otherInbox.getByRole("button", { name: "Sign in" }).click();
    const code = (await otherInbox.getByLabel("Sign-in code").textContent())!.trim();
    await cloud.approve(code);
    await otherInbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click({ timeout: 15_000 });
    await expect(otherInbox.getByText("Cloud half shipped. Ready for your go.")).toBeVisible();
    await expect(otherInbox.getByText("Go. Cut the release.")).toBeVisible();
    // This profile cannot queue replies: it was approved for reading only.
    await expect(otherInbox.getByText(/can read this conversation but not reply/)).toBeVisible();
    const desk = [...cloud.devices.values()].find((device) => device.label === "Desk")!;
    expect(cloud.acks.get(desk.id)?.size).toBe(4);
    await desktop.close();
  });

  const qa = process.env.QA_ARTIFACTS;
  if (qa) {
    // cas-qa-craft polish evidence for cas-9b7d: four renders of the open
    // thread, a standalone snapshot of the dialog with the committed CSS,
    // and the three a11y modes proven by matchMedia.
    const { readFile, writeFile } = await import("node:fs/promises");
    for (const size of [{ name: "desktop", width: 1280, height: 800 }, { name: "phone", width: 390, height: 844 }]) {
      for (const scheme of ["light", "dark"] as const) {
        await page.setViewportSize(size);
        await page.emulateMedia({ colorScheme: scheme });
        await page.evaluate((value) => { document.documentElement.dataset.scheme = value; }, scheme);
        await expect(inbox.getByText("Waiting for soundwave")).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scroll").toBe(true);
        await page.screenshot({ path: `${qa}/inbox-${scheme}-${size.name}.png` });
      }
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await page.emulateMedia({ colorScheme: "light" });
    await page.evaluate(() => { delete document.documentElement.dataset.scheme; });
    const css = await readFile(new URL("../../dist/app.css", import.meta.url), "utf8");
    const dialog = await inbox.evaluate((node) => node.outerHTML);
    await writeFile(`${qa}/inbox.html`, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Operator inbox</title><style>${css}</style></head><body>${dialog}</body></html>`);
    const modes: Array<[string, string, Parameters<typeof page.emulateMedia>[0]]> = [
      ["forced-colors", "(forced-colors: active)", { forcedColors: "active", reducedMotion: null, contrast: null }],
      ["reduced-motion", "(prefers-reduced-motion: reduce)", { forcedColors: null, reducedMotion: "reduce", contrast: null }],
      ["contrast-more", "(prefers-contrast: more)", { forcedColors: null, reducedMotion: null, contrast: "more" }],
    ];
    for (const [name, query, media] of modes) {
      await page.emulateMedia(media);
      expect(await page.evaluate((q) => matchMedia(q).matches, query)).toBe(true);
      await expect(inbox.getByText("Waiting for soundwave")).toBeVisible();
      await page.screenshot({ path: `${qa}/a11y-${name}.png` });
    }
    await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
  }

  await journey.stage("soundwave returns and accepts the reply; a reload keeps everything", async () => {
    const [command] = cloud.commands.keys();
    cloud.acceptCommand(command);
    await expect(inbox.getByText("soundwave received it")).toBeVisible({ timeout: 15_000 });
    await page.reload();
    await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
    await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
    await expect(inbox.getByText("Started the release epic. Three lanes.")).toBeVisible();
    await expect(inbox.getByText("soundwave received it")).toBeVisible();
    // Signed in once: the reload neither asked again nor replayed history twice.
    expect(cloud.enrollments.size).toBe(2);
  });
});

test("HUB-J19 a message that can't be verified is named, never shown (cas-9b7d QA F03)", journeyPart, async ({ page, journey }) => {
  const cloud = operatorCloudDouble();
  const commandKey = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
  await cloud.enrollMachine(HUB, [PROJECT], new Uint8Array(await crypto.subtle.exportKey("raw", commandKey.publicKey)), "soundwave");
  await cloud.appendSessionEvent({ hubId: HUB, projectId: PROJECT, sessionId, plaintext: supervisorTurn(201, "Genuine message one.", journeyDay(3, 9)) });
  await cloud.appendSessionEvent({ hubId: HUB, projectId: PROJECT, sessionId, plaintext: supervisorTurn(202, "TAMPERED-SECRET should never render.", journeyDay(2, 9)) });
  // One ciphertext byte flipped after sealing; the stated digest is left as the producer sent it.
  const tampered = cloud.events[1];
  tampered.ciphertext = new Uint8Array(tampered.ciphertext);
  tampered.ciphertext[tampered.ciphertext.length - 5] ^= 0x41;
  await routeOperatorCloud(page.context(), cloud);
  await page.setViewportSize({ width: 390, height: 844 });
  const inbox = page.getByRole("dialog", { name: "Operator inbox" });

  await journey.stage("Sign in; the genuine message shows and the refused one is named", async () => {
    await journey.open();
    await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
    await inbox.getByLabel("Name this browser").fill("QA phone");
    await inbox.getByRole("button", { name: "Sign in" }).click();
    const code = (await inbox.getByLabel("Sign-in code").textContent())!.trim();
    await cloud.approve(code);
    await expect(inbox.getByText("1 message couldn’t be verified, so it isn’t shown.")).toBeVisible({ timeout: 15_000 });
    await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
    await expect(inbox.getByText("Genuine message one.")).toBeVisible();
    await expect(inbox.getByText("1 message couldn’t be verified, so it isn’t shown.")).toBeVisible();
    await expect(page.getByText(/TAMPERED-SECRET/)).toHaveCount(0);
    const phone = [...cloud.devices.values()].find((device) => device.label === "QA phone")!;
    expect([...(cloud.acks.get(phone.id) ?? [])].map((value) => String(Array.isArray(value) ? value[0] : value))).not.toContain(tampered.eventId);
  });
});

test("HUB-J19 terminal question and supervisor answer survive cloud replay on two devices (cas-5c89)", journeyPart, async ({ page, journey, browser }) => {
  const cloud = operatorCloudDouble();
  const commandKey = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
  await cloud.enrollMachine(HUB, [PROJECT], new Uint8Array(await crypto.subtle.exportKey("raw", commandKey.publicKey)), "soundwave");
  await cloud.appendSessionEvent({ hubId: HUB, projectId: PROJECT, sessionId, plaintext: {
    type: "cas.operator.turn", v: 1, session_name: SESSION,
    snapshot: { prompt_id: 301, source: "terminal", target: "terminal-history", prompt: "What changed in Violet today?", operator: null, factory_session: SESSION, created_at: journeyDay(1, 12) },
  } });
  await cloud.appendSessionEvent({ hubId: HUB, projectId: PROJECT, sessionId, plaintext: supervisorTurn(302, JSON.stringify({
    schema_version: 2, reply_to: 301, message: "**Two fixes landed.**", summary: "Two fixes", device_id: "*", kind: "answer", attachments: [],
  }), journeyDay(1, 13)) });
  await routeOperatorCloud(page.context(), cloud);
  await page.setViewportSize({ width: 390, height: 844 });

  const readThread = async (reader: typeof page, label?: string) => {
    await reader.getByRole("button", { name: "Operator inbox", exact: true }).click();
    const inbox = reader.getByRole("dialog", { name: "Operator inbox" });
    if (label) {
      await inbox.getByLabel("Name this browser").fill(label);
      await inbox.getByRole("button", { name: "Sign in" }).click();
      const code = (await inbox.getByLabel("Sign-in code").textContent())!.trim();
      await cloud.approve(code);
    }
    await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click({ timeout: 15_000 });
    await expect(inbox.locator(".operator-inbox-bubble")).toHaveText(["What changed in Violet today?", "Two fixes landed."]);
    await expect(inbox.locator(".operator-inbox-author")).toHaveText(["You", "Supervisor"]);
    await expect(inbox.locator("strong")).toHaveText("Two fixes landed.");
  };

  await journey.stage("Read both terminal turns on the phone, then revisit without duplicates", async () => {
    await journey.open();
    await readThread(page, "Terminal phone");
    await page.reload();
    await readThread(page);
  });
  await journey.stage("Read the same question and answer from a fresh desktop profile", async () => {
    const desktop = await browser.newContext({ viewport: { width: 1280, height: 800 }, timezoneId: "UTC" });
    try {
      await routeOperatorCloud(desktop, cloud);
      const reader = await desktop.newPage();
      await reader.clock.install({ time: journeyNow() });
      await reader.goto(page.url());
      await readThread(reader, "Terminal desk");
    } finally {
      await desktop.close();
    }
    expect(cloud.enrollments.size).toBe(2);
  });
});

// Cloud #174/PR #181: the shipped capability enables this flow; absent support offers no control.
async function replyUpgradeCloud(enabled = true) {
  const cloud = operatorCloudDouble();
  cloud.replyGrants.enabled = enabled;
  const key = await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"]);
  await cloud.enrollMachine(HUB, [PROJECT], new Uint8Array(await crypto.subtle.exportKey("raw", key.publicKey)), "soundwave");
  await cloud.appendSessionEvent({ hubId: HUB, projectId: PROJECT, sessionId, plaintext: {
    type: "cas.operator.turn", v: 1, session_name: SESSION,
    snapshot: { prompt_id: 874, source: "terminal", target: "terminal-history", prompt: "Keep this conversation.", operator: null, factory_session: SESSION, created_at: journeyDay(1, 12) },
  } });
  return cloud;
}
async function enrollReplyBrowser(reader: import("@playwright/test").Page, cloud: Awaited<ReturnType<typeof replyUpgradeCloud>>, label: string, owner = false, waitForHistory = true) {
  await reader.getByRole("button", { name: "Operator inbox", exact: true }).click();
  const inbox = reader.getByRole("dialog", { name: "Operator inbox" });
  await inbox.getByLabel("Name this browser").fill(label);
  await inbox.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(inbox.getByLabel("Sign-in code")).toHaveText(/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/);
  const code = (await inbox.getByLabel("Sign-in code").textContent())!.trim();
  await cloud.approve(code, { capabilities: ["feed:read", ...(owner ? ["account:manage"] : [])] });
  if (waitForHistory) await expect(inbox.getByRole("button", { name: /^soundwave · amber-fox-29/ })).toBeVisible({ timeout: 15_000 });
  return inbox;
}

// Opt-in delivery captures reuse the journey rather than adding a QA-only test.
async function captureReply(reader: import("@playwright/test").Page, name: string, html = false) {
  const root = process.env.QA_REPLY_ARTIFACTS;
  if (!root) return;
  const { mkdir, readFile, writeFile } = await import("node:fs/promises");
  await mkdir(root, { recursive: true });
  await reader.screenshot({ path: `${root}/${name}.png` });
  if (html) {
    const css = await readFile(new URL("../../dist/app.css", import.meta.url), "utf8");
    const dialog = await reader.locator("#operator-inbox-dialog").evaluate(el => el.outerHTML);
    await writeFile(`${root}/${name}.html`, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Reply permission</title><style>${css}</style></head><body>${dialog}</body></html>`);
  }
}

test("HUB-J19 Ask to reply upgrades the same browser after its owner approves (cas-874b)", journeyPart, async ({ page, journey, browser }) => {
  const cloud = await replyUpgradeCloud();
  await routeOperatorCloud(page.context(), cloud);
  await page.setViewportSize({ width: 390, height: 844 });
  const inbox = page.getByRole("dialog", { name: "Operator inbox" });
  const peer = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  let readerId = "";
  try {
    await journey.stage("The read-only phone asks by keyboard and keeps its conversation", async () => {
      await journey.open();
      await enrollReplyBrowser(page, cloud, "Reader phone");
      await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
      await expect(inbox.getByText("Keep this conversation.", { exact: true })).toBeVisible();
      await expect(inbox.getByRole("button", { name: "Queue reply", exact: true })).toHaveCount(0);
      await captureReply(page, "M01-read-only", true);
      readerId = [...cloud.devices.values()].find(d => d.label === "Reader phone")!.id;
      await inbox.getByRole("button", { name: "Ask to reply", exact: true }).press("Enter");
      await expect(inbox.getByText("Waiting for the account owner to allow replies. Your messages stay here.", { exact: true })).toBeVisible();
      expect(cloud.devices.get(readerId)!.scopes).toEqual([]);
      await captureReply(page, "M02-pending");
    });
    const owner = await peer.newPage();
    await owner.clock.install({ time: journeyNow() });
    await routeOperatorCloud(peer, cloud);
    await journey.stage("The owner sees the browser, origin and conversation, then allows just this reply scope", async () => {
      await owner.goto(page.url());
      const ownerInbox = await enrollReplyBrowser(owner, cloud, "Owner desktop", true);
      const requests = ownerInbox.getByRole("region", { name: "Reply requests", exact: true });
      await expect(requests.getByText(/Reader phone.*asks to reply to soundwave · amber-fox-29/)).toBeVisible({ timeout: 15_000 });
      await captureReply(owner, "M03-owner", true);
      const allow = requests.getByRole("button", { name: /Allow Reader phone/ });
      const deny = requests.getByRole("button", { name: /Deny Reader phone/ });
      const allowBox = (await allow.boundingBox())!;
      const denyBox = (await deny.boundingBox())!;
      expect(denyBox.x - allowBox.x - allowBox.width).toBeGreaterThanOrEqual(8);
      expect(await allow.evaluate(el => getComputedStyle(el).background)).not.toBe(await deny.evaluate(el => getComputedStyle(el).background));
      await allow.press("Enter");
      await expect(ownerInbox.getByText("Replying allowed for Reader phone.", { exact: true })).toBeVisible();
      expect(cloud.devices.get(readerId)!.scopes).toEqual([{ hub_id: HUB, project_id: PROJECT, session_id: sessionId, operations: ["operator_message"] }]);
    });
    await journey.stage("The existing phone can reply without signing out or pairing again", async () => {
      const reply = inbox.getByRole("textbox", { name: /Reply — soundwave/ });
      await expect(reply).toBeVisible({ timeout: 15_000 });
      await page.emulateMedia({ colorScheme: "dark" });
      await page.evaluate(() => { document.documentElement.dataset.scheme = "dark"; });
      const fieldTokens = await reply.evaluate((field) => {
        const expected = document.createElement("textarea");
        expected.style.background = "var(--panel)";
        expected.style.border = "var(--line-width) solid var(--line-strong)";
        expected.style.borderRadius = "23px";
        expected.style.color = "var(--ink)";
        expected.style.font = getComputedStyle(field).font;
        expected.style.padding = "12px 22px";
        expected.style.lineHeight = "1.55";
        expected.style.resize = "none";
        document.body.append(expected);
        const actualStyle = getComputedStyle(field);
        const expectedStyle = getComputedStyle(expected);
        const matches = {
          background: actualStyle.backgroundColor === expectedStyle.backgroundColor,
          border: actualStyle.borderColor === expectedStyle.borderColor && actualStyle.borderStyle === expectedStyle.borderStyle && actualStyle.borderWidth === expectedStyle.borderWidth,
          radius: actualStyle.borderRadius === expectedStyle.borderRadius,
          color: actualStyle.color === expectedStyle.color,
          padding: actualStyle.padding === expectedStyle.padding,
          lineHeight: actualStyle.lineHeight === expectedStyle.lineHeight,
          resize: actualStyle.resize === expectedStyle.resize,
        };
        expected.remove();
        return matches;
      });
      expect(fieldTokens).toEqual({ background: true, border: true, radius: true, color: true, padding: true, lineHeight: true, resize: true });
      const longReply = [...Array.from({ length: 11 }, (_, i) => `Reply line ${i + 1} stays inset from the rounded field edge.`), `Reply line 12: https://example.test/${"long-text-".repeat(8)}`].join("\n");
      await reply.fill(longReply);
      await expect(reply).toHaveValue(longReply);
      expect(await reply.evaluate(field => field.scrollHeight > field.clientHeight)).toBe(true);
      await captureReply(page, "F01-long-reply");
      await reply.fill("Thanks, I can reply now.");
      await inbox.getByRole("button", { name: "Queue reply", exact: true }).press("Enter");
      await expect(inbox.getByText("Thanks, I can reply now.", { exact: true })).toBeVisible();
      await captureReply(page, "M04-replied", true);
      expect(cloud.commands.size).toBe(1);
      expect(cloud.devices.size).toBe(2);
    });
    await journey.stage("Reload preserves the old history, the upgraded device and one reply", async () => {
      await page.reload();
      await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
      await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
      await expect(inbox.getByText("Keep this conversation.", { exact: true })).toBeVisible();
      await expect(inbox.getByText("Thanks, I can reply now.", { exact: true })).toBeVisible();
      await expect(inbox.getByRole("textbox", { name: /Reply — soundwave/ })).toBeVisible();
      await expect(inbox.getByRole("button", { name: "Queue reply", exact: true })).toMatchAriaSnapshot('- button "Queue reply"');
      await captureReply(page, "M05-reloaded");
      if (process.env.QA_REPLY_ARTIFACTS) {
        for (const [name, query, media] of [
          ["forced-colors", "(forced-colors: active)", { forcedColors: "active" }],
          ["reduced-motion", "(prefers-reduced-motion: reduce)", { reducedMotion: "reduce" }],
          ["contrast-more", "(prefers-contrast: more)", { contrast: "more" }],
        ] as const) {
          await page.emulateMedia(Object.assign({ forcedColors: null, reducedMotion: null, contrast: null }, media));
          expect(await page.evaluate(q => matchMedia(q).matches, query)).toBe(true);
          await expect(inbox.getByRole("textbox", { name: /Reply — soundwave/ })).toBeVisible();
          await captureReply(page, `a11y-${name}`);
        }
        await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
      }
      expect(cloud.devices.get(readerId)!.generation).toBe(2n);
      expect(cloud.log.filter(r => r.method === "POST" && r.path === "/api/operator/enrollments")).toHaveLength(2);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    });
  } finally { await peer.close(); }
});

test("HUB-J19 Ask to reply names denial without granting permission (cas-874b)", journeyPart, async ({ page, journey, browser }) => {
  const cloud = await replyUpgradeCloud();
  await routeOperatorCloud(page.context(), cloud);
  const inbox = page.getByRole("dialog", { name: "Operator inbox" });
  const peer = await browser.newContext();
  try {
    await journey.stage("Reader asks and the owner denies", async () => {
      await journey.open(); await enrollReplyBrowser(page, cloud, "Denied phone");
      await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
      await inbox.getByRole("button", { name: "Ask to reply", exact: true }).click();
      const owner = await peer.newPage(); await owner.clock.install({ time: journeyNow() });
      await routeOperatorCloud(peer, cloud); await owner.goto(page.url());
      const ownerInbox = await enrollReplyBrowser(owner, cloud, "Owner", true);
      await ownerInbox.getByRole("button", { name: /Deny Denied phone/ }).click();
      await expect(inbox.getByText("The owner denied this reply request. This browser is still read-only.", { exact: true })).toBeVisible({ timeout: 15_000 });
      await expect(inbox.getByRole("button", { name: "Queue reply", exact: true })).toHaveCount(0);
      await captureReply(page, "F01-denied");
    });
    // The denied reader can request again; no expiry setup is needed to
    // prove that transition (cas-33167). Keep the denied stage capture above.
    await inbox.getByRole("button", { name: "Ask to reply", exact: true }).click();
    await expect(inbox.getByText(/Waiting for the account owner/)).toBeVisible();
  } finally { await peer.close(); }
});

// Expiry is independent of denial and needs only the requesting browser.
// It must not spend its test budget enrolling a separate owner (cas-33167).
test("HUB-J19 Ask to reply names expiry without granting permission (cas-874b)", journeyPart, async ({ page, journey }) => {
  const cloud = await replyUpgradeCloud();
  await routeOperatorCloud(page.context(), cloud);
  const inbox = page.getByRole("dialog", { name: "Operator inbox" });
  await journey.stage("Reader requests permission", async () => {
    await journey.open(); await enrollReplyBrowser(page, cloud, "Expiring phone");
    await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
    await inbox.getByRole("button", { name: "Ask to reply", exact: true }).click();
    await expect(inbox.getByText(/Waiting for the account owner/)).toBeVisible();
  });
  await journey.stage("An expired request is shown honestly after a reload", async () => {
    const pending = [...cloud.replyGrants.requests.values()].find(r => r.status === "pending")!;
    // Only the server double's deadline changes; no browser sleeps/clock advance.
    pending.expires_at = new Date(cloud.now() - 1).toISOString();
    await page.reload(); await page.getByRole("button", { name: "Operator inbox", exact: true }).click();
    await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
    await expect(inbox.getByText("This reply request expired. This browser is still read-only.", { exact: true })).toBeVisible();
    await captureReply(page, "F02-expired");
    await expect(inbox.getByRole("button", { name: "Ask to reply", exact: true })).toBeVisible();
    await expect(inbox.getByRole("button", { name: "Queue reply", exact: true })).toHaveCount(0);
  });
});

test("HUB-J19 unavailable or unshipped reply permissions never block history (cas-6f38)", journeyPart, async ({ page, journey }) => {
  const cloud = await replyUpgradeCloud(false);
  let grantsUnavailable = true;
  let releaseHistory!: () => void;
  const historyHeld = new Promise<void>(resolve => { releaseHistory = resolve; });
  // cas-7f99: legacy introspection without origin must still replay history.
  const handle = cloud.handle.bind(cloud);
  cloud.handle = async request => {
    if (request.path.startsWith("/api/operator/feed?")) await historyHeld;
    const result = await handle(request);
    if (request.path === "/api/operator/grants/me" && result.status === 200) {
      if (grantsUnavailable) { result.status = 500; result.body = { error: "internal_error" }; }
      else delete (result.body as { grant: Record<string, unknown> }).grant.origin;
    }
    return result;
  };
  await routeOperatorCloud(page.context(), cloud);
  await journey.stage("Reply permissions fail but history loads, with no false empty state or dead control", async () => {
    await journey.open(); const inbox = await enrollReplyBrowser(page, cloud, "Read only", false, false);
    await expect(inbox.getByText("Loading inbox history…", { exact: true })).toBeVisible({ timeout: 15_000 });
    await expect(inbox.getByText(/^No messages yet/)).toHaveCount(0);
    releaseHistory();
    await expect(inbox.getByRole("button", { name: /soundwave · amber-fox-29/ })).toBeVisible({ timeout: 15_000 });
    await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
    await expect(inbox.getByRole("button", { name: "Ask to reply", exact: true })).toHaveCount(0);
    await expect(inbox.getByText(/This browser can read this conversation but not reply/)).toBeVisible();
    await cloud.appendSessionEvent({ hubId: HUB, projectId: PROJECT, sessionId,
      plaintext: supervisorTurn(799, "New history after grant introspection.", journeyDay(1, 13)) });
    await expect(inbox.getByText("New history after grant introspection.", { exact: true })).toBeVisible({ timeout: 15_000 });
    await captureReply(page, "M06-grants500");
  });
  await journey.stage("An older Cloud can recover without enabling unsupported replies", async () => {
    grantsUnavailable = false;
    const inbox = page.getByRole("dialog", { name: "Operator inbox" });
    await cloud.appendSessionEvent({ hubId: HUB, projectId: PROJECT, sessionId,
      plaintext: supervisorTurn(800, "History after reply-permission recovery.", journeyDay(1, 14)) });
    await expect(inbox.getByText("History after reply-permission recovery.", { exact: true })).toBeVisible({ timeout: 15_000 });
    await expect(inbox.getByRole("button", { name: "Ask to reply", exact: true })).toHaveCount(0);
    expect(cloud.log.some(r => r.path === "/api/operator/grants/me")).toBe(true);
    expect(cloud.log.some(r => r.path.includes("reply-grant-requests"))).toBe(false);
    await captureReply(page, "F03-unshipped");
  });
});
