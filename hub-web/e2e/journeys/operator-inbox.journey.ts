import { createHash } from "node:crypto";
import { test, expect, journeyPart } from "./journey";
import { journeyDay, journeyNow } from "./clock";
import { operatorCloudDouble, routeOperatorCloud } from "./operator-cloud-route";

const SESSION = "cas-src-amber-fox-29";
const HUB = "hub-soundwave";
const PROJECT = "github.com/richards-llc/cassy";
const sessionId = `s_${createHash("sha256").update(SESSION).digest("base64url")}`;

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
