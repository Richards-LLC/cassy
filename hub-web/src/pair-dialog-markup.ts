import { cloudBrand, escapeHtml } from "./cloud-brand";
import { cleanupStepCopy, type CleanupStepContext } from "./pairing-cleanup";
import type { PairingDraft, PairingStep } from "./pairing-draft";
import { DEFAULT_PAIRING_SCOPES } from "./pairing-relay";
import { HUB_ADMIN_NOTE, HUB_ADMIN_SCOPE, PAIRING_SCOPES, pairCommand, scopeChoices, scopeLabel, scopeSummary, ungrantedScopes } from "./pairing-scopes";
import type { PendingPairing } from "./pending-pairing";
import type { Scope } from "./types";

/** Everything the pairing dialog shows; no module state is read. */
export interface PairDialogState {
  cleanupFailed: boolean;
  cleanupContext: CleanupStepContext;
  pendingPairing: PendingPairing | null;
  draft: PairingDraft;
  status: string;
  createInFlight: boolean;
  exchangeInFlight: boolean;
  /** The reviewed relay origin; undefined means page-initiated pairing is unavailable. */
  relayOrigin: string | undefined | null;
  /** This page's origin (location.origin in the app). */
  pageOrigin: string;
  /**
   * cas-093d F02: the re-pair command that keeps session launch, shown as its
   * own copyable code under the status that introduces it.
   */
  repairCommand?: string;
  /**
   * cas-d8a5 (journey F31): the countdown as last shown, so a rebuilt dialog
   * (the machine claiming the code) never starts again from 10:00.
   */
  countdown?: string;
}

