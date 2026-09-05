import { createHash, webcrypto } from "node:crypto";
import { readFile } from "node:fs/promises";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, type ConnectionState, type HubCallbacks } from "./connection";
import { connectingView } from "./connection-state-view";
import { createDeviceKey, dpopHeaders } from "./dpop";
import { consumePairingFragment } from "./fragment";
import type { StoredMachine } from "./types";

Object.defineProperty(globalThis, "crypto", { value: webcrypto, configurable: true });

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("binding Cassy Commander browser invariants", () => {
  it("H4-CATALOG-01 consumes pairing fragments synchronously and preserves no capability in the URL", () => {
    const token = "A".repeat(43);
    let replacement = "";
    const location = { hash: `#pair=${token}&hub=machine-1`, pathname: "/", search: "" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;
    expect(consumePairingFragment(location, history)).toEqual({ token, hubId: "machine-1" });
    expect(replacement).toBe("/");
    expect(replacement).not.toContain(token);
  });

  it("offers exactly one primary action without an invitation and never a Pair control (cas-8051 F7)", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    const html = await readFile(new URL("../index.html", import.meta.url), "utf8");
    // The entry step used to render a disabled Pair beside Create pairing code
    // and explain the disabled button in prose. The stronger property: with no
    // invitation there is no Pair/submit control in the dialog at all, the
    // link path is named as the alternative, and Pair exists only on the
    // confirmation form an invitation opens directly.
    const entry = source.slice(source.indexOf("// One state, one next action."), source.indexOf("// A phone sentence takes longer to type"));
    expect(entry).toContain("<h2>Pair a machine</h2>");
    expect(entry).not.toContain(">Pair</button>");
    expect(entry).not.toContain('type="submit"');
    expect(entry).not.toContain("pairing-disabled-reason\">Pair is disabled");
    expect(entry).toContain("Open the pairing URL that <code>cas hub pair</code> printed on the machine; it continues straight to confirmation.");
    expect(source).toContain('<button type="submit" class="primary" ${pairingExchangeInFlight ? "disabled" : ""}>${pairingExchangeInFlight ? "Pairing…" : "Pair"}</button></div></form></dialog>');
    expect(source).toContain('id="pair-create" type="button" class="primary"');
    expect(source).toContain('pairingCreateInFlight ? "Creating…" : "Create pairing code"');
    expect(source).toContain("const relayAction = relayOrigin");
    expect(html).toContain('name="cas-pairing-relay-origin" content="https://petra-stella-cloud.vercel.app"');
    expect(html).toContain("<title>Cassy Commander</title>");
  });

  it("declares the Cassy Commander favicon from the static web source", async () => {
    const [html, favicon] = await Promise.all([
      readFile(new URL("../index.html", import.meta.url), "utf8"),
      readFile(new URL("../public/favicon.svg", import.meta.url), "utf8"),
    ]);
    expect(html).toContain('<link rel="icon" type="image/svg+xml" href="/favicon.svg" />');
    expect(favicon).toContain('>C</text>');
  });

  it("asks for the machine's hub address instead of seeding the page origin (cas-8051 F5)", async () => {
    const [main, draft] = await Promise.all(["main.ts", "pairing-draft.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    expect(draft).toContain('hubUrl: "",');
    expect(draft).toContain("pageOrigin: controllerOrigin,");
    expect(main).toContain("<label>Machine's hub address<input name=\"url\" type=\"url\" required autofocus placeholder=");
    expect(main).toContain("It is not this page's address unless this page is served by that machine.");
    expect(main).toContain('id="pair-use-page-origin"');
    expect(main).toContain("<dt>Machine's hub address</dt>");
    // Consent keeps the exact origin and the exact scope list beside the summary.
    expect(main).toContain("<dt>This browser will be able to</dt>");
    expect(main).toContain("<dt>Exact scopes</dt>");
    expect(main).toContain("<dt>Granted scopes</dt>");
  });

  it("names the remedy when an observer-only credential disables control", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("Relay pairing granted read-only scopes for ${location.origin}");
    expect(source).toContain("cas hub pair --origin ${location.origin}");
    expect(source).toContain("Pairings are specific to each Cassy Commander origin.");
    expect(source).toContain("<dt>Cassy Commander origin</dt>");
    expect(source).toContain('class="control-action" title="${escapeAttr(takeControlReason');
    expect(source).toContain('class="control-disabled-reason"');
    // A phone cannot hover, so an unavailable control keeps its reason in the DOM
    // and says it out loud when tapped instead of hiding it in a title attribute.
    expect(source).toContain('aria-disabled="true" data-disabled-reason="${escapeAttr(takeControlReason)}"');
    expect(source).toContain('aria-disabled="true" data-disabled-reason="${escapeAttr(interruptReason)}"');
    expect(source).toContain("const reason = button.dataset.disabledReason;");
    expect(source).toContain("toast(reason);");
    expect(source).not.toContain('disabled aria-describedby="control-disabled-reason"');
  });

  it("keeps a half-typed supervisor message across background renders", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    // render() replaces app.innerHTML on every heartbeat, which destroyed the
    // composer's contents mid-sentence — fatal on a phone, where typing is slow.
    expect(source).toContain("function captureMessageDraft(): void {");
    expect(source).toContain("function restoreMessageDraft(): void {");
    expect(source.indexOf("captureMessageDraft();")).toBeLessThan(source.indexOf("app.innerHTML ="));
    expect(source.indexOf("app.innerHTML =")).toBeLessThan(source.indexOf("restoreMessageDraft();"));
    expect(source).toContain('const composerWasFocused = document.activeElement?.id === "message-text";');
    // Both restores fire after the same innerHTML rewrite. Arbitrate them once,
    // in a tested function, instead of relying on the order two microtasks
    // happen to be queued in: a terminal that wins swallows the sentence.
    expect(source).toContain("const focusWinner = composerFocusWinner({ composerWasFocused, terminalWasFocused });");
    expect(source).toContain('if (focusWinner === "terminal") queueMicrotask(() => activePaneContext()?.surface.focus());');
    expect(source).toContain('if (focusWinner === "composer") queueMicrotask(() => document.querySelector<HTMLTextAreaElement>("#message-text")?.focus());');
  });

  it("detects a phone from one definition, in both orientations", async () => {
    const [main, css, design] = await Promise.all(["main.ts", "styles.css", "../DESIGN.md"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    // A rotated Pixel 7 is 915px wide, so a width-only breakpoint handed a
    // 412px-tall screen the three-column desktop console (report defect D5).
    // CSS and JS must ask the identical question, or rotation puts the layout
    // and the pane-mounting logic in different modes.
    expect(main).toContain('import { COMPACT_MEDIA_QUERY, PHONE_MEDIA_QUERY } from "./viewport";');
    expect(main).toContain("function phoneLayout(): boolean { return window.matchMedia(PHONE_MEDIA_QUERY).matches; }");
    expect(main).toContain("let attentionPanelCollapsed = window.matchMedia(PHONE_MEDIA_QUERY).matches;");
    expect(main).toContain("function compactViewport(): boolean { return window.matchMedia(COMPACT_MEDIA_QUERY).matches; }");
    // Every viewport question is asked with a shared query string, so no literal
    // breakpoint can drift out of step with the stylesheet again.
    expect(main).not.toContain("max-width: 850px");
    expect(main).not.toContain('matchMedia("(max-width');
    // Rotation flips the layout in CSS instantly; pane composition and the PTY
    // column floor are decided in JS at render time and must follow it.
    expect(main).toContain("for (const query of [PHONE_MEDIA_QUERY, COMPACT_MEDIA_QUERY]) {");
    expect(main).toContain('window.matchMedia(query).addEventListener("change", () => render());');
    expect(css).toContain("@media (max-width: 53rem), (max-height: 30rem) and (pointer: coarse) {");
    expect(css).toContain("@media (max-height: 30rem) and (pointer: coarse) {");
    // The desktop hover-drawer rule must not reach a landscape phone either.
    expect(css).toContain("@media (hover: hover) and (min-width: 53.0625rem) {");
    expect(design).toContain("(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)");
    expect(design).toContain("landscape");
  });

  it("gives a landscape phone the long edges and the full-height terminal", async () => {
    const css = await readFile(new URL("styles.css", import.meta.url), "utf8");
    const landscape = css.slice(css.indexOf("@media (max-height: 30rem) and (pointer: coarse) {"));
    expect(landscape.length).toBeGreaterThan(0);
    // One row: the terminal keeps every one of the 412 pixels it has, instead of
    // giving a third of them to a bottom rail and an attention row.
    expect(landscape).toContain("grid-template-rows: minmax(0, 1fr);");
    expect(landscape).toContain(".shell main { grid-column: 2; grid-row: 1; }");
    // The rail returns to a column on the long edge rather than eating height.
    expect(landscape).toContain("  .machine-rail {\n    flex-direction: column;");
    // An expanded panel floats over the terminal instead of taking a row from it.
    expect(landscape).toContain("  .context-panel:not(.collapsed) {\n    position: fixed;");
    expect(landscape).toContain("env(safe-area-inset-left)");
    expect(landscape).toContain("env(safe-area-inset-right)");
  });

  it("reports the outcome of sending a supervisor message", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    // A send with no outcome is indistinguishable from a lost one, and invites a
    // duplicate message to the supervisor.
    expect(source).toContain("function sendControl(machineId: string, session: string, message: unknown): boolean {");
    expect(source).toContain("const sent = sendControl(machine.id, session, supervisorMessage(supervisor, text));");
    expect(source).toContain("messageDelivery = { session, target: supervisor };");
    expect(source).toContain("toast(`Message sent to ${supervisor}`);");
  });

  it("sends the supervisor message from Enter and from the button, through one path", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    // Enter was never wired: it inserted a newline and sent nothing, in observe
    // mode and in control mode alike (measured against the live hub, cas-0d61).
    expect(source).toContain("composer.onkeydown = (event) => {");
    expect(source).toContain("if (!sendsOnEnter(event)) return;");
    expect(source).toContain("void submitSupervisorMessage();");
    expect(source).toContain('document.querySelector<HTMLButtonElement>("#message-send")!.onclick = () => { void submitSupervisorMessage(); };');
    expect(source).toContain("async function submitSupervisorMessage(): Promise<void> {");
    expect(source).toContain("const plan = planSupervisorSend(supervisorSendContext(text));");
  });

  it("keeps a hub heartbeat off the shell rebuild path", async () => {
    const [main, regions] = await Promise.all(["main.ts", "live-regions.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    // A five-second status frame used to replace every live control in the
    // page: the composer was re-created six times and blurred six times inside
    // ten seconds of typing, and on a phone each blur closes the keyboard.
    expect(main.match(/app\.innerHTML\s*=/g)).toHaveLength(1);
    const decision = main.indexOf("const decision = renderDecision(");
    const rebuild = main.indexOf("app.innerHTML =");
    expect(decision).toBeGreaterThan(0);
    expect(decision).toBeLessThan(rebuild);
    // The regions path returns before the rebuild it is standing in for.
    const guard = main.slice(decision, rebuild);
    expect(guard).toContain('if (decision !== "shell") {');
    expect(guard).toContain("renderRegions({");
    expect(guard).toContain("return;");

    // renderRegions and the updater it calls may only write into nodes that
    // already exist; a single innerHTML there would restore the whole defect.
    const body = main.slice(main.indexOf("function renderRegions(context: RegionContext): void {"));
    const end = body.indexOf("\n}\n");
    expect(end).toBeGreaterThan(0);
    expect(body.slice(0, end)).not.toMatch(/\.innerHTML\s*=/);
    expect(regions).not.toMatch(/\.innerHTML\s*=/);
    expect(regions).not.toContain("createElement(");
    expect(regions).not.toContain("replaceChildren(");

    // The deferred rebuild has to be flushed, or a structural change that
    // arrived mid-sentence would never land — but never mid-gesture, or it
    // deletes the button under the finger before the click is dispatched
    // (cas-c142).
    expect(main).toContain("deferredRender.defer();");
    expect(main).toContain('app.addEventListener("focusout"');
    expect(main).toContain('app.addEventListener("pointerdown", () => deferredRender.gestureStarted(), true);');
    expect(main).toContain('app.addEventListener("pointerup", () => deferredRender.gestureEnded(), true);');
    expect(main).toContain('app.addEventListener("pointercancel", () => deferredRender.gestureCancelled(), true);');
    // A microtask would still run before the click; the flush has to be a
    // macrotask scheduled off the gesture ending.
    expect(main).toContain("afterGesture: (run) => window.setTimeout(run, 0),");
  });

  it("keeps pairing failures inside the open dialog and cancellation cleanup visible (cas-7d55 F1/F2/F3/F6)", async () => {
    const [main, model] = await Promise.all(["main.ts", "render-model.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    // F1: the status sentence and busy flags are live regions, not shell
    // signature; a failed exchange must re-enable Pair under a focused field.
    expect(main).toContain("pairingCleanupFailed ? `cleanup-failed:${pairingCleanupContext.cause}");
    expect(main).not.toContain("      pairingStatus,\n      pairingExchangeInFlight ? \"in-flight\" : \"\",");
    expect(main).toContain("exchangeInFlight: pairingExchangeInFlight,\n      createInFlight: pairingCreateInFlight,");
    expect(main).toContain("pairingStepChanged: pairingView !== lastPairingView,");
    expect(main).toContain('focusInPairingDialog: composing && document.querySelector("#pair-dialog")?.contains(active) === true,');
    expect(model).toContain("return input.pairingStepChanged && input.focusInPairingDialog ? \"shell\" : \"defer\";");
    // The status node is always in the markup so a region can fill it.
    expect(main).toContain("function pairStatusMarkup(): string {");
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

  it("keeps the live-region selectors and the shell markup on the same nodes", async () => {
    const [main, regions, fixture] = await Promise.all(
      ["main.ts", "live-regions.ts", "live-regions.test.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")),
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
      'class="connection-summary ',
      'data-machine-latency="',
      'class="connection-dot"',
      'class="mode-badge ',
      'id="lease"',
      'class="control-action"',
      'id="control-disabled-reason"',
      'id="interrupt"',
      'class="status-stale" role="status"',
      'class="control-disabled-reason" role="note"',
      'id="message-send"',
      'id="message-status"',
      'id="message-delivery"',
    ]) expect(main, `${marker} left the shell template`).toContain(marker);
  });

  it("never leaves the supervisor send button silently disabled", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
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
    expect(main).toContain('function showComposerStatus(text: string, tone: "info" | "error"): void {');
    expect(css).toContain(".message-status {");
    expect(css).toContain(".message-status.error {");
  });

  it("takes control to deliver an observed message instead of dropping it", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    // The hub refuses SendMessage without this device's session lease
    // (hub/server.rs handle_client_message), so observe-mode sends need the
    // lease the operator would otherwise have to take by hand.
    expect(source).toContain('if (plan.kind === "take-control-then-send") {');
    expect(source).toContain("async function takeControlForMessage(machine: StoredMachine, session: string): Promise<boolean> {");
    expect(source).toContain("await connections.get(machine.id)?.requestControl(session, false);");
    expect(source).toContain("return leases.get(sessionKey(machine.id, session))?.held_by_me === true;");
    expect(source).toContain("Could not take control of ${session}");
  });

  it("collapses one outage into one attention card per machine and session", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    // Without a stable fingerprint each retry coalesces to its own card, so a
    // single unreachable hub buries the feed under near-identical criticals.
    expect(source).toContain("fingerprint: `${machine.id}:auth_loss`");
    expect(source).toContain("fingerprint: `${machine.id}:hub_disconnected`");
    expect(source).toContain("fingerprint: `${machine.id}:${session}:session_transport`");
  });

  it("marks operations data as stale while the hub connection is not live", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    expect(main).toContain("const statusIsStale = Boolean(selected) && machineConnectionSnapshot !== undefined");
    expect(main).toContain('class="status-stale" role="status"');
    expect(main).toContain("Not live — reconnecting.");
    expect(main).toContain("Showing the last state received ");
    // Retry transitions rewrite snapshot.since, so staleness is anchored to the
    // last live moment instead of reporting a long outage as "just now".
    expect(main).toContain('if (state.phase === "live") lastLiveAt.set(machine.id, Date.now());');
    expect(css).toContain(".status-stale {");
  });

  it("derives the header mode and terminal cursor from the real session lease", async () => {
    const [main, terminal] = await Promise.all([
      readFile(new URL("main.ts", import.meta.url), "utf8"),
      readFile(new URL("terminal/ghostty/surface.ts", import.meta.url), "utf8"),
    ]);
    expect(main).toContain('const mode = lease?.held_by_me ? "CONTROL" : "OBSERVER"');
    expect(main).toContain('class="mode-badge ${mode.toLowerCase()}"');
    expect(main).toContain("setControlMode(leases.get(selectedKey)?.held_by_me === true)");
    expect(terminal).toContain("state.controlMode && state.focused");
  });

  it("keeps palette codenames primary while indexing optional session summaries", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("<span>Jump to ${escapeHtml(session.name)}</span>");
    expect(source).toContain("summary ? `${summary.title} ${summary.description} ${summary.phase}` : \"\"");
    expect(source).toContain('data-search-text="${escapeAttr(searchMetadata)}"');
    expect(source).toContain('const secondary = summary ? `${machine.label} · ${summary.title} · ${summary.phase}` : machine.label');
    expect(source).toContain('command.dataset.searchText ?? ""');
  });

  it("renders instructional empty pane and all-clear feed states", async () => {
    const [main, attentionView] = await Promise.all([
      readFile(new URL("main.ts", import.meta.url), "utf8"),
      readFile(new URL("attention-view.ts", import.meta.url), "utf8"),
    ]);
    expect(main).toContain('<p class="empty-title">No session open</p>');
    expect(main).toContain('<button id="open-machines" class="primary" type="button">Open machines</button>');
    expect(main).toContain('openMachines.onclick = () => { machineDrawerOpen = true; render(); }');
    expect(main).toContain('emptyTitle.textContent = "No panes in this session yet"');
    expect(main).toContain('empty.className = "empty empty-pane-slot"');
    // Cassy Commander has no pane drag-and-drop, so the empty slot must not promise one.
    expect(main).not.toContain("drag it here");
    expect(attentionView).toContain('message.textContent = "All clear"');
    expect(attentionView).toContain("Last event ${new Date(latest.createdAt).toLocaleString()}");
  });

  it("distinguishes a loading catalog from an unpaired Cassy Commander drawer", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("let machineCatalogLoaded = false;");
    expect(source).toContain("machineCatalogLoaded = true;");
    expect(source).toContain('"Loading paired machines…"');
    // An unpaired Cassy Commander offers pairing instead of naming a glyph, and the
    // machine being paired is the one running the sessions, not this device.
    expect(source).toContain('"No machines paired yet. Pair the machine your sessions run on."');
    expect(source).toContain('pair.textContent = "Pair a machine";');
    expect(source).toContain('<p class="empty-title">No machine paired yet</p>');
    expect(source).toContain('<button id="empty-pair" class="primary" type="button">Pair a machine</button>');
    expect(source).not.toContain("press + to pair this machine");
    expect(source).toContain("render(false);");
  });

  it("gives an unpaired phone one pairing path and no empty-state debris", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    expect(main).toContain("const fleetEmpty = machineCatalogLoaded && machines.size === 0 && attention.length === 0;");
    expect(main).toContain('fleetEmpty ? " fleet-empty" : ""');
    expect(main).toContain("const showSessionControls = selected !== undefined && selectedSession !== undefined;");
    expect(main).toContain("${showSessionControls ?");
    expect(main).toContain('if (paletteToggle) paletteToggle.onclick = openCommandPalette;');
    expect(main).toContain('if (leaseButton) leaseButton.onclick = () =>');
    expect(main).toContain('<span class="commander-mark-label">Machines</span>');
    expect(css).toContain(".shell.fleet-empty .machine-navigation,");
    expect(css).toContain(".shell.fleet-empty .context-panel");
    expect(css).toContain(".shell.fleet-empty .session-header");
    expect(css).toContain(".attention-last-event {\n  font-family: var(--font-ui);");
    expect(css).toContain(".machine-chip, .mode-badge, .connection-summary { display: none; }");
    expect(css).toContain(".machine-rail .commander-mark-label { display: inline; }");
  });

  it("names the ticket from the card's derived attention content", async () => {
    const view = await readFile(new URL("attention-view.ts", import.meta.url), "utf8");
    // Hub event-stream cards derive their CAS ticket during coalescing, so the
    // renderer must not look only at the raw latest event.
    expect(view).toContain('ticket.className = "attention-ticket"');
    expect(view).toContain("ticket.textContent = card.content.ticketId;");
    expect(view).not.toContain("ticket.textContent = card.latest.ticketId;");
  });

  it("makes the pairing code reachable without retyping it from a phone screen", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain('data-pair-command="cas hub authorize ${escapeAttr(pendingPairing.userCode)}"');
    expect(source).toContain("navigator.clipboard.writeText(pairCopy.dataset.pairCommand");
    expect(source).toContain('toast("Command copied")');
    // Pairing used to end by silently closing its dialog, then by claiming
    // "paired" before any connection existed. Saved access is announced at
    // once; "connected" only when that machine's connection reaches live.
    expect(source).toContain("toast(`Access saved — connecting to ${paired?.label ?? \"the machine\"}…`);");
    expect(source).toContain("if (paired) firstConnections.expect(paired.id);");
    expect(source).toContain("const connectedNotice = firstConnections.observe(machine.id, machine.label, state);");
    expect(source).toContain("if (connectedNotice) toast(connectedNotice);");
    expect(source).toContain("firstConnections.forget(selected.id);");
    expect(source).not.toContain("} paired`);");
  });

  it("binds the browser fetch receiver at every pairing handoff", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
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

  it("keeps long-lived credentials out of ambient browser storage and URL channels", async () => {
    const source = await Promise.all(["storage.ts", "dpop.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    const joined = source.join("\n");
    for (const forbidden of ["local" + "Storage", "document.cookie", "serviceWorker.register", "caches.open"]) {
      expect(joined).not.toContain(forbidden);
    }
    expect(await readFile(new URL("storage.ts", import.meta.url), "utf8")).not.toContain("session" + "Storage");
    expect(joined).toContain("indexedDB.open");

    // Layout is an intentionally non-secret, per-device preference. It must
    // remain isolated from the IndexedDB credential catalog.
    const layout = await readFile(new URL("pane-layout.ts", import.meta.url), "utf8");
    expect(layout).toContain("cas-commander:pane-layout:");
    for (const secretField of ["credential", "privateKey", "deviceKey"]) expect(layout).not.toContain(secretField);
  });

  it("feature-detects hub versions and keeps controls disabled on skew", async () => {
    const source = await Promise.all(["main.ts", "connection.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    const joined = source.join("\n");
    expect(joined).toContain('"/v1/machine"');
    expect(joined).toContain("Compatibility check unavailable");
    expect(joined).toContain('hubSupports(machineId, "daemon_attach")');
    expect(joined).toContain("unsupported controls are disabled");
  });

  it("targets interrupt at the explicitly selected pane rather than render order", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("selectedPanes.get(sessionKey(selected.id, selectedSession))");
    expect(source).toContain("{ InterruptPane: { pane_id: pane } }");
    expect(source).not.toContain("[...surfaces.keys()].find");
  });

  it("never caches an asynchronously-created terminal against a detached render", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("existingSurface.element !== mount || !existingSurface.element.isConnected");
    expect(source).toContain("!mount.isConnected || currentMount !== mount");
    expect(source).toContain("surface.dispose();");
  });

  it("preserves the active pane grid across lease and status renders", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("currentGrid?.dataset.sessionKey === terminalSessionKey");
    expect(source).toContain("replaceWith(preservedGrid)");
    expect(source).toContain('document.activeElement?.matches(".t3-ghostty-input")');
    expect(source).toContain('if (focusWinner === "terminal") queueMicrotask(() => activePaneContext()?.surface.focus());');
    expect(source).toContain("data-session-key");
  });

  it("keeps the phone ATTENTION hierarchy human-readable and group-actionable", async () => {
    const [main, attentionView, css] = await Promise.all(["main.ts", "attention-view.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    expect(main).toContain('machineEventAttention(kind, payload, pending)');
    expect(main).toContain('applyAttentionEnrichment(provisional, enriched');
    expect(main).toContain('renderAttentionPanel(container, attention');
    expect(attentionView).toContain('headline.textContent = card.content.headline');
    expect(attentionView).toContain('button("Dismiss all info"');
    expect(attentionView).toContain('button("Dismiss group"');
    expect(attentionView).toContain('severity !== "critical"');
    expect(css).toContain(".attention-item--critical");
    expect(css).toContain(".attention-item--enriching .attention-title::after");
    expect(css).toContain("prefers-reduced-motion: reduce");
    expect(css).toContain("@media (max-width: 53rem), (max-height: 30rem) and (pointer: coarse)");
    expect(css).toContain("max-width: var(--mobile-attention-label-width)");
  });

  it("opens the pairing dialog for an invitation instead of leaving the user on the empty state", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));

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
    expect(main).toContain('<input name="url" type="url" required autofocus');
    expect(main).toContain('<input name="device" required autofocus');

    // With the keyboard up the dialog can be 300px tall: the fields scroll and
    // the action row does not, so Pair stays reachable.
    expect(css).toContain("dialog[open] {\n  display: flex;");
    expect(css).toContain("  max-height: min(88dvh, 720px);");
    expect(css).toContain("  position: sticky;\n  bottom: 0;");

    expect(css).toContain('.pair-flow[tabindex="-1"]:focus-visible { outline: none; }');

    // D13: a sized card, not a full-viewport dashed rectangle.
    expect(css).toContain(".empty-pane-slot {\n  place-self: center;");
    expect(css).toContain("  width: min(var(--terminal-state-width), 100%);");
    expect(css).not.toContain("border: var(--line-width) dashed var(--line-strong);");
  });

  it("D7 gives every control in the phone rail one container treatment", async () => {
    const [main, css, view, design] = await Promise.all(
      ["main.ts", "styles.css", "attention-view.ts", "../DESIGN.md"]
        .map((path) => readFile(new URL(path, import.meta.url), "utf8")),
    );

    // One rule, one surface, one radius, one minimum target for Machines, each
    // machine chip, Pair, the attention summary and the envelope. Three
    // container treatments in one 48px row is the defect, not a style choice.
    expect(css).toContain("--rail-item-min: 44px");
    expect(css).toContain(`  .machine-rail .commander-mark,
  .machine-rail .machine-icon,
  .machine-rail .pair-machine,
  .context-panel.collapsed .attention-rail-counts,
  .context-panel.collapsed .mobile-message-toggle {`);
    expect(css).toContain("    min-width: var(--rail-item-min);\n    min-height: var(--rail-item-min);");
    expect(design).toContain("--rail-item-min");

    // --line-strong is the focused-pane border. Pair is not a pane, and the
    // compact layout has no focusable pane chrome of its own, so the whole
    // phone block must be free of it.
    expect(css).toContain(".pane.selected { border-color: var(--line-strong); }");
    const compact = css.slice(css.indexOf("@media (max-width: 53rem), (max-height: 30rem) and (pointer: coarse)"));
    expect(compact).not.toContain("var(--line-strong)");

    // The collapsed pill floats over the rail, so it must not paint a second
    // surface there: the seam in fig a1 is --bg-raised over --bg-panel.
    expect(css).toContain(`  .context-panel,
  .context-panel.collapsed {
    position: fixed;`);
    expect(css).toContain("    background: var(--color-transparent);\n    overflow: hidden;");
    expect(css).toContain(".context-panel.collapsed .attention-rail {\n    display: flex;");

    // The machine chip carries a readable name on a phone and an unclipped
    // status dot, instead of two initials with the dot on the corner radius.
    expect(main).toContain('<span class="machine-state ${state}"></span><span class="machine-initials">');
    expect(main).toContain('<span class="machine-name">');
    expect(css).toContain(".machine-icon .machine-name { display: none; }");
    expect(css).toContain("  .machine-rail .machine-icon .machine-initials { display: none; }");
    expect(css).toContain("  .machine-rail .machine-icon .machine-name {");
    expect(css).toContain("  .machine-rail .machine-icon .machine-state { position: static; }");

    // D8: one badge treatment. State lives in the text and the dot, never in a
    // fill that only two of the three severities receive.
    expect(css).not.toContain(".attention-count--critical { color: var(--state-crit); background: var(--tint-crit); }");
    expect(css).toContain(".attention-count--critical { color: var(--state-crit); }");
    expect(css).toContain(".attention-count--info { color: var(--state-info); }");
    expect(view).toContain("export function renderAttentionSummary(");
    expect(main).toContain("renderAttentionSummary(context.counts)");
  });

  it("keeps supervisor messaging reachable from the collapsed phone rail", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    expect(main).toContain('id="mobile-message-toggle"');
    expect(main).toContain("function openSupervisorComposer(): void {");
    expect(main).toContain('activeContextTab = "status";');
    expect(main).toContain("attentionPanelCollapsed = false;");
    expect(css).toContain(".mobile-message-toggle { display: none; }");
    expect(css).toContain(".mobile-message-toggle {");
    // The collapsed pill holds the attention summary and the envelope on one
    // row. A pill narrower than the two rail items it renders lets the summary
    // overflow left across the Pair button (D7/fig b1a).
    expect(css).toContain("--mobile-context-pill-width: 152px");
    expect(css).toContain(".context-panel.collapsed .attention-rail .rail-control { display: none; }");
    expect(css).toContain("padding-right: calc(var(--mobile-context-pill-width) + var(--space-1))");
    // Tapping the envelope must land on the composer it advertises.
    expect(main).toContain('document.querySelector<HTMLTextAreaElement>("#message-text")');
    expect(main).toContain("composer?.focus();");
  });

  it("keeps a dedicated one-handed supervisor action and voice-first phone composer", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    expect(main).toContain('id="talk-supervisor"');
    expect(main).toContain("Talk to supervisor");
    expect(main).toContain('id="message-mic"');
    expect(main).toContain('id="message-keyboard"');
    // Opening the composer focuses the composer on every layout. Focusing the
    // mic button first made the phone composer unusable by keyboard: the caret
    // was never in the textarea, so the operator's typing went nowhere and
    // Enter toggled dictation (operator report, cas-0d61). Voice stays one
    // labelled tap away.
    expect(main).not.toContain("if (phoneLayout() && mic && !mic.hidden) mic.focus();");
    expect(main).toContain("// Voice is one labelled tap away; focus belongs in the field that accepts text.");
    expect(css).toContain(".talk-supervisor {");
    expect(css).toContain("#message-mic {");
    expect(css).toContain("grid-column: 1 / -1;");
    expect(css).toContain("min-height: 48px;");
  });

  it("keeps a focused terminal focused across steady-state renders", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    // Re-inserting a pane card blurs its hidden textarea, which closes a phone
    // keyboard on every five-second heartbeat render. Panes move only when their
    // slot or position genuinely changed.
    expect(source).toContain("const placePane = (slot: HTMLElement, card: HTMLElement): void => {");
    expect(source).toContain("if (slot.children[index] === card) return;");
    expect(source).toContain("placePane(pane.id === layout.primaryPaneId ? primarySlot : secondaryStrip, card);");
    expect(source).not.toContain("(pane.id === layout.primaryPaneId ? primarySlot : secondaryStrip).append(card)");
  });

  it("colours connection dots from the phases the supervisor actually emits", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    // connectionClass emits lifecycle phases, so styling legacy names such as
    // "connected" or "offline" leaves every dot stuck on idle grey.
    expect(main).toContain('function connectionClass(state: ConnectionState | undefined): string { return state?.degraded ? "degraded" : state?.phase ?? "idle"; }');
    expect(css).toContain(".machine-state.live,");
    expect(css).toContain(".machine-state.backoff,");
    expect(css).toContain(".machine-state.failed,");
    expect(css).not.toContain(".machine-state.connected");
    expect(css).not.toContain(".machine-state.offline");
    expect(css).not.toContain(".machine-state.auth-blocked");
  });

  it("renders phone secondary panes as tappable rows instead of empty terminal wells", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    // Only the primary pane mounts a surface on a phone, so every other pane has
    // to read as a compact row and open on tap rather than reserve empty space.
    expect(main).toContain("const secondaryOnPhone = phone && pane.id !== layout.primaryPaneId;");
    expect(main).toContain('card.classList.toggle("collapsed", secondaryOnPhone || (pane.kind !== "Supervisor" && collapsedWorkerPanes.has(key)));');
    expect(main).toContain("if (phoneLayout() && !card?.classList.contains(\"primary\")) {");
    expect(main).toContain("updateLayout((current) => promotePane(current, pane.id));");
    expect(main).toContain('const hint = secondaryOnPhone\n        ? "Tap to open this pane"');
    expect(css).toContain("grid-template-rows: minmax(0, 1fr) auto;");
    expect(css).toContain('.pane-grid.pane-layout .pane-search::after { content: "⌕"');
    expect(css).toContain("min-width: var(--space-8);\n    min-height: var(--space-8);");
  });

  it("keeps the section 2 visual system tokenized and machine copy mono", async () => {
    const [main, css, html, renderer, surface] = await Promise.all([
      "main.ts",
      "styles.css",
      "../index.html",
      "terminal/ghostty/renderer.ts",
      "terminal/ghostty/surface.ts",
    ].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    for (const token of [
      "--bg-root: #101318",
      "--bg-panel: #151922",
      "--bg-raised: #1B202B",
      "--bg-terminal: #0C0E13",
      "--bg-hover: #222836",
      "--bg-active: #2A3142",
      "--line-subtle: #232936",
      "--line-strong: #38415A",
      "--text-hi: #E8EBF2",
      "--text-mid: #9AA3B5",
      "--text-lo: #5C6577",
      "--state-ok: #4CC38A",
      "--state-warn: #E5B454",
      "--state-crit: #E5645E",
      "--state-info: #6CA7F2",
      "--state-idle: #5C6577",
      "--fs-xs: .6875rem",
      "--fs-sm: .78125rem",
      "--fs-base: .84375rem",
      "--fs-md: .9375rem",
      "--fs-lg: 1.125rem",
      "--radius-card: 6px",
      "--radius-pane: 8px",
      "--radius-pill: 999px",
    ]) expect(css).toContain(token);

    const rootEnd = css.indexOf("\n}\n");
    expect(rootEnd).toBeGreaterThan(0);
    const componentCss = css.slice(rootEnd + 3);
    expect(componentCss).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(componentCss).not.toMatch(/rgba?\(/i);
    expect(main).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(html).not.toMatch(/#[0-9a-f]{3,8}\b/i);

    expect(main).toContain('class="session-name"');
    expect(main).toContain('class="session-meta"');
    expect(main).toContain('"toolbar-session-title"');
    expect(main).toContain('span.className = "status-identifier"');
    expect(css).toContain("font-family: var(--font-mono)");
    expect(css).not.toContain("border-right:");
    expect(css).not.toContain(".context { border-left:");
    expect(css.match(/box-shadow:/g)).toHaveLength(2);
    expect(renderer).not.toContain('"700"');
    expect(surface).not.toContain('"normal 700"');
    expect(surface).not.toContain('"italic 700"');
  });

  it("encodes the supervisor-first Cassy Commander shell at desktop and phone widths", async () => {
    const [main, css, connection] = await Promise.all(["main.ts", "styles.css", "connection.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    expect(main).toContain('class="machine-navigation${machineDrawerOpen ? " drawer-open" : ""}"');
    expect(main).toContain('id="pair-toggle" class="rail-control pair-machine"');
    expect(main).toContain('class="session-header"');
    expect(main).toContain('data-context-tab="status"');
    expect(main).toContain('Workers &amp; Tasks');
    expect(main).toContain('visiblePanes.find((pane) => pane.kind === "Supervisor")?.id');
    expect(main).toContain('data-machine-latency');
    expect(main).toContain('state.latencyMs === undefined ? "live" : `live · ${state.latencyMs}ms`');
    expect(main).not.toContain("state.latencyMs ?? 0");
    expect(connection).toContain('onLatency?(latencyMs: number)');
    expect(css).toContain('--machine-rail-width: 48px');
    expect(css).toContain('--context-panel-width: 320px');
    expect(css).toContain('--session-header-height: 44px');
    expect(css).toContain('--pane-header-height: 32px');
    expect(css).toContain('grid-template-columns: var(--machine-rail-width) minmax(0, 1fr) var(--context-panel-width)');
    expect(css).toContain('flex: 0 0 var(--session-header-height)');
    expect(css).toContain('grid-template-rows: minmax(0, 65fr) minmax(var(--space-8), 35fr)');
    expect(css).toContain('.secondary-pane-strip .pane.collapsed');
    expect(css).toContain('grid-template-rows: minmax(0, 1fr) calc(var(--machine-rail-width) + env(safe-area-inset-bottom))');
    expect(css).toContain('grid-template-rows: minmax(0, 1fr) minmax(0, min(45dvh, var(--mobile-drawer-max-height))) calc(var(--machine-rail-width) + env(safe-area-inset-bottom))');
    expect(css).toContain('/* Full words or no chip: OBS / Ctrl / Int made a first-time Pixel pass read');
    expect(css).toContain('.machine-chip, .mode-badge, .connection-summary { display: none; }');
    expect(main).not.toContain('class="toolbar"');
    expect(main).not.toContain('class="machines"');
    expect(main).not.toContain('class="sessions"');
  });

  it("puts a session picker and a back control in the primary chrome on both layouts", async () => {
    const [main, css] = await Promise.all(["main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    // The session name in the header is the switch. It is the only chrome that
    // is always visible on a phone, where the ⌘K palette is display:none.
    expect(main).toContain('id="session-picker-toggle" class="session-picker-toggle" type="button" aria-haspopup="dialog"');
    expect(main).toContain('<dialog id="session-picker" class="command-palette session-picker">');
    expect(main).toContain('document.querySelector<HTMLButtonElement>("#session-picker-toggle")!.onclick = openSessionPicker;');
    expect(main).toContain('if (back) back.onclick = goBack;');
    expect(main).toContain('backTarget ? `<button id="session-back" class="session-back"');
    // Every session the hub exposes, with its role and status — a bare animal
    // name does not distinguish one supervisor from another.
    expect(main).toContain("escapeHtml(sessionPickerMeta(entry))");
    // cas-5d94: the hub derives the roster from the live agent registry, so the
    // count is stated — including a real zero — instead of being suppressed.
    expect(main).not.toContain("const workers = entry.workerCount > 0 ?");
    expect(main).toContain("escapeHtml(workerCountLabel(session.workers.length))");
    expect(main).toContain('if (entry.current) button.setAttribute("aria-current", "true");');
    // A five-second heartbeat render must not close the picker mid-choice.
    expect(main).toContain('if (sessionPickerOpen) document.querySelector<HTMLDialogElement>("#session-picker")?.showModal();');
    expect(css).toContain(".session-identity {");
    expect(css).toContain(".session-back {");
    expect(css).toContain('.session-picker-entry[aria-current="true"]');
    expect(css).toContain(".session-back { width: var(--space-10); }");
  });

  it("routes every navigation through one recorded selection and restores the last session on reopen", async () => {
    const main = await readFile(new URL("main.ts", import.meta.url), "utf8");
    // One trail: a machine pick, a session open, an attention jump, and a
    // pairing all record the same way, so back and restore never disagree.
    expect(main).toContain("function commitSelection(next: SessionSelection): void {");
    expect(main).toContain("selection = selectSelection(selection, next);");
    expect(main).toContain("saveStoredSelection(selectionStorage(), next);");
    expect(main).toContain("commitSelection({ machineId: machine.id });");
    expect(main).toContain("commitSelection({ machineId, session });");
    // Back re-attaches without recording a new step forward.
    expect(main).toContain("selection = goBackSelection(selection);");
    expect(main).toContain("if (previous.session) void attachSelectedSession(previous.machineId, previous.session);");
    // D14: reopening landed on "No session open" because boot only restored a
    // machine. The session is claimed against the hub's own list.
    expect(main).toContain("const lastSelection = loadStoredSelection(selectionStorage());");
    expect(main).toContain("restoreTarget = restoredMachineId && lastSelection?.session ? lastSelection : undefined;");
    expect(main).toContain("restoreLastSession(machine.id, items);");
    expect(main).toContain("const session = restorableSession(restoreTarget, machineId, items);");
    expect(main).toContain("if (selectedSession !== undefined) return;");
    // A removed machine must not survive in the back stack or in storage.
    expect(main).toContain("selection = forgetMachine(selection, selected.id);");
    expect(main).toContain("clearStoredSelection(selectionStorage());");
    expect(main).not.toContain("selectedMachineId = machines.keys().next().value; selectedSession = undefined;");
  });

  it("captures both legacy and relay pairing drafts before a background render replaces markup", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source.indexOf("if (captureDraft) capturePairingDraft();")).toBeLessThan(source.indexOf("app.innerHTML ="));
    expect(source).toContain("updatePairingDraft(pairingDraft, new FormData(form).entries()");
    for (const field of ["hubUrl", "machineLabel", "deviceLabel", "operatorLabel", "scopes"]) {
      expect(source).toContain(`pairingDraft.${field}`);
    }
  });

  it("lets unleased observers size panes but preserves controller-owned geometry", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain('machines.get(machineId)?.scopes.includes("pane-read")');
    expect(source).toContain("return !lease?.controller_label || lease.held_by_me");
    expect(source).toContain("if (becameGeometryOwner) resizeViewablePanes(machineId, session)");
    expect(source).toContain("if (!canResizePanes(machineId, session)) return;");
    expect(source).toContain("{ ResizePane: { pane_id: paneId, cols, rows } }");
  });

  // cas-37f8: a phone-sized viewer must never shrink the operator's console.
  it("stops asking for a pane size once the local dashboard claims that pane", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain('const local = authority === "LocalDashboard";');
    expect(source).toContain("if (!ownsPaneGeometry(machineId, session, paneId)) return;");
    expect(source).toContain(
      "surfaces.get(key)?.setAuthoritativeSize(local ? { cols, rows } : null)",
    );
    // Every ResizePane the viewer can send goes through the one suppression gate.
    const sends = source.match(/ResizePane: \{/g) ?? [];
    expect(sends).toHaveLength(1);
    expect(source).toContain("onResize: (cols, rows) => requestPaneSize(machineId, session, pane.id, cols, rows)");
  });

  it("turns a reachable revoked hub into a terminal auth stop", async () => {
    const source = await readFile(new URL("connection.ts", import.meta.url), "utf8");
    expect(source).toContain('new URL("/v1/health", this.machine.baseUrl)');
    expect(source).toContain('mode: "no-cors"');
    expect(source).toContain("const reachable = await this.hubIsReachable()");
    expect(source).toContain("if (reachable)");
    expect(source).toContain("this.desired = false");
    expect(source).toContain("this.eventAbort?.abort()");
    expect(source).toContain('this.transition("failed", "auth", { reason: detail, authFailure: kind })');
    expect(source).toContain("this.callbacks.onAuthFailure?.(kind, detail)");
  });

  it("turns a reachable hub with opaque authenticated reads into a re-pair stop", async () => {
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

    await vi.waitFor(() => {
      expect(callbacks.onAuthFailure).toHaveBeenCalledWith(
        "needs-pairing",
        "Hub is reachable but this Cassy Commander is no longer paired. Re-pair to continue.",
      );
    });
    expect(supervisor.snapshot()).toMatchObject({
      phase: "failed", stage: "auth", authFailure: "needs-pairing",
    });
    expect(callbacks.onState).not.toHaveBeenCalledWith(expect.objectContaining({ phase: "backoff" }));
    expect(fetchMock.mock.calls.map(([input]) => new URL(String(input)).pathname)).toEqual([
      "/v1/health", "/v1/machine", "/v1/sessions",
    ]);
    const main = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(main).toContain('state.authFailure === "needs-pairing" ? "Machine needs pairing"');
    expect(main).toContain('snapshot.authFailure === "revoked" || snapshot.authFailure === "scope-mismatch" || snapshot.authFailure === "needs-pairing"');
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

  it("degrades an unusable engine honestly instead of spinning at 0s", async () => {
    const [connection, main, css] = await Promise.all(["connection.ts", "main.ts", "styles.css"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    // The version floor that broke every attach on Chrome 113 is gone: the
    // combined signal is built through a helper with a fallback.
    expect(connection).toContain("signal: anySignal([this.eventAbort.signal, signal]),");
    expect(connection).not.toContain("AbortSignal.any(");
    // Fatal is declared, never inferred from TypeError — fetch rejects with
    // TypeError on an ordinary network failure, which must keep retrying.
    expect(connection).toContain("export class UnsupportedBrowserError extends Error {}");
    expect(connection).toContain("if (unsupported) throw new UnsupportedBrowserError(unsupported);");
    expect(connection).toContain("this.transition(\"failed\", stage, { reason: error.message, fatal: true });");
    expect(connection).not.toContain("error instanceof TypeError");
    // The connect clock survives the transitions that reset `since`.
    expect(connection).toContain("connectingSince: connectingAnchor(this.lifecycle, phase, now),");
    // One line naming the missing API and the minimum browsers.
    expect(main).toContain("const browserNotice = unsupportedBrowserNotice(browserSupport());");
    expect(main).toContain('<p class="browser-unsupported" role="alert">');
    expect(css).toContain(".browser-unsupported {");
    expect(css).toContain(".shell.with-browser-notice { height: calc(100dvh - var(--browser-notice-height)); }");
    // No spinner, no rising counter, and no "reconnecting" claim over a
    // failure that will never resolve.
    expect(main).toContain("title.textContent = fatal ? `Cannot connect to ${session}` : `Connecting to ${session}…`;");
    expect(main).toContain("if (snapshot.fatal === true) return;");
    expect(main).toContain("? snapshot.reason ?? \"This browser cannot reconnect to the terminal.\"");
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

  it("turns connection failures into timed actions while retaining prior terminal frames", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    const styles = await readFile(new URL("styles.css", import.meta.url), "utf8");
    expect(source).toContain("const view = connectingView(snapshot, now)");
    expect(source).toContain('retry.textContent = "Retry"');
    expect(source).toContain('diagnose.textContent = "Diagnose"');
    expect(source).toContain("openConnectionLog(machineId)");
    expect(source).toContain("const view = disconnectedView(snapshot, now)");
    expect(source).toContain("Disconnected ${view.elapsedSeconds}s ago");
    expect(styles).toContain(".terminal-state");
    expect(styles).toContain(".terminal-connecting-step");
    expect(styles).toContain(".terminal-disconnected .terminal-mount { opacity: .4; }");
  });

  it("drives pane recovery from the selected session attach lifecycle", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("onAttachState: (session, state) =>");
    expect(source).toContain("attachStates.set(sessionKey(machine.id, session), state)");
    expect(source).toContain("connection.attachSnapshot(selectedSession) ?? connection.snapshot()");
    expect(source).toContain("connection.attachSnapshot(session) ?? connection.snapshot()");
    expect(source).toContain("const connectionSnapshot = terminalAttachSnapshot ?? machineConnectionSnapshot");
    expect(source).toContain("connections.get(machineId)?.attach(session)");
  });

  it("removes the connecting instruction when the terminal state arrives", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain('grid.querySelector(".empty")?.remove();');
  });

  it("requests an authoritative supervisor keyframe before lazily mounted workers", async () => {
    const [connection, main] = await Promise.all(["connection.ts", "main.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    expect(connection.indexOf("this.requestPaneKeyframe(session, supervisor.id)")).toBeLessThan(
      connection.indexOf("this.callbacks.onSessionState(session, welcome.state, undefined, true)"),
    );
    expect(connection).not.toContain("welcome.scrollback, true");
    expect(main).toContain("const collapsedOnPhone = phoneLayout() && secondaryOnPhone;");
    expect(main.indexOf("if (collapsedOnPhone) continue;")).toBeLessThan(
      main.indexOf("requestPaneKeyframe(session, pane.id)"),
    );
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
      onFlowControlReset: vi.fn(), onSocketError: vi.fn(),
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
      { channel: "pty:factory-a", subscribe: true },
      { channel: "pty:factory-b", subscribe: true },
    ]));
    expect(socket.sent.filter((value) => value === JSON.stringify({ channel: "pty:factory-a", subscribe: true }))).toHaveLength(1);

    socket.receive(JSON.stringify({
      channel: "pty:factory-a",
      message: { Welcome: {
        session_name: "factory-a",
        state: { focused_pane: "supervisor", panes: [{ id: "supervisor", kind: "Supervisor" }] },
        protocol_version: 3,
        capabilities: ["authoritative_pane_keyframes"],
      } },
    }));
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
