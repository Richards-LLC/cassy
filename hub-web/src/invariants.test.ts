import { createHash, webcrypto } from "node:crypto";
import { readFile } from "node:fs/promises";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, TransientAuthError, type ConnectionState, type HubCallbacks } from "./connection";
import { connectingView } from "./connection-state-view";
import { createDeviceKey, dpopHeaders } from "./dpop";
import { consumePairingFragment } from "./fragment";
import type { StoredMachine } from "./types";

Object.defineProperty(globalThis, "crypto", { value: webcrypto, configurable: true });

// Retained historical source pins are inventoried in cas-9d89. They are not
// behavioural proof: the comments identify their consumers and remaining gaps.
// "main.ts" includes the builders used by that application's shell.
const APP_SOURCES = ["main.ts", "composer-markup.ts", "pair-dialog-markup.ts", "conversation-shell.ts"];
async function readSource(path: string): Promise<string> {
  if (path !== "main.ts") return readFile(new URL(path, import.meta.url), "utf8");
  return (await Promise.all(APP_SOURCES.map((source) => readFile(new URL(source, import.meta.url), "utf8")))).join("\n");
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("binding Cassy Cloud browser invariants", () => {
  it("H4-CATALOG-01 consumes pairing fragments synchronously and preserves no capability in the URL", () => {
    const token = "A".repeat(43);
    let replacement = "";
    const location = { hash: `#pair=${token}&hub=machine-1`, pathname: "/", search: "" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;
    expect(consumePairingFragment(location, history)).toEqual({ token, hubId: "machine-1" });
    expect(replacement).toBe("/");
    expect(replacement).not.toContain(token);
  });

  // Contract: offers exactly one primary action without an invitation and never a Pair control (cas-8051 F7).
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("offers exactly one primary action without an invitation and never a Pair control (cas-8051 F7)", async () => {
    const source = await readSource("main.ts");
    const html = await readFile(new URL("../index.html", import.meta.url), "utf8");
    // The entry step used to render a disabled Pair beside Create pairing code
    // and explain the disabled button in prose. The stronger property: with no
    // invitation there is no Pair/submit control in the dialog at all, the
    // link path is named as the alternative, and Pair exists only on the
    // confirmation form an invitation opens directly.
    const entry = source.slice(source.indexOf("// One state, one next action."), source.indexOf("* Render the six scopes against the invitation's ceiling."));
    expect(entry).toContain('<h2 id="pair-title">Pair a machine</h2>');
    expect(entry).not.toContain(">Pair</button>");
    expect(entry).not.toContain('type="submit"');
    expect(entry).not.toContain("pairing-disabled-reason\">Pair is disabled");
    expect(entry).toContain("Open the pairing URL that <code>cas hub pair</code> printed on the machine; it continues straight to confirmation.");
    expect(source).toContain('<button type="submit" class="primary" ${pairingExchangeInFlight ? "disabled" : ""}>${pairingExchangeInFlight ? "Pairing…" : "Pair"}</button></div></form></dialog>');
    expect(source).toContain('id="pair-create" type="button" class="primary"');
    expect(source).toContain('pairingCreateInFlight ? "Creating…" : "Create pairing code"');
    expect(source).toContain("const relayAction = relayOrigin");
    expect(html).toContain('name="cas-pairing-relay-origin" content="https://petra-stella-cloud.vercel.app"');
    expect(html).toContain("<title>Cassy Cloud</title>");
  });

  // Contract: declares the Cassy Cloud favicon from the static web source.
  // Consumer: Browser document icon loader consumes index.html favicon link and public/favicon.svg.
  // Structural contract retained.
  it("declares the Cassy Cloud favicon from the static web source", async () => {
    const [html, favicon] = await Promise.all([
      readFile(new URL("../index.html", import.meta.url), "utf8"),
      readFile(new URL("../public/favicon.svg", import.meta.url), "utf8"),
    ]);
    expect(html).toContain('<link rel="icon" type="image/svg+xml" href="/favicon.svg" />');
    expect(favicon).toContain('docs/assets/cassy-logo.png');
    expect(favicon.match(/<path /g)).toHaveLength(3);
    expect(favicon).not.toContain('<text');
  });

  // Contract: asks for the machine's hub address instead of seeding the page origin (cas-8051 F5).
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("asks for the machine's hub address instead of seeding the page origin (cas-8051 F5)", async () => {
    const [main, draft] = await Promise.all(["main.ts", "pairing-draft.ts"].map((path) => readSource(path)));
    // Only the machine's own link may seed the address, never this page (HUB-J2).
    expect(draft).toContain('hubUrl: prefill.suggestedHubUrl ?? "",');
    expect(draft).not.toContain("hubUrl: controllerOrigin");
    expect(draft).toContain("pageOrigin: controllerOrigin,");
    expect(main).toContain("<label>Machine's hub address<input name=\"url\" type=\"url\" required${focus(\"url\")} placeholder=");
    expect(main).toContain("It is not this page's address unless this page is served by that machine.");
    expect(main).toContain('<summary>Where do I find this?</summary>');
    expect(main).toContain('id="pair-use-page-origin" type="button" class="secondary"');
    expect(main).toContain('detailRow("Machine\'s hub address", hubUrl)');
    // Consent leads with the plain summary; the exact origin and scope list stay one tap away (F3).
    expect(main).toContain('<p class="pair-lead">This browser will be able to: <strong class="pair-summary">');
    expect(main).toContain('<summary>Technical details</summary>');
    expect(main).toContain('detailRow("Exact scopes", exactScopes(');
    expect(main).toContain('detailRow("Granted scopes", exactScopes(invitationScopes))');
  });

  // Contract: never leaves the command palette flagged open after it closes (cas-dfc8).
  // Consumer: Commander application render and event handlers (main.ts), operating the command palette.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("never leaves the command palette flagged open after it closes (cas-dfc8)", async () => {
    const source = await readSource("main.ts");
    // Any close of the palette settles the flag render() reopens it from.
    expect(source).toMatch(/palette\.onclose = \(\) => \{\s*if \(!palette\.isConnected \|\| palette\.open\) return;\s*commandPaletteOpen = false;/);
    // cas-3c25: closing in place adopts the closed-palette signature, so the
    // next unrelated render does not rebuild the shell under the operator's focus.
    expect(source).toContain("if (lastShellSignatureWithPaletteClosed !== undefined) lastShellSignature = lastShellSignatureWithPaletteClosed;");
    expect(source).toContain("lastShellSignatureWithPaletteClosed = shellSignature({ ...signatureParts, commandPaletteOpen: false }) + signatureTail;");
    // Paired machines replaces the palette and clears the flag itself too.
    // cas-460a: it also remembers its opener so its close can hand focus back.
    expect(source).toContain("const open = (opener: string) => { pairedMachinesOpener = opener; commandPaletteOpen = false; document.querySelector<HTMLDialogElement>('#command-palette')?.close(); dialog.showModal(); };");
    // No other code closes the palette dialog behind the flag's back.
    const closes = source.match(/#command-palette['"]\)\?\.close\(\)/g) ?? [];
    expect(closes).toHaveLength(1);
  });

  // Contract: names the remedy when a credential cannot interrupt (cas-0546, was: observer-only control).
  // Consumer: Commander application render and event handlers (main.ts), operating the conversation header.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("names the remedy when a credential cannot interrupt", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain("This browser was paired without permission to interrupt. Run cas hub pair --origin ${location.origin} on ${machine.label}");
    expect(source).toContain('detailRow("Cassy Cloud origin", ');
    // A phone cannot hover, so an unavailable action keeps its reason in the
    // DOM, is described by it, and says it out loud when tapped.
    expect(source).toContain('applyActionAvailability(document.querySelector<HTMLButtonElement>("#conversation-interrupt"), document.querySelector<HTMLElement>("#conversation-interrupt-reason"), interruptUnavailableReason());');
    expect(source).toContain('button.setAttribute("aria-disabled", "true");');
    expect(source).toContain("if (reason) { toast(reason, { thread, until: () => interruptUnavailableReason() !== reason }); return; }");
    expect(source).not.toContain('disabled aria-describedby="conversation-interrupt-reason"');
  });

  // Contract: answers a supervisor ask through the leased send path with in_reply_to and pins it above the composer (cas-43f9).
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("answers a supervisor ask through the leased send path with in_reply_to and pins it above the composer (cas-43f9)", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain("installAttentionObjects();");
    expect(source).toContain("respond: (ask, text) => { void submitSupervisorMessage({ text, replyTo: ask.notification_id }); },");
    expect(source).toContain("const replyTo = quick ? quick.replyTo : (selectedThread ? conversationHistory(selectedThread).pinnedAsk()?.notification_id : undefined);");
    expect(source).toContain("deliverSupervisorMessage(machine, session, supervisor, text, replyTo, quick?.retryOf, editOf);");
    // An edited resend of a refused message retires the original (F6): no Retry left to resend corrected text.
    expect(source).toContain("if (editOf) { history.retireRefused(editOf); editingRefused = undefined; }");
    expect(source).toContain("const editOf = !quick && editingRefused?.threadKey === selectedThread");
    // The composer speaks plain words, never protocol vocabulary (F5).
    expect(source).not.toContain("awaiting receipt");
    // The reason is said once, on the refused bubble; the composer points at it (cas-4d92).
    expect(source).toContain("showComposerStatus(onBubble ? REFUSED_SEE_ABOVE : refusalSentence(detail), \"error\");");
    // The list preview never shows unsent text as said (F6).
    expect(source).toContain("preview: conversationHistories.get(key)?.preview(),");
    // Retry of a refused send (cas-b1ee): same leased path, the refused send's own in_reply_to.
    expect(source).toContain("retryMessage: (send) => { void submitSupervisorMessage({ text: send.text, replyTo: send.replyTo, retryOf: send.id }); },");
    expect(source).toMatch(/if \(retryOf\) \{[\s\S]*?history\.discardRefused\(retryOf\);/);
    expect(source).toContain("supervisorMessage(held.supervisor, held.text, held.clientRef, held.replyTo)");
    expect(source).toContain("holdSupervisorMessage(machine, session, clientRef, supervisor, text, replyTo);");
    expect(source).toContain("composerSlot.prepend(conversation.pinned);");
    // The list's waiting affordance is driven by unanswered asks and blockers.
    expect(source).toContain("const waiting = waitingOnOperator(conversationHistories.get(key));");
    expect(source).toContain("attention: waiting,");
  });

  // Contract: detects a phone from one definition, in both orientations.
  // Consumer: Commander application render and event handlers (main.ts), operating the phone shell.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("detects a phone from one definition, in both orientations", async () => {
    const [main, css, design] = await Promise.all(["main.ts", "styles.css", "../DESIGN.md"].map((path) => readSource(path)));
    // A rotated Pixel 7 is 915px wide, so a width-only breakpoint handed a
    // 412px-tall screen the three-column desktop console (report defect D5).
    // CSS and JS must ask the identical question, or rotation puts the layout
    // and the phone sheets in different modes.
    expect(main).toContain('import { PHONE_MEDIA_QUERY } from "./viewport";');
    expect(main).toContain("function phoneLayout(): boolean { return window.matchMedia(PHONE_MEDIA_QUERY).matches; }");
    // Every viewport question is asked with a shared query string, so no literal
    // breakpoint can drift out of step with the stylesheet again.
    expect(main).not.toContain("max-width: 850px");
    expect(main).not.toContain('matchMedia("(max-width');
    // Rotation flips the layout in CSS instantly; the phone sheets are decided
    // in JS at render time and must follow it.
    expect(main).toContain('window.matchMedia(PHONE_MEDIA_QUERY).addEventListener("change", () => render());');
    expect(css).toContain("@media (max-width: 53rem), (max-height: 30rem) and (pointer: coarse) {");
    expect(design).toContain("(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)");
    expect(design).toContain("landscape");
  });

  // Contract: sends the supervisor message from Enter and from the button, through one path.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("sends the supervisor message from Enter and from the button, through one path", async () => {
    const source = await readSource("main.ts");
    // Enter was never wired: it inserted a newline and sent nothing, in observe
    // mode and in control mode alike (measured against the live hub, cas-0d61).
    expect(source).toContain("composer.onkeydown = (event) => {");
    expect(source).toContain("if (!sendsOnEnter(event)) return;");
    expect(source).toContain("void submitSupervisorMessage();");
    expect(source).toContain('document.querySelector<HTMLButtonElement>("#message-send")!.onclick = () => { void submitSupervisorMessage(); };');
    expect(source).toContain("async function submitSupervisorMessage(quick?: { text: string; replyTo?: number; retryOf?: string }): Promise<void> {");
    expect(source).toContain("const plan = planSupervisorSend(supervisorSendContext(text));");
  });

  // Contract: a deferred structural change flushes after focus/gestures end.
  // Consumer: main.ts's app event handlers and DeferredRenderScheduler.
  // The real heartbeat/draft path is exercised by HUB-J5; gesture wiring still
  // needs a built-dist regression (deferred-render-dom.test.ts tests the scheduler).
  it("wires deferred shell flushes to focus and pointer gestures", async () => {
    const main = await readSource("main.ts");
    expect(main).toContain("deferredRender.defer();");
    expect(main).toContain('app.addEventListener("focusout"');
    expect(main).toContain('app.addEventListener("pointerdown", () => deferredRender.gestureStarted(), true);');
    // A touch's click follows its pointerup in a later task (cas-207a).
    expect(main).toContain('app.addEventListener("pointerup", (event) => { if (event.pointerType === "touch") deferredRender.touchEnded(); else deferredRender.gestureEnded(); }, true);');
    expect(main).toContain('app.addEventListener("click", () => deferredRender.clicked(), true);');
    expect(main).toContain("touchWindow: (run) => window.setTimeout(run, 600),");
    expect(main).toContain('app.addEventListener("pointercancel", () => deferredRender.gestureCancelled(), true);');
    expect(main).toContain("afterGesture: (run) => window.setTimeout(run, 0),");
    // cas-3c25 (HUB-J18): a keyboard Tab is mid-transit during focusout, so
    // the owed rebuild waits a task for focus to settle, then restores the
    // rebuilt control with its ring.
    expect(main).toMatch(/app\.addEventListener\("focusout", \(\) => \{[\s\S]{0,600}?window\.setTimeout\(\(\) => \{/);
    expect(main).not.toMatch(/app\.addEventListener\("focusout", \(\) => \{\s*queueMicrotask/);
    expect(main).toContain("focus({ preventScroll: true, focusVisible: focusedControlVisible } as FocusOptions)");
  });

  // Contract: keeps pairing failures inside the open dialog and cancellation cleanup visible (cas-7d55 F1/F2/F3/F6).
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("keeps pairing failures inside the open dialog and cancellation cleanup visible (cas-7d55 F1/F2/F3/F6)", async () => {
    const [main, model] = await Promise.all(["main.ts", "render-model.ts"].map((path) => readSource(path)));
    // F1: the status sentence and busy flags are live regions, not shell
    // signature; a failed exchange must re-enable Pair under a focused field.
    expect(main).toContain("pairingCleanupFailed ? `cleanup-failed:${pairingCleanupContext.cause}");
    expect(main).not.toContain("      pairingStatus,\n      pairingExchangeInFlight ? \"in-flight\" : \"\",");
    expect(main).toContain("exchangeInFlight: pairingExchangeInFlight,\n      createInFlight: pairingCreateInFlight,");
    expect(main).toContain("pairingStepChanged: pairingView !== lastPairingView,");
    expect(main).toContain('focusInPairingDialog: composing && document.querySelector("#pair-dialog")?.contains(active) === true,');
    expect(model).toContain("return input.pairingStepChanged && input.focusInPairingDialog ? \"shell\" : \"defer\";");
    // The status node is always in the markup so a region can fill it.
    expect(main).toContain("function pairStatusMarkup(pairingStatus: string): string {");
    expect(main).not.toContain("${pairingStatus ? `<p class=\"pair-status\"");
    // F2: cancel closes only once cleanup is durable; otherwise a retry step.
    expect(main).toContain("const outcome = cancellationOutcome(cleared, verifiesCleanup);");
    expect(main).toContain('<h2 id="pair-cleanup-title">${escapeHtml(copy.title)}</h2>');
    expect(main).toContain("const copy = cleanupStepCopy(pairingCleanupContext);");
    // A failed exchange whose rollback rejected needs a retry owner too (review 25564).
    expect(main).toContain("pairingCancellations.begin(operation.generation);\n      pairingCleanupContext = { cause: \"failure\", storeOpen: !cleared.failClosed, rollbackPending: true };");
    expect(main).toContain('<button id="pair-cleanup-retry" type="button" class="primary">Retry cleanup</button>');
    expect(main).toContain("async function retryPairingCleanup(): Promise<void> {");
    // A late rollback failure from the operation Cancel invalidated is shown
    // only while that cancellation owns the dialog; retries are serialized,
    // rejection-safe and applied only if still current (review 25536).
    expect(main).toContain("if (pairingCancellations.ownsOperation(operation.generation)) {");
    expect(main).toContain("const ticket = pairingCancellations.beginRetry();");
    expect(main).toContain("if (!pairingCancellations.finishRetry(ticket)) return;");
    expect(main).toContain("recovery = { failed: true };");
    expect(main).toContain("pairingCancellations.begin(verifiesCleanup ? exchangeOperationGeneration : undefined);");
    expect(main).not.toContain('const verifiesCleanup = pairingExchangeInFlight;\n  document.querySelector<HTMLDialogElement>("#pair-dialog")?.close();');
    // F3: a storage failure after the hub consumed the invitation is named.
    expect(main).toContain("if (error instanceof PairingStorageError) {");
    // F6: invalid and expired links open the dialog on a nonsecret sentence.
    expect(main).toContain("const arrivedFragment = readPairingFragment(window.location, window.history, pendingPairingStore);");
    expect(main).toContain('let pairDialogAutoOpen = pendingPairing !== null || arrivedFragment.kind === "invalid";');
    expect(main).toContain('if (stored.kind === "expired" && !pairingArrivalNotice) {');
    expect(main).toContain("pairingStatus = INVALID_PAIRING_LINK_MESSAGE;");
  });

  // Contract: keeps the live-region selectors and the shell markup on the same nodes.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("keeps the live-region selectors and the shell markup on the same nodes", async () => {
    const [main, regions, fixture] = await Promise.all(
      ["main.ts", "live-regions.ts", "live-regions.test.ts"].map((path) => readSource(path)),
    );
    // The updater writes by selector into markup rendered somewhere else. A
    // rename on either side would silently stop updating a region rather than
    // fail, so both ends are pinned here.
    const selectors = [...regions.matchAll(/(?:querySelector|closest)<[^>]*>\("([^"]+)"\)/g)].map((match) => match[1]!);
    expect(selectors.length).toBeGreaterThan(8);
    for (const selector of new Set(selectors)) {
      // Every region the updater touches is exercised by its own fixture.
      expect(fixture, `${selector} is missing from the live-regions fixture`).toContain(selector.replace(/^[.#]/, ""));
    }
    for (const marker of [
      'class="status-stale" role="status"',
      'id="message-send"',
      'id="message-status"',
      'id="message-delivery"',
    ]) expect(main, `${marker} left the shell template`).toContain(marker);
  });

  // Contract: never leaves the supervisor send button silently disabled.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("never leaves the supervisor send button silently disabled", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readSource(path)));
    // A real `disabled` attribute swallows the tap: no event, no frame, no
    // reason. Observing operators concluded the feature was broken.
    expect(main).not.toContain('<button id="message-send" class="primary" ${!selected || !selectedSession || !supervisor || !canControl(selected.id, selectedSession, "message-send") ? "disabled" : ""}>');
    expect(main).toContain('id="message-send"');
    // The reason now reaches the button through the live-region updater, which
    // must still state it with aria-disabled rather than the disabled property.
    const regions = await readFile(new URL("live-regions.ts", import.meta.url), "utf8");
    expect(main).toContain("...(sendReason ? { sendReason } : {}),");
    expect(regions).toContain('setDisabledReason(root.querySelector<HTMLElement>("#message-send"), view.sendReason);');
    expect(regions).toContain('element.setAttribute("aria-disabled", "true");');
    expect(regions).not.toMatch(/\.disabled\s*=\s*true/);
    expect(main).toContain('<p id="message-status" class="message-status');
    expect(main).toContain('function showComposerStatus(text: string, tone: "info" | "error", transport = false): void {');
    // A reconnecting refusal clears when the session is live again (cas-b789).
    // In the banner's words (journey F9).
    expect(main).toContain('Your message is kept; re-pair, then send it.` : outageRefusal(machine.label), "error", true);');
    expect(main).toContain("sessionsEverLive.add(key);\n        clearTransportStatus(key);");
    expect(css).toContain(".message-status {");
    expect(css).toContain(".message-status.error {");
  });

  // Contract: takes control to deliver an observed message instead of dropping it.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("takes control to deliver an observed message instead of dropping it", async () => {
    const source = await readSource("main.ts");
    // The hub refuses SendMessage without this device's session lease
    // (hub/server.rs handle_client_message), so observe-mode sends need the
    // lease the operator would otherwise have to take by hand.
    expect(source).toContain('if (plan.kind === "take-control-then-send" && (sessionIsUp(machine.id, session) || !machineWillReconnect(machine.id))) {');
    expect(source).toContain("async function takeControlForMessage(machine: StoredMachine, session: string, force = false): Promise<boolean> {");
    expect(source).toContain("await connections.get(machine.id)?.requestControl(session, force);");
    expect(source).toContain("return leases.get(sessionKey(machine.id, session))?.held_by_me === true;");
    expect(source).toContain("Could not take control of ${session}");
    // cas-3433: the conversation header has no Take control, so no copy may
    // send the operator there; a refused message carries the control itself.
    expect(source).not.toContain("Take control from the header");
    expect(source).toContain("takeControl: () => { void takeControlForRefused(threadMachineId, threadSession); },");
    expect(source).toContain("await connections.get(machineId)?.requestControl(session, force);");
    // cas-8e0a: only a control refusal makes the cached lease stale; an
    // in_reply_to refusal must not bring Take control back on another message.
    expect(source).toContain('if (refusal(detail).action === "take-control") controlTakenAfterRefusal.delete(key);');
    // cas-008f: whatever the take's outcome, focus goes back to the message's control, never the body.
    expect(source).toContain("if (pressed && stillHere()) landFocus([messageControl(pressed), focusTargets.thread], { keep: true, nextTask: true, waitMs: 1_000, since: pressed });");
  });

  // Contract: collapses one outage into one attention card per machine and session.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("collapses one outage into one attention card per machine and session", async () => {
    const source = await readSource("main.ts");
    // Without a stable fingerprint each retry coalesces to its own card, so a
    // single unreachable hub buries the feed under near-identical criticals.
    expect(source).toContain("fingerprint: `${machine.id}:auth_loss`");
    expect(source).toContain("fingerprint: `${machine.id}:hub_disconnected`");
    expect(source).toContain("fingerprint: `${machine.id}:${session}:session_transport`");
  });

  // Contract: marks operations data as stale while the hub connection is not live.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("marks operations data as stale while the hub connection is not live", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readSource(path)));
    expect(main).toContain("const statusIsStale = Boolean(selected) && machineConnectionSnapshot !== undefined");
    expect(main).toContain('class="status-stale" role="status"');
    expect(main).toContain("Not live — reconnecting.");
    expect(main).toContain("Showing the last state received ");
    // Retry transitions rewrite snapshot.since, so staleness is anchored to the
    // last live moment instead of reporting a long outage as "just now".
    expect(main).toContain('if (state.phase === "live") lastLiveAt.set(machine.id, Date.now());');
    expect(css).toContain(".status-stale {");
  });

  // Contract: keeps palette rows project-led while indexing project names and optional session summaries.
  // Consumer: Commander application render and event handlers (main.ts), operating the command palette.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("keeps palette rows project-led while indexing project names and optional session summaries", async () => {
    // Behaviour is pinned in palette-commands.test.ts (cas-cfcb); this keeps main.ts on that one renderer.
    const [source, palette] = await Promise.all([readSource("main.ts"), readFile(new URL("palette-commands.ts", import.meta.url), "utf8")]);
    expect(source).toContain("sessionJumpCommandMarkup(machine, session, sessionSummaries.get(sessionKey(machine.id, session.name)), { current, needsYou: conversationNeedsYou(machine.id, session.name) })");
    expect(palette).toContain("<span>Jump to ${escapeHtml(project ?? session.name)}</span>");
    expect(palette).toContain('data-search-text="${escapeHtml(searchText)}"');
    expect(source).toContain('command.dataset.searchText ?? ""');
  });

  // Contract: renders instructional empty pane and all-clear feed states.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("renders instructional empty pane and all-clear feed states", async () => {
    const [main, attentionView] = await Promise.all([
      readSource("main.ts"),
      readFile(new URL("attention-view.ts", import.meta.url), "utf8"),
    ]);
    expect(main).toContain('emptyTitle.textContent = "The supervisor hasn\'t started yet"');
    expect(main).toContain('empty.className = "empty empty-pane-slot"');
    // Cassy Cloud has no pane drag-and-drop, so the empty slot must not promise one.
    expect(main).not.toContain("drag it here");
    expect(attentionView).toContain('message.textContent = options.outage ?? "All clear"');
    expect(attentionView).toContain("timestamp.textContent = lastEventLabel(latest.createdAt, options.now ?? Date.now());");
    expect(attentionView).toContain("`Last event ${stampLabel(at, now)}`");
    expect(attentionView).not.toContain("toLocaleString()");
  });

  // Contract: distinguishes a loading catalog from an unpaired Cassy Cloud drawer.
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("distinguishes a loading catalog from an unpaired Cassy Cloud drawer", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain("let machineCatalogLoaded = false;");
    expect(source).toContain("machineCatalogLoaded = true;");
    expect(source).toContain('"Loading paired machines…"');
    // An unpaired Cassy Cloud offers pairing instead of naming a glyph, and the
    // machine being paired is the one running the sessions, not this device.
    expect(source).toContain('"Pair a machine to start your first conversation."');
    expect(source).toContain('<button id="empty-pair" class="primary" type="button">Pair a machine</button>');
    expect(source).not.toContain("press + to pair this machine");
    expect(source).toContain("render(false);");
  });

  // Contract: gives an unpaired phone one pairing path and no empty-state debris.
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("gives an unpaired phone one pairing path and no empty-state debris", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readSource(path)));
    // First run: the welcome carries the one primary Pair a machine; the list
    // header's chip is its only other way in (cas-0546: no machine rail).
    expect(main).toContain("const welcomePairs = !model.selected && model.loaded && !model.paired;");
    expect(main).toContain('if (paletteToggle) paletteToggle.onclick = openCommandPalette;');
    expect(main).not.toContain('class="commander-mark-label"');
    expect(css).toContain(".attention-last-event {\n  font-family: var(--font-ui);");
    expect(css).not.toMatch(/\.shell\.fleet-empty|\.machine-rail|\.fleet-board/);
  });

  // Contract: names the ticket from the card's derived attention content.
  // Consumer: renderAttentionPanel displaying the coalesced card.content.ticketId.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("names the ticket from the card's derived attention content", async () => {
    const view = await readFile(new URL("attention-view.ts", import.meta.url), "utf8");
    // Hub event-stream cards derive their CAS ticket during coalescing, so the
    // renderer must not look only at the raw latest event.
    expect(view).toContain('ticket.className = "attention-ticket"');
    expect(view).toContain("ticket.textContent = card.content.ticketId;");
    expect(view).not.toContain("ticket.textContent = card.latest.ticketId;");
  });

  // Contract: makes the pairing code reachable without retyping it from a phone screen.
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("makes the pairing code reachable without retyping it from a phone screen", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain('data-pair-command="cas hub authorize ${escapeAttr(pendingPairing.userCode)}"');
    expect(source).toContain("navigator.clipboard.writeText(pairCopy.dataset.pairCommand");
    expect(source).toContain('toast("Command copied")');
    // Pairing used to end by silently closing its dialog, then by claiming
    // "paired" before any connection existed. Saved access is announced at
    // once; "connected" only when that machine's connection reaches live.
    expect(source).toContain("return machine;\n}\n\nasync function startRelayPairing(");
    expect(source).toContain("// Saved and connected are announced at the installation seam inside");
    // The expectation is registered inside pairMachine, before the connection
    // for the new credential is created, so a first live phase can never
    // arrive ahead of it (review 25642).
    const pairMachineBody = source.slice(source.indexOf("async function pairMachine("), source.indexOf("async function startRelayPairing("));
    // Both the saved sentence and the armed expectation live in
    // installPairedMachine, which starts the connection last; the only
    // replaceMachineConnection call in pairMachine is inside its starter.
    expect(pairMachineBody).toContain("installPairedMachine(machine, {\n    announcer: firstConnections,\n    notify: toast,\n    startConnection: (installed) => { replaceMachineConnection(installed, connections, connectionStates, createConnection); },\n  });");
    expect(pairMachineBody.split("replaceMachineConnection(").length - 1).toBe(1);
    expect(source).not.toContain("if (paired) firstConnections.expect(paired.id);");
    expect(source).not.toContain("toast(`Access saved");
    expect(source).toContain("async function pairMachine(form: HTMLFormElement): Promise<StoredMachine | false> {");
    expect(source).not.toContain("const paired = machines.get(selectedMachineId ?? \"\");");
    expect(source).toContain("const connectedNotice = firstConnections.observe(machine.id, machine.label, state);");
    expect(source).toContain("if (connectedNotice) toast(connectedNotice);");
    expect(source).toContain("firstConnections.forget(id);");
    expect(source).not.toContain("} paired`);");
  });

  // Contract: binds the browser fetch receiver at every pairing handoff.
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("binds the browser fetch receiver at every pairing handoff", async () => {
    const source = await readSource("main.ts");
    expect(source).not.toMatch(/fetcher:\s*(?:window\.|globalThis\.)?fetch\s*[,}]/);
    expect(source).not.toMatch(/(?:acknowledgePairing|createPairingRequest|pollPairingRequest)\(\s*(?:window\.|globalThis\.)?fetch\s*,/);
  });

  it("H4-STORAGE-02 creates a non-extractable P-256 signing key and valid proof", async () => {
    const { privateKey, publicKey } = await createDeviceKey();
    expect(privateKey.extractable).toBe(false);
    await expect(crypto.subtle.exportKey("jwk", privateKey)).rejects.toThrow();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const headers = await dpopHeaders(machine, "GET", "/v1/machine");
    const [encodedHeader, encodedClaims, encodedSignature] = headers.DPoP.split(".");
    const decode = (value: string) => Buffer.from(value, "base64url");
    expect(JSON.parse(decode(encodedClaims).toString())).toMatchObject({ htm: "GET", htu: "/v1/machine" });
    const imported = await crypto.subtle.importKey("jwk", publicKey, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, imported, decode(encodedSignature), new TextEncoder().encode(`${encodedHeader}.${encodedClaims}`))).toBe(true);
  });

  // Contract: approved Ghostty WASM bytes retain their pinned integrity.
  // Consumer: the Ghostty terminal runtime loading these vendored artifacts.
  it("pins the green Ghostty WASM spike artifacts by integrity", async () => {
    const cases = [
      ["terminal/ghostty/vendor/ghostty-vt.wasm", "6b1df1a96d59adc26360c312924898dbc122f980c17a32eb1624e48795b83f7e"],
      ["terminal/ghostty/vendor/ghostty-write-pty.wasm", "75cb147e98ede3f85f3cd6236a30f6d12565b0b237e1d8db941f5f3e8ad3d903"],
    ];
    for (const [path, expected] of cases) {
      const bytes = await readFile(new URL(path, import.meta.url));
      expect(createHash("sha256").update(bytes).digest("hex")).toBe(expected);
    }
  });

  // Contract: keeps long-lived credentials out of ambient browser storage and URL channels.
  // Consumer: IndexedDB credential catalog and DPoP signing code; ambient-channel security audit (not a runtime proof).
  // Structural contract retained.
  it("keeps long-lived credentials out of ambient browser storage and URL channels", async () => {
    const source = await Promise.all(["storage.ts", "dpop.ts"].map((path) => readSource(path)));
    const joined = source.join("\n");
    for (const forbidden of ["local" + "Storage", "document.cookie", "serviceWorker.register", "caches.open"]) {
      expect(joined).not.toContain(forbidden);
    }
    expect(await readFile(new URL("storage.ts", import.meta.url), "utf8")).not.toContain("session" + "Storage");
    expect(joined).toContain("indexedDB.open");
  });

  // Contract: feature-detects hub versions and keeps controls disabled on skew.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("feature-detects hub versions and keeps controls disabled on skew", async () => {
    const source = await Promise.all(["main.ts", "connection.ts"].map((path) => readSource(path)));
    const joined = source.join("\n");
    expect(joined).toContain('"/v1/machine"');
    expect(joined).toContain("Compatibility check unavailable");
    expect(joined).toContain('hubSupports(machine.id, "daemon_attach")');
    expect(joined).toContain("unsupported controls are disabled");
  });

  // Contract: targets Interrupt at the session's supervisor pane, chosen deterministically (cas-0546).
  // Consumer: Commander application render and event handlers (main.ts), operating the conversation header.
  // Historical regression retained (was: the Terminal view's selected pane); see cas-9d89 inventory.
  it("targets interrupt at the session's supervisor pane rather than render order or focus", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain('return (visible.find((pane) => pane.kind === "Supervisor") ?? visible[0])?.id;');
    expect(source).toContain("const paneId = supervisorPane(machine.id, session);");
    expect(source).toContain("{ InterruptPane: { pane_id: paneId } }");
    expect(source).not.toContain("[...surfaces.keys()].find");
    expect(source).not.toContain("selectedPanes");
  });

  // Contract: Interrupt takes control the way a send does, and never silently from another device (cas-0546).
  // Consumer: Commander application render and event handlers (main.ts), operating the conversation header.
  it("takes control for an interrupt as a send does, and says when it took it from another device", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain('const force = Boolean(holder && machine.scopes.includes("hub-admin"));');
    expect(source).toContain("if (!await takeControlForMessage(machine, session, force)) {");
    expect(source).toContain('const took = holder ? `Took control from ${holder}. ` : "";');
    expect(source).toContain("toast(`${took}Interrupted ${supervisorPhrase(machine.id, session)}.`, { thread: key });");
    expect(source).toContain("Interrupt works once it releases control, or from a pairing with administrator access, which can take over.");
  });

  // Contract: never caches an asynchronously-created terminal against a detached render.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("never caches an asynchronously-created terminal against a detached render", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain("existingSurface.element !== mount || !existingSurface.element.isConnected");
    expect(source).toContain("!mount.isConnected || currentMount !== mount");
    expect(source).toContain("surface.dispose();");
  });

  // Contract: preserves the active pane grid across lease and status renders.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("preserves the active pane grid across lease and status renders", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain("currentGrid?.dataset.sessionKey === selectedThreadKey");
    expect(source).toContain("const grid = preservedGrid ?? build(");
    expect(source).toContain("data-session-key");
  });

  // Contract: keeps the phone ATTENTION hierarchy human-readable and group-actionable.
  // Consumer: Commander application render and event handlers (main.ts), operating the phone shell.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("keeps the phone ATTENTION hierarchy human-readable and group-actionable", async () => {
    const [main, attentionView, css] = await Promise.all(["main.ts", "attention-view.ts", "styles.css"].map((path) => readSource(path)));
    expect(main).toContain('machineEventAttention(kind, payload, pending)');
    expect(main).toContain('applyAttentionEnrichment(provisional, enriched');
    expect(main).toContain('renderAttentionPanel(container, visibleAttention');
    expect(attentionView).toContain('headline.textContent = card.content.headline');
    expect(attentionView).toContain('button("Dismiss all info"');
    expect(attentionView).toContain('button("Dismiss group"');
    expect(attentionView).toContain('severity !== "critical"');
    expect(css).toContain(".attention-item--critical");
    expect(css).not.toMatch(/\.attention-title::after|attention-summary-shimmer/);
    expect(css).toContain("prefers-reduced-motion: reduce");
    expect(css).toContain("@media (max-width: 53rem), (max-height: 30rem) and (pointer: coarse)");
    for (const selector of ["attention-session", "attention-group-label"]) {
      const rule = css.match(new RegExp(`\\.${selector} \\{([^}]+)\\}`))?.[1] ?? "";
      expect(rule).toContain("overflow-wrap: anywhere");
      expect(rule).not.toMatch(/nowrap|ellipsis|max-width/);
    }
    expect(css).not.toContain("max-width: var(--mobile-attention-label-width)");
  });

  // Contract: opens the pairing dialog for an invitation instead of leaving the user on the empty state.
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("opens the pairing dialog for an invitation instead of leaving the user on the empty state", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readSource(path)));

    // A consumed fragment used to render the same "No machine paired yet"
    // screen, so the only signal that the invitation arrived was that nothing
    // visibly changed.
    expect(main).toContain("function openPairDialog(): void {");
    expect(main).toContain("if (pendingPairing) openPairDialog();");
    // And a fragment delivered to an already-open tab must still be consumed.
    expect(main).toContain("watchPairingFragment(window, pendingPairingStore, (fragment) => {");

    // The soft keyboard belongs to a field the operator chose to fill. Focusing
    // an optional email on open pops it and scrolls the title off the screen.
    expect(main).toContain('<section class="pair-flow" tabindex="-1" autofocus>');
    expect(main).not.toContain('<input id="pair-email" type="email" autofocus');
    // A focused field scrolls clear of the sticky action row, hint included (QA F03).
    expect(css).toContain("scroll-padding-bottom: calc(var(--button-height) + var(--space-4));");
    expect(css).toContain("dialog label:has(> .field-hint) > input { scroll-margin-bottom: 3em; }");
    // Focus goes to the first field still empty, so a prefilled link opens on the operator's name.
    expect(main).toContain("const autofocus = firstEmptyField(pairingDraft, ");
    expect(main).toContain('<input name="url" type="url" required${focus("url")}');
    expect(main).toContain('<input name="operator" required${focus("operator")}');
    expect(main).toContain('<input name="device" required${focus("device")}');

    // With the keyboard up the dialog can be 300px tall: the fields scroll and
    // the action row does not, so Pair stays reachable.
    expect(css).toContain("dialog[open] {\n  display: flex;");
    expect(css).toContain("  max-height: min(88dvh, 720px);");
    expect(css).toContain("  position: sticky;\n  bottom: 0;");

    expect(css).toContain('.pair-flow[tabindex="-1"]:focus-visible { outline: none; }');
    // cas-0bf5: the conversation thread takes the house ring from the keyboard, nothing from a pointer.
    expect(css).toContain(".conversation-reading.thread:focus-visible { outline: var(--focus-ring-width) solid var(--color-focus); outline-offset: calc(-1 * var(--focus-ring-width)); }");
    expect(css).toContain(".conversation-reading.thread:focus:not(:focus-visible) { outline: none; }");

    // D13: a sized card, not a full-viewport dashed rectangle.
    expect(css).toContain(".empty-pane-slot {\n  place-self: center;");
    expect(css).toContain("  width: min(var(--terminal-state-width), 100%);");
    expect(css).not.toContain("border: var(--line-width) dashed var(--line-strong);");
  });

  // Contract: keeps a dedicated one-handed supervisor action and voice-first phone composer.
  // Consumer: Commander application render and event handlers (main.ts), operating the phone shell.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("keeps a voice-first phone composer", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readSource(path)));
    // cas-0546: the side composer ("Talk to supervisor") is gone; the
    // conversation's composer is the one way to write to the supervisor.
    expect(main).not.toContain('id="talk-supervisor"');
    expect(main).toContain('id="message-mic"');
    expect(main).toContain('id="message-keyboard"');
    expect(main).toContain('aria-description="Checking voice input support…"');
    expect(main).not.toContain('Tap to talk');
    expect(main).not.toContain('id="speech-status"');
    // Opening the composer focuses the composer on every layout. Focusing the
    // mic button first made the phone composer unusable by keyboard: the caret
    // was never in the textarea, so the operator's typing went nowhere and
    // Enter toggled dictation (operator report, cas-0d61). Voice stays one
    // labelled tap away.
    expect(main).not.toContain("if (phoneLayout() && mic && !mic.hidden) mic.focus();");
    expect(css).not.toContain(".talk-supervisor {");
    expect(css).toContain("#message-mic {");
    expect(css).toContain(".conversation-composer #message-mic {");
    expect(css).toContain(".conversation-composer #message-mic.listening {");
  });

  // Contract: keeps the section 2 visual system tokenized and machine copy mono.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("keeps the section 2 visual system tokenized and machine copy mono", async () => {
    const [main, css, html, renderer, surface] = await Promise.all([
      "main.ts",
      "styles.css",
      "../index.html",
      "terminal/ghostty/renderer.ts",
      "terminal/ghostty/surface.ts",
    ].map((path) => readSource(path)));
    expect(css).toContain('@import "./tokens.css";');
    expect(css).not.toContain(":root {");
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(css).not.toMatch(/rgba?\(/i);
    expect(main).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(html).not.toMatch(/#[0-9a-f]{3,8}\b/i);

    expect(main).toContain('span.className = "status-identifier"');
    expect(css).toContain("font-family: var(--font-mono)");
    expect(css).not.toContain("border-right:");
    expect(css).not.toContain(".context { border-left:");
    // Elevation is tokenized: the two overlay shadows, the phone rail reset,
    // the unfilled refused send (cas-b1ee), and the Pebble --lift set
    // (cas-cac1: elevation replaces hairlines on the rail, the rows, the
    // compose FAB and the thread).
    const shadows = [...css.matchAll(/box-shadow:\s*([^;]+);/g)].map((match) => match[1].trim());
    expect(shadows.filter((value) => value === "var(--shadow-overlay)")).toHaveLength(2);
    expect(shadows.filter((value) => value === "none")).toHaveLength(2);
    for (const value of shadows) expect(value).toMatch(/^(?:none|var\(--(?:shadow-overlay|lift(?:-strong|-edge|-head|-sup)?)\))$/);
    expect(renderer).not.toContain('"700"');
    expect(surface).not.toContain('"normal 700"');
    expect(surface).not.toContain('"italic 700"');
  });

  // Contract: keeps palette and picker states readable and distinct in every colour mode (cas-78c81).
  // Consumer: Browser forced-colors and normal CSS engines consume palette/picker state rules.
  // Structural contract retained.
  it("keeps palette states readable and distinct in every colour mode (cas-78c81)", async () => {
    const css = await readSource("styles.css");
    const forced = css.slice(css.indexOf("@media (forced-colors: active) {\n  dialog {"));
    // Unavailable rows are not faded below a readable contrast in either mode.
    expect(css).toContain(".palette-command:disabled { opacity: 1; color: var(--text-mid); background: transparent; }");
    expect(forced).toContain(".palette-command:disabled { border-color: GrayText; opacity: 1; }");
    // The system Highlight can carry alpha (0.8 in Chromium's emulation); the
    // focus ring and the open conversation's fill restate it at full strength.
    expect(forced).toContain("outline-color: color(from Highlight srgb r g b / 1);");
    expect(forced).toContain('.conversation-row[aria-current="true"]:is(:hover, :active, :focus-visible):not(:disabled) { forced-color-adjust: none; color: HighlightText; background: Highlight; background: color(from Highlight srgb r g b / 1); }');
    // Relative colour is newer than the support floor (Chrome/Edge 110,
    // Firefox 115): every restatement is preceded by the plain system colour
    // on the same property, which older engines keep (QA round 1, F1).
    const relative = [...forced.matchAll(/([a-z-]+): color\(from Highlight srgb r g b \/ 1\);/g)];
    expect(relative.length).toBeGreaterThanOrEqual(3);
    for (const match of relative) {
      const before = forced.slice(0, match.index);
      expect(before.endsWith(`${match[1]}: Highlight; `), `${match[1]} has a plain Highlight fallback`).toBe(true);
    }
    // Every focused row keeps the opaque ring, the current Appearance row
    // (aria-current since cas-479a) included.
    expect(forced).toContain(".palette-commands .palette-command:focus-visible {");
    // Hover is a pointer cue distinct from the focus ring and the open fill.
    expect(forced).toContain(".palette-command:hover:not(:disabled) > :first-child { text-decoration: underline; }");
    // cas-0546: the session picker is gone with Terminal view.
    expect(css).not.toContain(".session-picker-entry");
  });

  // Contract: routes every navigation through one recorded selection and restores the last session on reopen.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("routes every navigation through one recorded selection and restores the last session on reopen", async () => {
    const main = await readSource("main.ts");
    // One trail: a machine pick, a session open, an attention jump, and a
    // pairing all record the same way, so back and restore never disagree.
    expect(main).toContain("function commitSelection(next: SessionSelection): void {");
    expect(main).toContain("selection = selectSelection(selection, next);");
    expect(main).toContain("saveStoredSelection(selectionStorage(), next);");
    expect(main).toContain("commitSelection({ machineId, session });");
    expect(main).toContain("commitSelection({ machineId: item.machineId });");
    // D14: reopening landed on "No session open" because boot only restored a
    // machine. The session is claimed against the hub's own list.
    expect(main).toContain("const lastSelection = loadStoredSelection(selectionStorage());");
    expect(main).toContain("restoreTarget = restoredMachineId && lastSelection?.session ? lastSelection : undefined;");
    expect(main).toContain("restoreLastSession(machine.id, visibleSessions(machine.id));");
    expect(main).toContain("const session = restorableSession(restoreTarget, machineId, items);");
    expect(main).toContain("if (selectedSession !== undefined) return;");
    // A removed machine must not survive in the back stack or in storage.
    expect(main).toContain("selection = forgetMachine(selection, id);");
    expect(main).toContain("clearStoredSelection(selectionStorage());");
    expect(main).not.toContain("selectedMachineId = machines.keys().next().value; selectedSession = undefined;");
  });

  // Contract: captures both legacy and relay pairing drafts before a background render replaces markup.
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("captures both legacy and relay pairing drafts before a background render replaces markup", async () => {
    const source = await readSource("main.ts");
    expect(source.indexOf("if (captureDraft) capturePairingDraft();")).toBeLessThan(source.indexOf("app.innerHTML ="));
    expect(source).toContain("updatePairingDraft(pairingDraft, new FormData(form).entries()");
    for (const field of ["hubUrl", "machineLabel", "deviceLabel", "operatorLabel", "scopes"]) {
      expect(source).toContain(`pairingDraft.${field}`);
    }
  });

  // cas-37f8, cas-0546: Commander never sizes a pane. Its hidden surface is
  // pinned to the pane's real grid and no ResizePane is ever sent.
  // Consumer: Commander application render and event handlers (main.ts), operating the hidden pane host.
  it("never sends a pane size and pins the hidden surface to the pane's real grid", async () => {
    const source = await readSource("main.ts");
    expect(source).not.toContain("ResizePane");
    expect(source).not.toMatch(/\bInput: \{ pane_id/);
    expect(source).toContain("onResize: () => undefined,");
    expect(source).toContain("onData: () => undefined,");
    expect(source).toContain("surfaces.get(key)?.setAuthoritativeSize({ cols, rows });");
    expect(source).toContain("surface.setAuthoritativeSize(paneGrid(key, state));");
  });

  // Contract: keeps retrying opaque authenticated reads without claiming a pairing refusal.
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("keeps retrying opaque authenticated reads without claiming a pairing refusal", async () => {
    vi.stubGlobal("window", globalThis);
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAuthFailure: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onPaneKeyframe: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      if (new URL(String(input)).pathname === "/v1/health") {
        return { ok: true, status: 200 };
      }
      throw new TypeError("Failed to fetch");
    });
    vi.stubGlobal("fetch", fetchMock);

    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    supervisor.start();

    await vi.waitFor(() => expect(supervisor.snapshot()).toMatchObject({ phase: "backoff", stage: "auth" }));
    expect(callbacks.onAuthFailure).not.toHaveBeenCalled();
    expect(supervisor.snapshot().authFailure).toBeUndefined();
    expect(callbacks.onState).toHaveBeenCalledWith(expect.objectContaining({ phase: "backoff" }));
    // A successful public probe does not classify an opaque fetch rejection.
    expect(fetchMock.mock.calls.map(([input]) => new URL(String(input)).pathname)).toEqual([
      "/v1/health", "/v1/machine", "/v1/sessions",
    ]);
    const [main, connectionView] = await Promise.all([
      readSource("main.ts"),
      readFile(new URL("connection-state-view.ts", import.meta.url), "utf8"),
    ]);
    // cas-a6f0 (journey F8): every refused pairing, revoked included, is headed
    // as the header names it.
    expect(main).toContain('headline: "Machine needs pairing",');
    expect(main).not.toContain('"Authentication blocked"');
    expect(connectionView).toContain('snapshot.authFailure === "revoked" || snapshot.authFailure === "scope-mismatch" || snapshot.authFailure === "needs-pairing"');
    supervisor.stop();
  });

  it("keeps a health-probe failure in the offline dialing backoff", async () => {
    vi.stubGlobal("window", globalThis);
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "offline-machine", label: "Offline machine", baseUrl: "https://offline.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAuthFailure: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onPaneKeyframe: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("Failed to fetch"); }));

    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    supervisor.start();

    await vi.waitFor(() => expect(supervisor.snapshot()).toMatchObject({ phase: "backoff", stage: "dialing" }));
    expect(callbacks.onAuthFailure).not.toHaveBeenCalled();
    supervisor.stop();
  });

  // Contract: degrades an unusable engine honestly instead of spinning at 0s.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("degrades an unusable engine honestly instead of spinning at 0s", async () => {
    const [connection, main, connectionView, css] = await Promise.all(["connection.ts", "main.ts", "connection-state-view.ts", "styles.css"].map((path) => readSource(path)));
    // The version floor that broke every attach on Chrome 113 is gone: the
    // combined signal is built through a helper with a fallback.
    expect(connection).toContain("signal: anySignal([this.eventAbort.signal, signal]),");
    expect(connection).not.toContain("AbortSignal.any(");
    // Fatal is declared, never inferred from TypeError — fetch rejects with
    // TypeError on an ordinary network failure, which must keep retrying.
    expect(connection).toContain("export class UnsupportedBrowserError extends Error {}");
    expect(connection).toContain("if (unsupported) throw new UnsupportedBrowserError(unsupported);");
    expect(connection).toContain("this.transition(\"failed\", stage, { reason: error.message, fatal: true });");
    // A TypeError is only ever classified as a network failure (cas-0978),
    // in one helper that no fatal path calls.
    expect(connection.match(/instanceof TypeError/g)).toHaveLength(1);
    expect(connection).toContain("function isNetworkFailure(error: unknown): boolean {");
    expect(connection).not.toMatch(/isNetworkFailure\([^)]*\)[^;\n]*fatal: true/);
    // The connect clock survives the transitions that reset `since`.
    expect(connection).toContain("connectingSince: connectingAnchor(this.lifecycle, phase, now),");
    // One line naming the missing API and the minimum browsers.
    expect(main).toContain("const browserNotice = unsupportedBrowserNotice(browserSupport());");
    expect(main).toContain('<p class="browser-unsupported" role="alert">');
    expect(css).toContain(".browser-unsupported {");
    // No spinner, no rising counter, and no "reconnecting" claim over a
    // failure that will never resolve.
    expect(connectionView).toContain('? "Connection failed — not retrying."');
    expect(main).toContain("if (snapshot.fatal === true) return;");
    expect(connectionView).toMatch(/for \(const entry of connectionTimeline\(snapshot\)/);
    // One recurring failure is one attention entry, not one per retry.
    expect(main).toContain("const merge = mergeAttentionItem(attention, item);");
    expect(main).toContain("await attentionStore.put(merge.stored);");
    expect(main).not.toContain("attention = [item, ...attention];\n  await attentionStore.put(item);");
  });

  it("fails an engine missing a transport API once, with the reason, instead of retrying it forever", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    const fetchMock = vi.fn(async () => ({ status: 200, ok: true, json: async () => ({}) }));
    vi.stubGlobal("fetch", fetchMock);
    const timeout = (AbortSignal as unknown as { timeout?: unknown }).timeout;
    Reflect.deleteProperty(AbortSignal as unknown as Record<string, unknown>, "timeout");
    try {
      const { privateKey, publicKey } = await createDeviceKey();
      const machine = {
        id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
        credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
        scopes: ["pane-read"], publicKey, privateKey,
      } satisfies StoredMachine;
      const callbacks = {
        onState: vi.fn(), onAttachState: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
        onSessionState: vi.fn(), onOutput: vi.fn(), onPaneKeyframe: vi.fn(), onSocketError: vi.fn(),
      } satisfies HubCallbacks;
      const supervisor = new HubConnectionSupervisor(machine, callbacks);
      const internals = supervisor as unknown as { desired: boolean; attachRetryTimers: Map<string, number> };
      internals.desired = true;

      await supervisor.attach("factory-a");

      const snapshot = supervisor.attachSnapshot("factory-a");
      expect(snapshot).toMatchObject({ phase: "failed", fatal: true });
      expect(snapshot?.reason).toContain("AbortSignal.timeout");
      expect(snapshot?.reason).toContain("Update to Chrome");
      // The overlay states it and offers the escape hatch on the first frame.
      expect(connectingView(snapshot!, Date.now())).toMatchObject({ step: snapshot?.reason, actionsAvailable: true });
      // No retry is scheduled, and the failure is not misreported as revoked.
      expect(internals.attachRetryTimers.size).toBe(0);
      expect(callbacks.onSocketError).toHaveBeenCalledWith("factory-a", snapshot?.reason);
      expect(callbacks.onSocketError).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(60_000);
      expect(callbacks.onSocketError).toHaveBeenCalledTimes(1);
      supervisor.stop();
    } finally {
      if (timeout !== undefined) Object.defineProperty(AbortSignal, "timeout", { value: timeout, configurable: true, writable: true });
    }
  });

  it("keeps one connect clock running across machine retries so the 5s and 15s states appear", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("Failed to fetch"); }));
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "offline", label: "Offline", baseUrl: "https://offline.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 600_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const states: ConnectionState[] = [];
    const callbacks = {
      onState: (state: ConnectionState) => states.push(state), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onPaneKeyframe: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    const startedAt = Date.now();
    supervisor.start();
    await vi.advanceTimersByTimeAsync(30_000);

    const latest = supervisor.snapshot();
    // `since` is rewritten by every transition — that is what froze the
    // overlay at 0s — while the connecting anchor holds the true start.
    expect(latest.since).toBeGreaterThan(startedAt);
    expect(latest.connectingSince).toBe(startedAt);
    expect(connectingView(latest, Date.now())).toMatchObject({ actionsAvailable: true });
    expect(connectingView(latest, Date.now()).elapsedSeconds).toBeGreaterThanOrEqual(15);
    expect(states.filter((state) => state.connectingSince !== startedAt)).toHaveLength(0);
    supervisor.stop();
  });

  it("bounds a terminal that opens but never sends its initial session state", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("fetch", vi.fn(async () => ({ status: 200, ok: true, json: async () => ({ ticket: "unused" }) })));
    class FakeWebSocket {
      static readonly OPEN = 1;
      static readonly CONNECTING = 0;
      static instances: FakeWebSocket[] = [];
      readyState = FakeWebSocket.CONNECTING;
      binaryType = "";
      onopen: (() => void) | null = null;
      onmessage: ((message: MessageEvent) => void) | null = null;
      onclose: ((event: CloseEvent) => void) | null = null;
      onerror: (() => void) | null = null;
      constructor() { FakeWebSocket.instances.push(this); }
      open(): void { this.readyState = FakeWebSocket.OPEN; this.onopen?.(); }
      close(): void { this.readyState = 3; this.onclose?.({ code: 1006 } as CloseEvent); }
      send(): void {}
    }
    vi.stubGlobal("WebSocket", FakeWebSocket);

    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["pane-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAttachState: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onPaneKeyframe: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    (supervisor as unknown as { desired: boolean }).desired = true;

    await supervisor.attach("factory-a");
    FakeWebSocket.instances[0].open();
    await vi.advanceTimersByTimeAsync(3_000);

    expect(callbacks.onSocketError).toHaveBeenCalledWith(
      "factory-a",
      "Terminal attach opened but sent no session state within 3s. Retrying…",
    );
    expect(callbacks.onAttachState.mock.calls.map(([, state]) => [state.phase, state.stage])).toEqual([
      ["auth", "auth"],
      ["dialing", "dialing"],
      ["attaching", "attaching"],
      ["failed", "attaching"],
      ["backoff", "attaching"],
    ]);
    expect(new Set(callbacks.onAttachState.mock.calls.slice(0, -1).map(([, state]) => state.attachSince)).size).toBe(1);
    expect(supervisor.attachSnapshot("factory-a")?.phase).toBe("backoff");
    supervisor.stop();
  });

  // Contract: turns connection failures into timed actions while retaining prior terminal frames.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("turns connection failures into timed actions while retaining prior terminal frames", async () => {
    const [source, connectionView] = await Promise.all([
      readSource("main.ts"),
      readFile(new URL("connection-state-view.ts", import.meta.url), "utf8"),
    ]);
    const styles = await readFile(new URL("styles.css", import.meta.url), "utf8");
    expect(source).toContain("renderConnectionSurfaceInto(placeholder, session, snapshot");
    expect(connectionView).toContain("const view = connectingView(snapshot, now)");
    expect(connectionView).toContain('addAction("Retry", actions.retry)');
    expect(connectionView).toContain('addAction("Diagnose", actions.diagnose)');
    expect(source).toContain("openConnectionLog(machineId)");
    expect(source).toContain("const view = disconnectedView(snapshot, now)");
    // Plain words naming the machine (cas-a447), not the protocol retry line.
    // The words now live in connection-state-view so the refusal and the
    // disabled controls share them (journey F9).
    expect(source).toContain(": lostConnectionBanner(where, snapshot.fatal === true, snapshot.reason);");
    // cas-d15c: a stream the hub closed below a still-connected machine names the conversation.
    expect(source).toContain("? sessionReconnectingBanner(conversationLabel(machineId, session), where, snapshot.fatal === true)");
    expect(connectionView).toContain("`Lost connection to ${machineLabel}. Reconnecting…`");
    expect(source).not.toContain("Connection interrupted — ${view.retryLabel}");
    // Header, row and footer read one conversation connection, and the
    // transport alarm resolves itself once the socket is live again.
    expect(source).toContain('conversationStatusLabel(machine.id, session.name)');
    // The header and the empty thread read one helper (cas-010f).
    expect(source).toContain('fleetConnectionLabel(conversationStatusState(machineId, session), machineId)');
    expect(source).toContain('  return conversationConnection(machineId, session);');
    expect(source).toContain('const label = conversationHeaderLabel(selectedMachineId, selectedSession);');
    expect(source).toContain('connection: () => conversationHeaderLabel(threadMachineId, threadSession),');
    // The empty thread waits for this session's first page, requested or not yet (cas-010f).
    expect(source).toContain("return !page.loaded && !page.unavailable;");
    expect(source).not.toContain("return page.requested === true && !page.loaded;");
    expect(source).toContain("const state = machineFooterConnection(machine.id);");
    // The header status reads "Live", not "· Live": the dot is aria-hidden (cas-17e3).
    expect(source).toContain('const separator = document.createElement("span"); separator.setAttribute("aria-hidden", "true"); separator.textContent = " · ";');
    expect(source).toContain("resolveAttention(`${machine.id}:${session}:session_transport`);");
    // A retrying drop is the banner's to tell; the rail defers to it (cas-90d4).
    expect(source).toContain("if (!transportFailureNeedsAttention(attachStates.get(sessionKey(machine.id, session)), connectionStates.get(machine.id))) return;");
    expect(source).not.toContain('headline: "Terminal transport problem"');
    // While the session is known to be down the banner says so; no toast repeats it over the banner (cas-00cc).
    expect(source).toContain('if (!attach || attach.phase === "live" || attach.phase === "idle") toast("The conversation is reconnecting", { thread: sessionKey(machineId, session) });');
    expect(source).toContain("if (shown) placeToastClearOfBanner(shown);");
    expect(styles).toContain(".terminal-state");
    expect(styles).toContain(".terminal-connecting-step");
    // Nothing dims the conversation during an outage (cas-3446): the banner
    // and the header already mark it as not live.
    expect(styles).not.toContain(".terminal-disconnected .terminal-mount { opacity: .4; }");
  });

  // Contract: drives pane recovery from the selected session attach lifecycle.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("drives pane recovery from the selected session attach lifecycle", async () => {
    const source = await readSource("main.ts");
    expect(source).toContain("onAttachState: (session, state) =>");
    expect(source).toContain("const key = sessionKey(machine.id, session);\n      const attachWasLive = attachStates.get(key)?.phase === \"live\";\n      attachStates.set(key, state);");
    expect(source).toContain("connection.attachSnapshot(selectedSession) ?? connection.snapshot()");
    expect(source).toContain("connection.attachSnapshot(session) ?? connection.snapshot()");
    expect(source).toContain("const connectionSnapshot = attachSnapshot ?? machineConnectionSnapshot");
    expect(source).toContain("connections.get(machineId)?.attach(session)");
  });

  // Contract: removes the connecting instruction when the terminal state arrives.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("removes the connecting instruction when the terminal state arrives", async () => {
    const source = await readSource("main.ts");
    // Scoped to the grid's own child: the conversation thread's empty state is
    // also ".empty" and must survive a re-open (cas-04ee).
    expect(source).toContain('grid.querySelector(":scope > .empty")?.remove();');
    expect(source).not.toMatch(/grid\.querySelector(<HTMLElement>)?\("\.empty"\)/);
  });

  // Contract: puts the conversation up before its pane surface loads (cas-04ee), in its own
  // visible slot beside the hidden pane host, never inside it (cas-0546).
  // Consumer: Commander application render and event handlers (main.ts), operating the conversation surface.
  it("puts the conversation up before its pane surface loads, outside the hidden host (cas-04ee, cas-0546)", async () => {
    const main = await readSource("main.ts");
    const premount = main.indexOf("mountConversation(key, slot);");
    expect(premount).toBeGreaterThan(-1);
    expect(premount).toBeLessThan(main.indexOf("const surface = await createTerminalSurface(mount, {"));
    // The thread mounts in the stage's slot; only the surface mounts in the host.
    expect(main).toContain("const { slot, host } = ensureConversationStage(grid);");
    expect(main).not.toMatch(/mountConversation\([^)]*mount\)/);
    expect(main).toContain("surface.setCanvasPainting(false);");
  });

  // Contract: requests an authoritative supervisor keyframe before lazily mounted workers.
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("requests an authoritative supervisor keyframe before lazily mounted workers", async () => {
    const [connection, main] = await Promise.all(["connection.ts", "main.ts"].map((path) => readSource(path)));
    expect(connection.indexOf("this.requestPaneKeyframe(session, supervisor.id)")).toBeLessThan(
      connection.indexOf("this.callbacks.onSessionState(session, welcome.state, undefined, true)"),
    );
    expect(connection).not.toContain("welcome.scrollback, true");
    // cas-0546: only the supervisor's pane is mounted, so only it asks for a keyframe.
    expect(main).toContain("connections.get(machineId)?.requestPaneKeyframe(session, paneId);");
    expect(main).not.toContain("requestPaneKeyframe(session, pane.id)");
  });

  it("multiplexes sessions on one proto-2 socket and routes raw PTY binary frames", async () => {
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("fetch", vi.fn(async () => ({
      status: 200,
      ok: true,
      json: async () => ({ ticket: "machine-ticket" }),
    })));
    class FakeWebSocket {
      static readonly OPEN = 1;
      static readonly CONNECTING = 0;
      static instances: FakeWebSocket[] = [];
      readyState = FakeWebSocket.CONNECTING;
      binaryType = "";
      sent: string[] = [];
      onopen: (() => void) | null = null;
      onmessage: ((message: MessageEvent) => void) | null = null;
      onclose: ((event: CloseEvent) => void) | null = null;
      onerror: (() => void) | null = null;
      constructor(readonly url: URL) { FakeWebSocket.instances.push(this); }
      open(): void { this.readyState = FakeWebSocket.OPEN; this.onopen?.(); }
      receive(data: string | ArrayBuffer): void { this.onmessage?.({ data } as MessageEvent); }
      close(code = 1000): void { this.readyState = 3; this.onclose?.({ code } as CloseEvent); }
      send(value: string): void { this.sent.push(value); }
    }
    vi.stubGlobal("WebSocket", FakeWebSocket);

    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["pane-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAttachState: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onPaneKeyframe: vi.fn(),
      onFlowControlReset: vi.fn(), onSocketError: vi.fn(), onConversationHistory: vi.fn(),
    } satisfies HubCallbacks;
    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    const internals = supervisor as unknown as { desired: boolean; machineMultiplex: boolean };
    internals.desired = true;
    internals.machineMultiplex = true;

    const firstAttach = supervisor.attach("factory-a");
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    socket.open();
    socket.receive(JSON.stringify({ proto: 2, capabilities: ["pty_binary", "machine_multiplex"] }));
    await firstAttach;
    await supervisor.attach("factory-b");
    await supervisor.attach("factory-a");

    expect(FakeWebSocket.instances).toHaveLength(1);
    expect(socket.sent.map((value) => JSON.parse(value))).toEqual(expect.arrayContaining([
      { proto: 2 },
      { channel: "events", subscribe: true },
      // Supervisors only by default (cas-6261): the subscribe names workers off.
      { channel: "pty:factory-a", subscribe: true, workers: false },
      { channel: "pty:factory-b", subscribe: true, workers: false },
    ]));
    expect(socket.sent.filter((value) => value === JSON.stringify({ channel: "pty:factory-a", subscribe: true, workers: false }))).toHaveLength(1);

    socket.receive(JSON.stringify({
      channel: "pty:factory-a",
      message: { Welcome: {
        session_name: "factory-a",
        state: { focused_pane: "supervisor", panes: [{ id: "supervisor", kind: "Supervisor" }] },
        protocol_version: 3,
        capabilities: ["authoritative_pane_keyframes"],
      } },
    }));
    const historyRequest = socket.sent
      .map((value) => JSON.parse(value) as Record<string, any>)
      .find((value) => value.channel === "pty:factory-a" && value.message?.ConversationHistoryRequest);
    expect(historyRequest?.message.ConversationHistoryRequest).toMatchObject({ limit: 50, device_id: "device" });
    const historyPage = {
      request_id: historyRequest?.message.ConversationHistoryRequest.request_id,
      messages: [{ notification_id: 17, target: "supervisor", text: "Earlier question", state: "acknowledged", stamped: true, device_id: "device", at: "2026-09-21T12:00:00Z" }],
      replies: [{ notification_id: 18, reply_to: 17, message: "Earlier answer", summary: "", device_id: "device", kind: "answer", attachments: [], at: "2026-09-21T12:01:00Z" }],
      has_earlier: false,
    };
    socket.receive(JSON.stringify({ channel: "pty:factory-a", message: { ConversationHistory: historyPage } }));
    expect(callbacks.onConversationHistory).toHaveBeenCalledWith("factory-a", historyPage, { credentialId: "credential-id", generation: 0 });
    const session = new TextEncoder().encode("factory-a");
    const pane = new TextEncoder().encode("supervisor");
    const payload = new Uint8Array([0x1b, 0x5b, 0x48, 0x4f, 0x4b]);
    const frame = new Uint8Array(9 + session.length + pane.length + payload.length);
    frame.set(new TextEncoder().encode("CAS2"));
    frame[4] = 1;
    new DataView(frame.buffer).setUint16(5, session.length);
    new DataView(frame.buffer).setUint16(7, pane.length);
    frame.set(session, 9);
    frame.set(pane, 9 + session.length);
    frame.set(payload, 9 + session.length + pane.length);
    socket.receive(frame.buffer);
    expect(callbacks.onOutput).toHaveBeenCalledWith("factory-a", "supervisor", payload);

    socket.receive(JSON.stringify({ channel: "pty:factory-a", keyframe_required: { skipped: 200 } }));
    expect(callbacks.onFlowControlReset).toHaveBeenCalledWith("factory-a");
    expect(socket.sent.some((value) => value.includes("RequestPaneKeyframe"))).toBe(true);
    supervisor.stop();
  });

  it.each(["stop", "auth-block"] as const)("cancels a scheduled attach retry on %s", async (terminalAction) => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    const fetchMock = vi.fn(async () => ({ status: 200, ok: true, json: async () => ({ ticket: "unused" }) }));
    vi.stubGlobal("fetch", fetchMock);
    const socketOpened = vi.fn();
    class FakeWebSocket {
      static readonly OPEN = 1;
      static readonly CONNECTING = 0;
      readonly readyState = FakeWebSocket.OPEN;
      binaryType = "";
      onopen: (() => void) | null = null;
      onmessage: ((message: MessageEvent) => void) | null = null;
      onclose: ((event: CloseEvent) => void) | null = null;
      onerror: (() => void) | null = null;
      constructor() { socketOpened(); }
      close(): void {}
      send(): void {}
    }
    vi.stubGlobal("WebSocket", FakeWebSocket);

    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["pane-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onPaneKeyframe: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    const internals = supervisor as unknown as {
      desired: boolean;
      scheduleAttach(session: string): void;
      blockAuthentication(detail: string, session?: string): void;
      attachRetryTimers: Map<string, number>;
      socketAttempts: Map<string, number>;
    };
    internals.desired = true;
    internals.scheduleAttach("factory-a");
    internals.scheduleAttach("factory-a");

    expect(vi.getTimerCount()).toBe(1);
    if (terminalAction === "stop") supervisor.stop();
    else internals.blockAuthentication("revoked", "factory-a");
    await vi.advanceTimersByTimeAsync(20_000);
    await supervisor.attach("factory-a");

    expect(internals.attachRetryTimers.size).toBe(0);
    expect(internals.socketAttempts.size).toBe(0);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(socketOpened).not.toHaveBeenCalled();
  });
});