/** m:ss, from milliseconds left. */
export function countdownLabel(remainingMs: number): string {
  const seconds = Math.max(0, Math.floor(remainingMs / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/**
 * The time left to show, from one clock: never more than was last shown for
 * the same request, so the countdown only goes down (journey F31).
 */
export function nextCountdown(expiresAt: string, now: number, lastShownMs: number | undefined): number {
  const remaining = Math.max(0, Date.parse(expiresAt) - now);
  return lastShownMs === undefined ? remaining : Math.min(remaining, lastShownMs);
}

function escapeAttr(value: string): string { return escapeHtml(value); }

/**
 * What pairing grants, in the operator's words. It leads every step; the
 * origin and the raw scope names wait in "Technical details" (F3).
 */
function capabilityLead(scopes: readonly Scope[], adminMachine?: string): string {
  // cas-d043 G08: hub:admin is held only once its box is ticked, so it is
  // never claimed up front; ticking it adds its power to the summary (CSS
  // :has on the box, so no re-render moves the operator's place).
  const summary = scopeSummary(scopes.filter((scope) => scope !== HUB_ADMIN_SCOPE)).map(escapeHtml).join(" · ");
  const admin = adminMachine === undefined ? "" : `<span class="pair-summary-admin"> · ${escapeHtml(hubAdminConsentLabel(adminMachine))}</span>`;
  return `<p class="pair-lead">This browser will be able to: <strong class="pair-summary">${summary}${admin}</strong></p>`;
}

/** The admin consent in plain words, naming the machine (cas-d043 G08); the scope id stays in Technical details. */
export function hubAdminConsentLabel(machine: string): string {
  return `See and revoke other browsers on ${machine.trim() || "this machine"}`;
}

function detailRow(term: string, value: string, identifier = true): string {
  return `<div><dt>${escapeHtml(term)}</dt><dd${identifier ? ' class="pair-identifier"' : ""}>${escapeHtml(value)}</dd></div>`;
}

function exactScopes(scopes: readonly Scope[]): string {
  return scopes.map(scopeLabel).join(", ");
}

/** The exact origin and scopes, for whoever wants to check them; collapsed by default. */
function technicalDetails(step: PairingStep, draft: PairingDraft, rows: string, extra = ""): string {
  return `<details class="pair-technical" data-step="${step}"${draft.technicalOpen === step ? " open" : ""}><summary>Technical details</summary><dl class="pair-details">${rows}</dl>${extra}</details>`;
}

type InvitationField = "url" | "label" | "operator" | "device";

/**
 * Focus lands on the first field still to fill. A `cas hub pair` link prefills
 * the address and machine name, so on a fresh link that is the operator's own
 * name; an older link still starts at the address.
 */
export function firstEmptyField(draft: PairingDraft, relayVerified: boolean): InvitationField {
  const fields: [InvitationField, string][] = [
    ...(relayVerified ? [] : [["url", draft.hubUrl], ["label", draft.machineLabel]] as [InvitationField, string][]),
    ["operator", draft.operatorLabel],
    ["device", draft.deviceLabel],
  ];
  return fields.find(([, value]) => !value.trim())?.[0] ?? "operator";
}

/**
 * Where the address comes from, one tap away instead of three lines under the
 * field. Using this page's origin is an ordinary secondary action, not a code
 * sample: it is right only when this page is served by the machine itself.
 */
function addressHelp(pageOrigin: string, open: boolean): string {
  return `<details class="pair-address-help"${open ? " open" : ""}><summary>Where do I find this?</summary><p class="field-hint">The link <code>cas hub pair</code> printed fills this in. Otherwise it is the address of the machine you are pairing (usually its Tailscale name). It is not this page's address unless this page is served by that machine.</p><div class="pair-address-actions"><button id="pair-use-page-origin" type="button" class="secondary" data-page-origin="${escapeAttr(pageOrigin)}">Use this page's address</button><small class="field-hint">This page is <span class="pair-identifier">${escapeHtml(pageOrigin)}</span></small></div></details>`;
}

function pairStatusMarkup(pairingStatus: string): string {
  return `<p id="pair-status" class="pair-status" role="status" tabindex="-1"${pairingStatus ? "" : " hidden"}>${escapeHtml(pairingStatus)}</p>`;
}

/**
 * A command to run on the machine, as one code token that wraps only between
 * words, with a Copy button whose result is announced inside the dialog
 * (cas-093d F02).
 */
export function pairCommandMarkup(command: string): string {
  // cas-093d QA F01: each word is unbreakable on its own (white-space:
  // nowrap), so the only places a line can break are the spaces between
  // words. word-break: keep-all still let the browser break after a hyphen
  // ("--" / "scopes"). The visible label is the accessible name (QA F02).
  const words = command.split(" ").map((word) => `<span class="pair-command-word">${escapeHtml(word)}</span>`).join(" ");
  return `<div class="pair-code-actions pair-command"><code class="pair-command-token" id="pair-repair-command">${words}</code><button type="button" class="pair-command-copy" data-pair-command="${escapeAttr(command)}" aria-describedby="pair-repair-command">Copy command</button><span class="sr-only pair-command-status" role="status"></span></div>`;
}

/**
 * The pairing dialog for one pairing state. main.ts renders it from its live
 * state; the visual-QA fixtures render it from a fixture state, so the gate
 * measures the production dialog — the email field included (D11).
 */
/**
 * Every variant of the dialog is named by its heading (cas-d043 G11): a
 * screen reader announced an unnamed dialog for all but the cleanup step.
 */
export function pairDialogMarkup(state: PairDialogState): string {
  const {
    cleanupFailed: pairingCleanupFailed,
    cleanupContext: pairingCleanupContext,
    pendingPairing,
    draft: pairingDraft,
    status: pairingStatus,
    createInFlight: pairingCreateInFlight,
    exchangeInFlight: pairingExchangeInFlight,
    relayOrigin,
    pageOrigin,
  } = state;
  if (pairingCleanupFailed) {
    // Cancel already discarded the invitation; this step exists because the
    // page cannot yet prove a reload will not see it again. There is no way
    // back to the invitation from here, only forward through the cleanup.
    const copy = cleanupStepCopy(pairingCleanupContext);
    return `<dialog id="pair-dialog" aria-labelledby="pair-cleanup-title">${cloudBrand()}<section class="pair-flow pair-cleanup" tabindex="-1" autofocus aria-labelledby="pair-cleanup-title"><h2 id="pair-cleanup-title">${escapeHtml(copy.title)}</h2><p>${escapeHtml(copy.discarded)} ${escapeHtml(copy.outstanding)}</p><p>${escapeHtml(copy.next)}</p>${pairStatusMarkup(pairingStatus)}<div class="dialog-actions"><button id="pair-close" type="button" data-role="cleanup">Close</button><button id="pair-cleanup-retry" type="button" class="primary">Retry cleanup</button></div></section></dialog>`;
  }
  if (pendingPairing?.kind === "relay-request") {
    const scopes = pendingPairing.requestedScopes;
    return `<dialog id="pair-dialog" aria-labelledby="pair-title">${cloudBrand()}<section class="pair-flow"><h2 id="pair-title">Pair a machine</h2>${capabilityLead(scopes)}<p>On the machine you want to pair, run this command, then approve the request it prints:</p><p><code>cas hub authorize ${escapeHtml(pendingPairing.userCode)}</code></p><div class="pair-code" aria-label="Pairing code">${escapeHtml(pendingPairing.userCode)}</div><div class="pair-code-actions"><button id="pair-copy" type="button" data-pair-command="cas hub authorize ${escapeAttr(pendingPairing.userCode)}">Copy command</button></div><p>Expires in <strong id="pair-countdown">${escapeHtml(state.countdown ?? "10:00")}</strong></p>${technicalDetails("code", pairingDraft, detailRow("Cassy Cloud origin", pendingPairing.controllerOrigin) + detailRow("Exact scopes", exactScopes(scopes)))}${pairStatusMarkup(pairingStatus)}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button></div></section></dialog>`;
  }
  if (pendingPairing?.kind === "invitation") {
    const relay = Boolean(pendingPairing.relay);
    const hubUrl = pendingPairing.hubUrl;
    const origin = pendingPairing.controllerOrigin;
    const invitationScopes = pendingPairing.scopes;
    const relayVerified = relay && Boolean(hubUrl && origin && invitationScopes);
    const autofocus = firstEmptyField(pairingDraft, relayVerified);
    const focus = (field: InvitationField): string => autofocus === field ? " autofocus" : "";
    // Only the scopes this invitation can grant are ever requested.
    const leadScopes = invitationScopes ? pairingDraft.scopes.filter((scope) => invitationScopes.includes(scope)) : pairingDraft.scopes;
    const adminMachine = invitationScopes?.includes(HUB_ADMIN_SCOPE) ? (relayVerified ? pendingPairing.machineLabel ?? "" : pairingDraft.machineLabel) : undefined;
    const machine = relayVerified && hubUrl && origin && invitationScopes
      ? `${capabilityLead(invitationScopes, adminMachine)}<p>Check this is your machine.</p><dl class="pair-details pair-machine"><div><dt>Machine</dt><dd>${escapeHtml(pendingPairing.machineLabel ?? pendingPairing.hubId)}</dd></div></dl><p class="pair-expiry">Invitation expires in <strong id="pair-countdown">${escapeHtml(state.countdown ?? "10:00")}</strong></p>${technicalDetails("authorized", pairingDraft, detailRow("Machine's hub address", hubUrl) + detailRow("Cassy Cloud origin", origin) + detailRow("Granted scopes", exactScopes(invitationScopes)))}`
      : `${capabilityLead(leadScopes.length ? leadScopes : invitationScopes ?? pairingDraft.scopes, adminMachine)}${withheldLead(pageOrigin, invitationScopes)}<p>One-time invitation ready. Check the machine, then add your name.</p><label>Machine's hub address<input name="url" type="url" required${focus("url")} placeholder="https://studio.tailnet.ts.net" value="${escapeAttr(pairingDraft.hubUrl)}"></label>${addressHelp(pairingDraft.pageOrigin, pairingDraft.addressHelpOpen)}<label>Machine name<input name="label" required${focus("label")} placeholder="Studio Mac" value="${escapeAttr(pairingDraft.machineLabel)}"></label>`;
    // The link form keeps its scope boxes (and the command that widens them)
    // with the other technical details, after the fields everyone fills in.
    const linkTechnical = relayVerified ? "" : technicalDetails("link", pairingDraft, detailRow("Cassy Cloud origin", pageOrigin), `<fieldset class="pair-scope-list"><legend>Scopes requested</legend>${scopeChecks(pairingDraft.scopes.filter((scope) => scope !== HUB_ADMIN_SCOPE), invitationScopes?.filter((scope) => scope !== HUB_ADMIN_SCOPE))}</fieldset>`);
    // cas-5e53 F08: hub:admin is consent, so it sits in plain view beside the
    // name fields, unticked, never folded into Technical details.
    const adminConsent = invitationScopes?.includes(HUB_ADMIN_SCOPE)
      ? `<fieldset class="pair-admin-consent"><legend>Hub administration</legend><label class="scope"><input type="checkbox" name="scope" value="${HUB_ADMIN_SCOPE}"${pairingDraft.scopes.includes(HUB_ADMIN_SCOPE) ? " checked" : ""}>${escapeHtml(hubAdminConsentLabel(adminMachine ?? ""))}<span class="scope-note">${escapeHtml(HUB_ADMIN_NOTE)} Scope: <span class="pair-identifier">${scopeLabel(HUB_ADMIN_SCOPE)}</span>.</span></label></fieldset>`
      : "";
    return `<dialog id="pair-dialog" aria-labelledby="pair-title">${cloudBrand()}<form id="pair-form"><h2 id="pair-title">${relay ? "Machine authorized" : "Pair a machine"}</h2>${machine}<label>Your name (shown on the machine)<input name="operator" required${focus("operator")} autocomplete="name" placeholder="Your name" value="${escapeAttr(pairingDraft.operatorLabel)}"></label><label>Name for this browser<input name="device" required${focus("device")} value="${escapeAttr(pairingDraft.deviceLabel)}"></label>${adminConsent}${linkTechnical}<details><summary>Browser signing key</summary><label><input type="checkbox" name="rotate-key"> Rotate the signing key when updating this installation</label><p>The existing key proves this is the same browser. If browser storage was lost, this creates a new installation; revoke the old device explicitly.</p></details>${pairStatusMarkup(pairingStatus)}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button><button type="submit" class="primary" ${pairingExchangeInFlight ? "disabled" : ""}>${pairingExchangeInFlight ? "Pairing…" : "Pair"}</button></div></form></dialog>`;
  }
  const relayAction = relayOrigin
    ? `<button id="pair-create" type="button" class="primary" ${pairingCreateInFlight ? "disabled" : ""}>${pairingCreateInFlight ? "Creating…" : "Create pairing code"}</button>`
    : '<p class="pairing-disabled-reason">Page-initiated pairing is unavailable because this Cassy Cloud build has no reviewed relay origin.</p>';
  // One state, one next action. Without an invitation there is nothing to
  // Pair, so no Pair control exists here at all; a link printed by the machine
  // opens the confirmation form directly and never passes through this step.
  return `<dialog id="pair-dialog" aria-labelledby="pair-title">${cloudBrand()}<section class="pair-flow" tabindex="-1" autofocus><h2 id="pair-title">Pair a machine</h2>${capabilityLead(DEFAULT_PAIRING_SCOPES)}<p>Create a ten-minute code, then approve it on the machine you want to pair.</p><label>Email me the code too (optional)<input id="pair-email" type="email" autocomplete="email" placeholder="you@example.com" value="${escapeAttr(pairingDraft.email)}"><small class="field-hint">Handy when you approve it on the machine from another screen.</small></label>${technicalDetails("create", pairingDraft, detailRow("Cassy Cloud origin", pageOrigin) + detailRow("Exact scopes", exactScopes(DEFAULT_PAIRING_SCOPES)))}${pairStatusMarkup(pairingStatus)}${state.repairCommand ? pairCommandMarkup(state.repairCommand) : ""}<p class="pair-alternative">Already have a link? Open the pairing URL that <code>cas hub pair</code> printed on the machine; it continues straight to confirmation.</p><div class="dialog-actions"><button id="pair-close" type="button">${pairingCreateInFlight ? "Cancel" : "Close"}</button>${relayAction}</div></section></dialog>`;
}


/**
 * Render the six scopes against the invitation's ceiling. A scope the machine
 * did not grant is shown, disabled, and explained — requesting it is what made
 * a default `cas hub pair` link fail its first exchange with a bare 401.
 */
function scopeChecks(selectedScopes: readonly Scope[], grantedScopes: readonly Scope[] | undefined): string {
  return scopeChoices(grantedScopes, selectedScopes).map((choice) => `<label class="scope${choice.granted ? "" : " scope-denied"}"><input type="checkbox" name="scope" value="${choice.scope}" ${choice.checked ? "checked" : ""} ${choice.granted ? "" : "disabled"}>${choice.label}${choice.granted ? choice.note ? `<span class="scope-note">${escapeHtml(choice.note)}</span>` : "" : '<span class="scope-note">not granted by this invitation</span>'}</label>`).join("");
}

/**
 * What a read-only (or otherwise narrowed) invitation withholds, in plain
 * words beside what it grants, with the exact command that mints a wider link
 * and its Copy (cas-b52d, journey F26). It used to sit under the scope boxes
 * at the bottom of Technical details, below the dialog's fold.
 */
function withheldLead(pageOrigin: string, grantedScopes: readonly Scope[] | undefined): string {
  const missing = ungrantedScopes(grantedScopes);
  if (!missing.length) return "";
  const command = pairCommand(pageOrigin, PAIRING_SCOPES);
  // The scope list breaks after its commas, never inside a scope name.
  return `<div class="pair-withheld"><p class="pair-lead">This link does not let it: <strong class="pair-summary">${scopeSummary(missing).map(escapeHtml).join(" · ")}</strong></p><div class="pair-withheld-command"><p class="scope-hint" id="pair-withheld-hint">Run this on the machine for a link that does:</p><button id="pair-copy" type="button" data-pair-command="${escapeAttr(command)}" aria-describedby="pair-withheld-hint">Copy command</button><code>${escapeHtml(command).replaceAll(",", ",<wbr>")}</code></div></div>`;
}
