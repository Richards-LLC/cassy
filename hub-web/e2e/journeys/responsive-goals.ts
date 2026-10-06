import type { Locator, Page } from "@playwright/test";
import { isPhoneLayout } from "../../src/viewport";
import { expect, type Journey } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

/** Match the product's real layout, including a rotated touch device. */
export async function phoneLayout(page: Page): Promise<boolean> {
  // Before navigation a mobile about:blank has the default 980px layout
  // viewport. Use the configured viewport and the product's same predicate;
  // after the meta viewport loads these are the actual CSS dimensions.
  const viewport = page.viewportSize()!;
  return isPhoneLayout({ ...viewport, coarsePointer: await page.evaluate(() => matchMedia("(pointer: coarse)").matches) });
}
export async function activate(page: Page, control: Locator): Promise<void> {
  if (await page.evaluate(() => navigator.maxTouchPoints > 0)) await control.tap();
  else await control.click();
}
export async function showConversationList(page: Page): Promise<void> {
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  if (!await list.isVisible()) {
    const back = page.getByRole("button", { name: "‹ Conversations", exact: true });
    await expect(back).toBeVisible();
    await activate(page, back);
  }
  await expect(list).toBeVisible();
}
export async function openConversation(page: Page, project: string): Promise<void> {
  await showConversationList(page);
  await activate(page, page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: new RegExp(`^${project} on `) }));
  await expect(page.getByRole("button", { name: `Send to the ${project} supervisor`, exact: true })).toBeVisible();
}
export async function declaredChoice(page: Page, name: string): Promise<Locator> {
  const choice = page.getByRole("button", { name, exact: true }).filter({ visible: true });
  const expand = page.locator(".pinned-expand").filter({ visible: true });
  // Wait for the arriving question's actual control, whether its declared
  // choice is in flow or its phone card is folded beside a focused composer.
  await expect(choice.or(expand).first()).toBeVisible();
  if (!await choice.isVisible()) await activate(page, expand);
  await expect(choice).toBeVisible();
  return choice;
}

export async function phoneFind(page: Page, journey: Journey): Promise<void> {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const search = page.getByRole("searchbox", { name: "Search conversations" });
  let ask = 0;
  await journey.stage("On the phone, search and open a conversation without desktop keyboard hints", async () => {
    await journey.open();
    await expect(list).toBeVisible();
    await expect(search).toHaveAttribute("placeholder", "Search conversations");
    await search.fill("atlas");
    await expect(list.getByRole("button")).toHaveCount(1);
    await openConversation(page, "cas-src");
    await expect(list).toBeHidden();
    await expect(page.locator(".conversation-identity h1")).toHaveText("cas-src");
  });
  await journey.stage("A question arrives while reading another machine", async () => {
    await openConversation(page, "gabber-studio");
    ask = hub.supervisorSays(PELICAN, "Which lane should go first?", { kind: "ask", options: ["Linux first", "Mac first"] });
    await showConversationList(page);
    await expect(list.getByRole("button", { name: /cas-src/ }).getByRole("img", { name: "Waiting for you", exact: true })).toBeVisible();
    await search.fill("cas-src");
    await expect(list.getByRole("button")).toHaveCount(1);
    await openConversation(page, "cas-src");
    await expect(page.getByRole("log")).toContainText("Which lane should go first?");
  });
  await journey.stage("Answer the waiting conversation and return to the phone list", async () => {
    const sent = hub.nextSend();
    await activate(page, await declaredChoice(page, "Linux first"));
    expect(await sent).toMatchObject({ machine: "atlas", target: PELICAN, text: "Linux first", in_reply_to: ask });
    await showConversationList(page);
    await expect(list.getByRole("button", { name: /cas-src/ }).getByRole("img", { name: "Waiting for you", exact: true })).toHaveCount(0);
    await expect(search).toHaveValue("");
    await expect(list.getByRole("button")).toHaveCount(2);
  });
}

