import { createHash } from "node:crypto";
import { test, expect } from "./journey";
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
    await page.getByRole("button", { name: "Inbox", exact: true }).click();
    await expect(inbox.getByText("this isn’t end-to-end encryption")).toBeVisible();
    await inbox.getByLabel("Name this browser").fill("Pixel 9");
    await inbox.getByRole("button", { name: "Sign in" }).click();
    await expect(inbox.getByLabel("Sign-in code")).toHaveText(/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/);
    await expect(inbox.getByRole("link", { name: "Approve on Petra Stella Cloud" })).toHaveAttribute("href", /\/operator\/approve\?code=/);
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

  await journey.stage("Reply while soundwave is off: it waits as Pending machine", async () => {
    await inbox.getByLabel(/Reply — soundwave gets it when it’s back/).fill("Go. Cut the release.");
    await inbox.getByRole("button", { name: "Queue reply" }).click();
    await expect(inbox.getByText("Pending machine")).toBeVisible();
    expect([...cloud.commands.values()].map((command) => [command.machineId, command.status])).toEqual([[soundwave.id, "pending_machine"]]);
  });

  await journey.stage("A second browser profile sees the history and the queued reply", async () => {
    const desktop = await browser.newContext({ viewport: { width: 1280, height: 800 }, timezoneId: "UTC" });
    const other = await desktop.newPage();
    await other.clock.install({ time: journeyNow() });
    await routeOperatorCloud(desktop, cloud);
    await other.goto(page.url());
    await other.getByRole("button", { name: "Inbox", exact: true }).click();
    const otherInbox = other.getByRole("dialog", { name: "Operator inbox" });
    await otherInbox.getByLabel("Name this browser").fill("Desk");
    await otherInbox.getByRole("button", { name: "Sign in" }).click();
    const code = (await otherInbox.getByLabel("Sign-in code").textContent())!.trim();
    await cloud.approve(code);
    await otherInbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click({ timeout: 15_000 });
    await expect(otherInbox.getByText("Cloud half shipped. Ready for your go.")).toBeVisible();
    await expect(otherInbox.getByText("Go. Cut the release.")).toBeVisible();
    // This profile cannot queue replies: it was approved for reading only.
    await expect(otherInbox.getByText(/can read this conversation but not leave replies/)).toBeVisible();
    const desk = [...cloud.devices.values()].find((device) => device.label === "Desk")!;
    expect(cloud.acks.get(desk.id)?.size).toBe(4);
    await desktop.close();
  });

  await journey.stage("soundwave returns and accepts the reply; a reload keeps everything", async () => {
    const [command] = cloud.commands.keys();
    cloud.acceptCommand(command);
    await expect(inbox.getByText("Accepted by machine")).toBeVisible({ timeout: 15_000 });
    await page.reload();
    await page.getByRole("button", { name: "Inbox", exact: true }).click();
    await inbox.getByRole("button", { name: /soundwave · amber-fox-29/ }).click();
    await expect(inbox.getByText("Started the release epic. Three lanes.")).toBeVisible();
    await expect(inbox.getByText("Accepted by machine")).toBeVisible();
    // Signed in once: the reload neither asked again nor replayed history twice.
    expect(cloud.enrollments.size).toBe(2);
  });
});