describe("design polish P3/P4/P12/P16 (D3/D4/D12/D17)", () => {
  // Contract: paints the primary action in the operator colour, keeps it on hover, and gives fields and placeholders their contrast.
  // Consumer: Browser CSS engine applying styles.css to Commander markup.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("paints the primary action in the operator colour, keeps it on hover, and gives fields and placeholders their contrast", async () => {
    const [css, tokens, markup] = await Promise.all(["styles.css", "tokens.css", "pair-dialog-markup.ts"].map((path) => readSource(path)));
    const rule = (selector: string) => css.slice(css.indexOf(`${selector} {`), css.indexOf("}", css.indexOf(`${selector} {`)) + 1);
    // P3: you-bg fill, you-fg label, semibold; hover brightens instead of falling to --bg-hover.
    // Scoped to controls with :where() (journey F11): the primary terminal pane also carries .primary.
    expect(rule("\n:where(button, a).primary")).toContain("color: var(--you-fg);");
    expect(rule("\n:where(button, a).primary")).toContain("background: var(--you-bg);");
    expect(rule("\n:where(button, a).primary")).toContain("font-weight: var(--weight-semibold);");
    expect(css).toContain(':where(button, a).primary:hover:not(:disabled):not([aria-disabled="true"]) { background: var(--you-bg); filter: brightness(1.08); }');
    expect(css).not.toContain(".primary:hover:not(:disabled) { background: var(--bg-hover); }");
    expect(css).toContain(".welcome-pairs #pair-toggle { display: none; }");
    expect(css).toContain(".conversation-shell.welcome-pairs .compose-fab { display: none; }");
    // P4: every placeholder in ink-mid at full opacity; dialog inputs are panel fields with a strong edge, not wells.
    expect(css).toContain("input::placeholder,\ntextarea::placeholder { color: var(--ink-mid); opacity: 1; }");
    expect(rule("\ndialog input")).toContain("background: var(--panel);");
    expect(rule("\ndialog input")).toContain("border: var(--line-width) solid var(--ink-mid);");
    expect(tokens).not.toContain("dialog:not(.command-palette) input");
    // P12: the composer draft pill has an edge and keeps its lift.
    expect(css).toContain("border: var(--line-width) solid var(--line-strong); border-radius: 23px; resize: none; background: var(--panel); color: var(--ink); box-shadow: var(--lift);");
    // P16: pairing and attention prose in the UI face; mono only on identifiers.
    expect(css).toContain(".pair-details dd { margin: 0; overflow-wrap: anywhere; font-family: var(--font-ui); }");
    expect(css).toContain(".pair-details dd.pair-identifier,\n.pair-address-actions .pair-identifier {\n  font-family: var(--font-mono);");
    expect(rule("\n.attention-detail")).toContain("font-family: var(--font-ui);");
    expect(markup).toContain('<strong class="pair-summary">');
    expect(markup).toContain('<dt>${escapeHtml(term)}</dt><dd${identifier ? \' class="pair-identifier"\' : ""}>');
  });
});