export async function phoneReply(page: Page, journey: Journey): Promise<void> {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const composer = page.getByRole("textbox", { name: "Your message" });
  await journey.stage("Write on the phone and keep the draft through a heartbeat", async () => {
    await journey.open(); await openConversation(page, "cas-src");
    await composer.fill("Please verify the gate first.");
    await composer.evaluate((field: HTMLTextAreaElement) => {
      field.setSelectionRange(7, 7);
      (window as unknown as { __phoneDraft: HTMLTextAreaElement }).__phoneDraft = field;
    });
    await hub.announceCatalog("atlas");
    await expect(composer).toHaveValue("Please verify the gate first.");
    await expect(composer).toBeFocused();
    expect(await composer.evaluate((field: HTMLTextAreaElement) => ({ same: field === (window as unknown as { __phoneDraft: HTMLTextAreaElement }).__phoneDraft, caret: field.selectionStart }))).toEqual({ same: true, caret: 7 });
  });
  await journey.stage("Use Back to visit another conversation, then restore the draft", async () => {
    await openConversation(page, "gabber-studio");
    await expect(composer).toHaveValue("");
    await openConversation(page, "cas-src");
    await expect(composer).toHaveValue("Please verify the gate first.");
    expect(await composer.evaluate((field: HTMLTextAreaElement) => field.selectionStart)).toBe(7);
    await page.reload();
    await openConversation(page, "cas-src");
    await expect(composer).toHaveValue("Please verify the gate first.");
  });
  await journey.stage("Tap Send and read the correlated supervisor reply on this phone", async () => {
    const sent = hub.nextSend();
    await activate(page, page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }));
    expect(await sent).toMatchObject({ machine: "atlas", target: PELICAN, text: "Please verify the gate first." });
    const queued = hub.deliverLatest(PELICAN);
    await expect(page.locator('.conversation-turn[data-state="acknowledged"]')).toBeVisible();
    hub.answerQueued(PELICAN, queued, "Verified. The gate is green.");
    await expect(page.getByRole("log").getByText("Verified. The gate is green.")).toBeVisible();
    await expect(composer).toHaveValue("");
    await expect(page.locator('.conversation-turn[data-state="replied"]')).toHaveCount(1);
    await expect(page.getByRole("navigation", { name: "Choose a supervisor" })).toBeHidden();
  });
}

export async function phoneSwitch(page: Page, journey: Journey): Promise<void> {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const composer = page.getByRole("textbox", { name: "Your message" });
  await journey.stage("Start a draft on Atlas with the list replaced by the phone conversation", async () => {
    await journey.open(); await openConversation(page, "cas-src");
    await expect(page.getByRole("button", { name: "‹ Conversations", exact: true })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Choose a supervisor" })).toBeHidden();
    await composer.fill("Draft for Atlas only");
  });
  await journey.stage("Navigate Back, choose Studio, and send only to its supervisor", async () => {
    await openConversation(page, "gabber-studio");
    await expect(page.locator(".conversation-host")).toContainText("Studio Mac");
    await expect(composer).toHaveValue("");
    await composer.fill("Is the Mac build green?");
    const sent = hub.nextSend();
    await activate(page, page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true }));
    expect(await sent).toMatchObject({ machine: "studio", target: OTTER, text: "Is the Mac build green?" });
    hub.answerLatest(OTTER, "The Mac build is green.");
    await expect(page.getByRole("log").getByText("The Mac build is green.")).toBeVisible();
  });
  await journey.stage("Back to Atlas preserves its draft and reload preserves the phone route", async () => {
    await openConversation(page, "cas-src");
    await expect(composer).toHaveValue("Draft for Atlas only");
    await page.reload(); await openConversation(page, "cas-src");
    await expect(composer).toHaveValue("Draft for Atlas only");
    await expect(page.locator(".conversation-host")).toContainText("Atlas");
    expect(hub.sends.filter(send => send.machine === "atlas")).toHaveLength(0);
  });
}

export async function phoneAsk(page: Page, journey: Journey): Promise<void> {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  let ask = 0;
  await journey.stage("Read the actual question on the phone, opening its folded form when needed", async () => {
    await journey.open(); await openConversation(page, "cas-src");
    await page.getByRole("textbox", { name: "Your message" }).focus();
    ask = hub.supervisorSays(PELICAN, "Fix the gate in this patch or send it back for review?", { kind: "ask", options: ["Fix the gate", "Send for review"] });
    await declaredChoice(page, "Fix the gate");
    await expect(page.getByRole("log")).toContainText("Fix the gate in this patch");
    await expect(page.getByRole("button", { name: "Send for review", exact: true }).filter({ visible: true })).toBeVisible();
  });
  await journey.stage("Tap the declared choice once and observe its exact reply target", async () => {
    const sent = hub.nextSend();
    await activate(page, await declaredChoice(page, "Fix the gate"));
    expect(await sent).toMatchObject({ text: "Fix the gate", in_reply_to: ask, machine: "atlas", target: PELICAN });
    await expect(page.getByRole("log").locator('.obj.t-a[data-answered="true"]')).toContainText("Fix the gate");
    expect(hub.sends.filter(send => send.in_reply_to === ask)).toHaveLength(1);
    await expect(page.getByRole("button", { name: "Send for review", exact: true })).toHaveCount(0);
    hub.answerLatest(PELICAN, "Fixing the gate now.");
    await expect(page.getByRole("log").getByText("Fixing the gate now.")).toBeVisible();
  });
  await journey.stage("Reload and keep the answered question quiet on the phone", async () => {
    await page.reload(); await openConversation(page, "cas-src");
    await expect(page.getByRole("log")).toContainText("Fix the gate");
    await expect(page.getByRole("button", { name: "Send for review", exact: true })).toHaveCount(0);
    await showConversationList(page);
    await expect(page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("img", { name: "Waiting for you", exact: true })).toHaveCount(0);
  });
}

