import { cloudBrand, escapeHtml } from "./cloud-brand";
import { cleanupStepCopy, type CleanupStepContext } from "./pairing-cleanup";
import type { PairingDraft } from "./pairing-draft";
import { DEFAULT_PAIRING_SCOPES } from "./pairing-relay";
import { PAIRING_SCOPES, pairCommand, scopeChoices, scopeLabel, scopeSummary, ungrantedScopes } from "./pairing-scopes";
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
}

function escapeAttr(value: string): string { return escapeHtml(value); }

/**
 * What pairing grants, in the operator's words. It leads every step; the
 * origin and the raw scope names wait in "Technical details" (F3).
 */
function capabilityLead(scopes: readonly Scope[]): string {
  return `<p class="pair-lead">This browser will be able to: <strong class="pair-summary">${scopeSummary(scopes).map(escapeHtml).join(" · ")}</strong></p>`;
}

function detailRow(term: string, value: string, identifier = true): string {
  return `<div><dt>${escapeHtml(term)}</dt><dd${identifier ? ' class="pair-identifier"' : ""}>${escapeHtml(value)}</dd></div>`;
}

function exactScopes(scopes: readonly Scope[]): string {
  return scopes.map(scopeLabel).join(", ");
}

/** The exact origin and scopes, for whoever wants to check them; collapsed by default. */
function technicalDetails(rows: string, open: boolean, extra = ""): string {
  return `<details class="pair-technical"${open ? " open" : ""}><summary>Technical details</summary><dl class="pair-details">${rows}</dl>${extra}</details>`;
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
  return `<p class="pair-status" role="status"${pairingStatus ? "" : " hidden"}>${escapeHtml(pairingStatus)}</p>`;
}