describe("3.30.0 journey polish (cas-b128)", () => {
  // Contract: groups palette commands by what they act on and offers Dismiss all info only when there is something to dismiss (F4).
  // Consumer: Commander application render and event handlers (main.ts), operating the command palette.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("groups palette commands by what they act on and offers Dismiss all info only when there is something to dismiss (F4)", async () => {
    const main = await readSource("main.ts");
    const conversations = main.slice(main.indexOf('data-palette-group="conversations"'), main.indexOf('data-palette-group="appearance"'));
    // The Conversations group holds only the session jumps.
    const firstGroupEnd = conversations.indexOf("</section>");
    expect(conversations.slice(0, firstGroupEnd)).not.toContain("data-palette-action");
    expect(conversations.slice(0, firstGroupEnd)).not.toContain("palette-paired-machines");
    // cas-0546: no control command (Take control is implicit) and no Terminal view entry.
    expect(main).not.toContain('data-palette-group="session"');
    expect(main).not.toContain('data-palette-action="control"');
    expect(main).not.toContain('data-palette-action="terminal-view"');
    expect(main).not.toContain('data-palette-action="workers"');
    expect(conversations).toContain('<h3 id="palette-group-machines" class="palette-group-heading">Machines</h3>');
    expect(conversations).toContain('${infoItems.length > 0 ? `<button type="button" class="palette-command" data-palette-action="dismiss-info">');
    // A new info item brings the command back: the shell rebuilds on that change.
    // So does a machine starting (or stopping) to grant session launch (cas-0f51).
    expect(main).toContain("JSON.stringify([selectedHubSession?.project_dir, infoItems.length > 0, launchAvailability()])");
  });

  // Contract: moves a visible toast with the layout and uses the thread's clock and plain words in Paired machines (F8, F10).
  // Consumer: Commander application render and event handlers (main.ts), operating the session and conversation surface.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("moves a visible toast with the layout and uses the thread's clock and plain words in Paired machines (F8, F10)", async () => {
    const [main, paired] = await Promise.all([readSource("main.ts"), readSource("paired-machines.ts")]);
    expect(main).toContain('const visibleToast = document.querySelector<HTMLElement>("#toast.visible");');
    expect(main).toContain("toastPlacementInThread(");
    // Journey F8 (dist 3126b032): on the list the toast drops below the brand row.
    expect(main).toContain('document.querySelector<HTMLElement>(".conversation-shell:not(.thread-open) .conversation-list-top")');
    expect(main).toContain("Last seen ${relativeTimestamp(Date.parse(updated))} · ${clockLabel(Date.parse(updated))}");
    expect(main).not.toContain("toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })");
    expect(paired).toContain("'Version unknown until it connects'");
    expect(paired).not.toContain("Runtime not yet received");
  });
});