export async function phoneDark(page: Page, journey: Journey): Promise<void> {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  await journey.stage("Read a supervisor message, then use Back to reach Appearance on the phone", async () => {
    await journey.open(); await openConversation(page, "cas-src");
    hub.supervisorSays(PELICAN, "Status: the Linux lane is green; the Mac lane is still running.", { kind: "status" });
    await expect(page.getByRole("log")).toContainText("Linux lane is green");
    await showConversationList(page);
    await activate(page, page.getByRole("button", { name: "Appearance & commands", exact: true }));
    await expect(page.locator('#command-palette [data-palette-scheme="system"]')).toHaveAttribute("aria-current", "true");
    await activate(page, page.getByRole("button", { name: /^Appearance · Dark/ }));
    await expect(page.locator("html")).toHaveAttribute("data-scheme", "dark");
  });
  await journey.stage("Return to the conversation and keep reading after a dark-theme reload", async () => {
    await openConversation(page, "cas-src");
    await expect(page.getByRole("log")).toContainText("Linux lane is green");
    await page.reload(); await openConversation(page, "cas-src");
    await expect(page.locator("html")).toHaveAttribute("data-scheme", "dark");
    await expect(page.getByRole("log")).toContainText("Linux lane is green");
  });
  await journey.stage("Forced colors preserve the phone list and its native Send control", async () => {
    await page.emulateMedia({ forcedColors: "active" });
    expect(await page.evaluate(() => matchMedia("(forced-colors: active)").matches)).toBe(true);
    await showConversationList(page);
    await expect(page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ })).toBeVisible();
    await openConversation(page, "cas-src");
    const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
    const style = await send.evaluate(element => { const css = getComputedStyle(element); return { border: css.borderTopStyle, color: css.color, background: css.backgroundColor }; });
    expect(style.border).toBe("solid"); expect(style.background).not.toBe(style.color);
    await expect(page.getByRole("log")).toContainText("Linux lane is green");
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeVisible();
    await page.emulateMedia({ forcedColors: null });
  });
}

export async function phonePairLink(page: Page, journey: Journey, token: string, earlier: string): Promise<void> {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO] });
  const dialog = page.locator("#pair-dialog");
  await journey.stage("Open the phone invitation and read what its pairing permits", async () => {
    await page.goto(`./#pair=${earlier}&hub=studio&hub_url=https%3A%2F%2Fstudio.test&machine=Studio%20Mac&scopes=machine:read,session:read,pane:read`);
    await expect(dialog.getByRole("textbox", { name: /Machine name/ })).toHaveValue("Studio Mac");
    await expect(dialog.locator(".pair-lead").first()).toHaveText("This browser will be able to: Read sessions and terminals");
    await expect(dialog.locator(".pair-withheld")).toContainText("This link does not let it: Type, send messages and interrupt");
    await expect(dialog.getByRole("button", { name: "Copy command" })).toBeInViewport({ ratio: 1 });
    await expect(dialog.locator(".pair-withheld-command code")).toHaveText(/^cas hub pair --origin \S+ --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt$/);
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
    // The native phone goal reads the visible grant language. Raw disclosure
    // inspection stays in the desktop scenario; its touch defect is cas-207a.
  });
  await journey.stage("Open the newer control link and confirm this machine, preserving my name", async () => {
    await page.evaluate(hash => { location.hash = hash; }, `pair=${token}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas%20%C2%B7%20Linux&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
    await expect(dialog.getByRole("textbox", { name: /Machine's hub address/ })).toHaveValue("https://atlas.test");
    await expect(dialog.getByRole("textbox", { name: /Machine name/ })).toHaveValue("Atlas · Linux");
    await expect(dialog.getByRole("textbox", { name: "Your name (shown on the machine)" })).toHaveValue("Daniel");
    await expect(dialog.locator(".pair-lead").first()).toHaveText("This browser will be able to: Read sessions and terminals · Type, send messages and interrupt");
    expect(new URL(page.url()).hash, "invitation leaves the address bar").toBe("");
    await activate(page, dialog.getByRole("button", { name: "Pair", exact: true }));
    await expect(dialog).toBeHidden();
    expect(hub.exchanges).toHaveLength(1);
    expect(hub.exchanges[0]).toMatchObject({ token, hub_id: "atlas", operator_label: "Daniel", requested_scopes: ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"] });
    expect(hub.exchangeOrigins).toEqual(["https://atlas.test"]);
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Choose a supervisor" })).toBeHidden();
  });
  await journey.stage("Contact the paired supervisor by tapping Send on this phone", async () => {
    await page.getByRole("textbox", { name: "Your message" }).fill("Can you read this phone's message?");
    const sent = hub.nextSend();
    await activate(page, page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }));
    expect(await sent).toMatchObject({ machine: "atlas", target: PELICAN, text: "Can you read this phone's message?" });
    hub.answerLatest(PELICAN, "Paired. I can read your phone's message.");
    await expect(page.getByRole("log")).toContainText("Paired. I can read your phone's message.");
    await expect(page.getByRole("textbox", { name: "Your message" })).toHaveValue("");
  });
}