/**
 * The pairing dialog for one pairing state. main.ts renders it from its live
 * state; the visual-QA fixtures render it from a fixture state, so the gate
 * measures the production dialog — the email field included (D11).
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
    return `<dialog id="pair-dialog">${cloudBrand()}<section class="pair-flow pair-cleanup" tabindex="-1" autofocus aria-labelledby="pair-cleanup-title"><h2 id="pair-cleanup-title">${escapeHtml(copy.title)}</h2><p>${escapeHtml(copy.discarded)} ${escapeHtml(copy.outstanding)}</p><p>${escapeHtml(copy.next)}</p>${pairStatusMarkup(pairingStatus)}<div class="dialog-actions"><button id="pair-close" type="button" data-role="cleanup">Close</button><button id="pair-cleanup-retry" type="button" class="primary">Retry cleanup</button></div></section></dialog>`;
  }
  if (pendingPairing?.kind === "relay-request") {
    const scopes = pendingPairing.requestedScopes;
    return `<dialog id="pair-dialog">${cloudBrand()}<section class="pair-flow"><h2>Pair a machine</h2>${capabilityLead(scopes)}<p>On the machine you want to pair, run this command, then approve the request it prints:</p><p><code>cas hub authorize ${escapeHtml(pendingPairing.userCode)}</code></p><div class="pair-code" aria-label="Pairing code">${escapeHtml(pendingPairing.userCode)}</div><div class="pair-code-actions"><button id="pair-copy" type="button" data-pair-command="cas hub authorize ${escapeAttr(pendingPairing.userCode)}">Copy command</button></div><p>Expires in <strong id="pair-countdown">10:00</strong></p>${technicalDetails(detailRow("Cassy Cloud origin", pendingPairing.controllerOrigin) + detailRow("Exact scopes", exactScopes(scopes)), pairingDraft.technicalOpen)}${pairStatusMarkup(pairingStatus)}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button></div></section></dialog>`;
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
    const machine = relayVerified && hubUrl && origin && invitationScopes
      ? `${capabilityLead(invitationScopes)}<p>Check this is your machine, then add your name.</p><dl class="pair-details pair-machine"><div><dt>Machine</dt><dd>${escapeHtml(pendingPairing.machineLabel ?? pendingPairing.hubId)}</dd></div></dl><p class="pair-expiry">Invitation expires in <strong id="pair-countdown">10:00</strong></p>${technicalDetails(detailRow("Machine's hub address", hubUrl) + detailRow("Cassy Cloud origin", origin) + detailRow("Granted scopes", exactScopes(invitationScopes)), pairingDraft.technicalOpen)}`
      : `${capabilityLead(leadScopes.length ? leadScopes : invitationScopes ?? pairingDraft.scopes)}<p>One-time invitation ready. Check the machine, then add your name.</p><label>Machine's hub address<input name="url" type="url" required${focus("url")} placeholder="https://studio.tailnet.ts.net" value="${escapeAttr(pairingDraft.hubUrl)}"></label>${addressHelp(pairingDraft.pageOrigin, pairingDraft.addressHelpOpen)}<label>Machine name<input name="label" required${focus("label")} placeholder="Studio Mac" value="${escapeAttr(pairingDraft.machineLabel)}"></label>`;
    // The link form keeps its scope boxes (and the command that widens them)
    // with the other technical details, after the fields everyone fills in.
    const linkTechnical = relayVerified ? "" : technicalDetails(detailRow("Cassy Cloud origin", pageOrigin), pairingDraft.technicalOpen, `<fieldset><legend>Scopes requested</legend>${scopeChecks(pairingDraft.scopes, invitationScopes)}</fieldset>${scopeCeilingHint(pageOrigin, invitationScopes)}`);
    return `<dialog id="pair-dialog">${cloudBrand()}<form id="pair-form"><h2>${relay ? "Machine authorized" : "Pair a machine"}</h2>${machine}<label>Your name (shown on the machine)<input name="operator" required${focus("operator")} autocomplete="name" placeholder="Your name" value="${escapeAttr(pairingDraft.operatorLabel)}"></label><label>Name for this browser<input name="device" required${focus("device")} value="${escapeAttr(pairingDraft.deviceLabel)}"></label>${linkTechnical}${pairStatusMarkup(pairingStatus)}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button><button type="submit" class="primary" ${pairingExchangeInFlight ? "disabled" : ""}>${pairingExchangeInFlight ? "Pairing…" : "Pair"}</button></div></form></dialog>`;
  }
  const relayAction = relayOrigin
    ? `<button id="pair-create" type="button" class="primary" ${pairingCreateInFlight ? "disabled" : ""}>${pairingCreateInFlight ? "Creating…" : "Create pairing code"}</button>`
    : '<p class="pairing-disabled-reason">Page-initiated pairing is unavailable because this Cassy Cloud build has no reviewed relay origin.</p>';
  // One state, one next action. Without an invitation there is nothing to
  // Pair, so no Pair control exists here at all; a link printed by the machine
  // opens the confirmation form directly and never passes through this step.
  return `<dialog id="pair-dialog">${cloudBrand()}<section class="pair-flow" tabindex="-1" autofocus><h2>Pair a machine</h2>${capabilityLead(DEFAULT_PAIRING_SCOPES)}<p>Create a ten-minute code, then approve it on the machine you want to pair.</p><label>Email me the code too (optional)<input id="pair-email" type="email" autocomplete="email" placeholder="you@example.com" value="${escapeAttr(pairingDraft.email)}"><small class="field-hint">Handy when you approve it on the machine from another screen.</small></label>${technicalDetails(detailRow("Cassy Cloud origin", pageOrigin) + detailRow("Exact scopes", exactScopes(DEFAULT_PAIRING_SCOPES)), pairingDraft.technicalOpen)}${pairStatusMarkup(pairingStatus)}<p class="pair-alternative">Already have a link? Open the pairing URL that <code>cas hub pair</code> printed on the machine; it continues straight to confirmation.</p><div class="dialog-actions"><button id="pair-close" type="button">${pairingCreateInFlight ? "Cancel" : "Close"}</button>${relayAction}</div></section></dialog>`;
}


/**
 * Render the six scopes against the invitation's ceiling. A scope the machine
 * did not grant is shown, disabled, and explained — requesting it is what made
 * a default `cas hub pair` link fail its first exchange with a bare 401.
 */
function scopeChecks(selectedScopes: readonly Scope[], grantedScopes: readonly Scope[] | undefined): string {
  return scopeChoices(grantedScopes, selectedScopes).map((choice) => `<label class="scope${choice.granted ? "" : " scope-denied"}"><input type="checkbox" name="scope" value="${choice.scope}" ${choice.checked ? "checked" : ""} ${choice.granted ? "" : "disabled"}>${choice.label}${choice.granted ? "" : '<span class="scope-note">not granted by this invitation</span>'}</label>`).join("");
}

/** Name the missing scopes and the exact command that mints them. */
function scopeCeilingHint(pageOrigin: string, grantedScopes: readonly Scope[] | undefined): string {
  const missing = ungrantedScopes(grantedScopes);
  if (!missing.length) return "";
  const command = pairCommand(pageOrigin, PAIRING_SCOPES);
  return `<p class="scope-hint">To also get ${missing.map((scope) => escapeHtml(scopeLabel(scope))).join(", ")}, run this on the machine and open the new link:</p><div class="pair-code-actions"><code>${escapeHtml(command)}</code><button id="pair-copy" type="button" data-pair-command="${escapeAttr(command)}">Copy command</button></div>`;
}