/**
 * cas-d636: on soundwave a phone's proofs were signed before it slept and sent
 * when it woke, 266 s later; the hub refused them and every 401 read as a
 * revoked pairing, so Commander went dark until a reload. A refusal now says
 * why: a retryable one is tried again with a fresh proof on the hub's clock,
 * and only a definitive one ends the pairing.
 */
describe("a refused DPoP proof is not a lost pairing (cas-d636)", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  type Reply = { status: number; ok: boolean; json: () => Promise<unknown>; clone: () => Reply };
  const reply = (status: number, body: unknown = {}): Reply => ({ status, ok: status >= 200 && status < 300, json: async () => body, clone: () => reply(status, body) });
  const claims = (init: RequestInit | undefined): { iat: number; jti: string } => {
    const proof = (init?.headers as Record<string, string>).DPoP;
    return JSON.parse(Buffer.from(proof.split(".")[1]!, "base64url").toString("utf8"));
  };
  async function supervisorWith(replies: (path: string, call: number) => Reply | Promise<never>) {
    vi.stubGlobal("window", globalThis);
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAuthFailure: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onPaneKeyframe: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    let calls = 0;
    const fetchMock = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => replies(new URL(String(input)).pathname, calls++));
    vi.stubGlobal("fetch", fetchMock);
    return { supervisor: new HubConnectionSupervisor(machine, callbacks), callbacks, fetchMock };
  }

  it("retries a stale proof once with a fresh one on the hub's clock, and recovers", async () => {
    const hubNow = Math.floor(Date.now() / 1000) + 600;
    const { supervisor, fetchMock } = await supervisorWith((_path, call) => call === 0
      ? reply(401, { error: "unauthorized", reason: "stale_proof", retryable: true, server_time: hubNow })
      : reply(200, { schema_version: 1 }));
    await expect(supervisor.request("GET", "/v1/machine")).resolves.toEqual({ schema_version: 1 });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [first, second] = fetchMock.mock.calls.map(([, init]) => claims(init));
    expect(second!.jti).not.toBe(first!.jti);
    // Signed on the hub's clock, ten minutes ahead of this device's.
    expect(Math.abs(second!.iat - hubNow)).toBeLessThanOrEqual(2);
  });

  it("reads a definitive refusal as a lost pairing at once, without a retry", async () => {
    const { supervisor, fetchMock } = await supervisorWith(() => reply(401, { reason: "revoked", retryable: false, server_time: Math.floor(Date.now() / 1000) }));
    await expect(supervisor.request("GET", "/v1/machine")).rejects.toMatchObject({ kind: "revoked", message: "pairing was revoked" });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const expired = await supervisorWith(() => reply(401, { reason: "expired", retryable: false }));
    await expect(expired.supervisor.request("GET", "/v1/machine")).rejects.toMatchObject({ kind: "expired" });
    const key = await supervisorWith(() => reply(401, { reason: "key_mismatch", retryable: false }));
    await expect(key.supervisor.request("GET", "/v1/machine")).rejects.toMatchObject({ kind: "revoked", message: "this browser's key no longer matches the pairing" });
  });

  it("keeps a legacy hub's bare 401 a lost pairing, after one fresh proof", async () => {
    const { supervisor, fetchMock } = await supervisorWith(() => reply(401, { error: "unauthorized" }));
    await expect(supervisor.request("GET", "/v1/machine")).rejects.toMatchObject({ kind: "revoked" });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("retries a proof refused twice like a network failure, never as re-pair", async () => {
    const { supervisor, callbacks } = await supervisorWith((path) => path === "/v1/health"
      ? reply(200, { status: "ok" })
      : reply(401, { reason: "stale_proof", retryable: true, server_time: Math.floor(Date.now() / 1000) }));
    await expect(supervisor.request("GET", "/v1/machine")).rejects.toBeInstanceOf(TransientAuthError);
    supervisor.start();
    await vi.waitFor(() => {
      expect(callbacks.onState).toHaveBeenCalledWith(expect.objectContaining({ phase: "backoff", stage: "auth" }));
    });
    expect(callbacks.onAuthFailure).not.toHaveBeenCalled();
    expect(supervisor.snapshot().authFailure).toBeUndefined();
    supervisor.stop();
  });
});
