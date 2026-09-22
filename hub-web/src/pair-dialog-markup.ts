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

function scopeSummaryMarkup(scopes: readonly Scope[]): string {
  return `<div><dt>This browser will be able to</dt><dd class="pair-summary">${scopeSummary(scopes).map(escapeHtml).join(" · ")}</dd></div>`;
}

function pairingDetails(origin: string, scopes: readonly Scope[]): string {
  return `<dl class="pair-details">${scopeSummaryMarkup(scopes)}<div><dt>Cassy Cloud origin</dt><dd class="pair-identifier">${escapeHtml(origin)}</dd></div><div><dt>Exact scopes</dt><dd class="pair-identifier">${scopes.map(scopeLabel).map(escapeHtml).join(", ")}</dd></div></dl>`;
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
    return `<dialog id="pair-dialog">${cloudBrand()}<section class="pair-flow"><h2>Pair a machine</h2><p>On the machine you want to pair, run this command, then approve the request it prints:</p><p><code>cas hub authorize ${escapeHtml(pendingPairing.userCode)}</code></p><div class="pair-code" aria-label="Pairing code">${escapeHtml(pendingPairing.userCode)}</div><div class="pair-code-actions"><button id="pair-copy" type="button" data-pair-command="cas hub authorize ${escapeAttr(pendingPairing.userCode)}">Copy command</button></div><p>Expires in <strong id="pair-countdown">10:00</strong></p>${pairingDetails(pendingPairing.controllerOrigin, pendingPairing.requestedScopes)}${pairStatusMarkup(pairingStatus)}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button></div></section></dialog>`;
  }
  if (pendingPairing?.kind === "invitation") {
    const relay = Boolean(pendingPairing.relay);
    const hubUrl = pendingPairing.hubUrl;
    const origin = pendingPairing.controllerOrigin;
    const invitationScopes = pendingPairing.scopes;
    return `<dialog id="pair-dialog">${cloudBrand()}<form id="pair-form"><h2>${relay ? "Machine authorized" : "Pair a machine"}</h2><p>${relay ? "Verify the machine details, then create this browser's device credential." : "One-time invitation ready. Confirm the target hub."}</p>${relay && hubUrl && origin && invitationScopes ? `<dl class="pair-details"><div><dt>Machine</dt><dd>${escapeHtml(pendingPairing.machineLabel ?? pendingPairing.hubId)}</dd></div><div><dt>Machine's hub address</dt><dd class="pair-identifier">${escapeHtml(hubUrl)}</dd></div>${scopeSummaryMarkup(invitationScopes)}<div><dt>Cassy Cloud origin</dt><dd class="pair-identifier">${escapeHtml(origin)}</dd></div><div><dt>Granted scopes</dt><dd class="pair-identifier">${invitationScopes.map(scopeLabel).map(escapeHtml).join(", ")}</dd></div></dl><p>Invitation expires in <strong id="pair-countdown">10:00</strong></p>` : `<label>Machine's hub address<input name="url" type="url" required autofocus placeholder="https://studio.tailnet.ts.net" value="${escapeAttr(pairingDraft.hubUrl)}"><small class="field-hint">The address of the machine you are pairing, as printed by <code>cas hub pair</code> (usually its Tailscale name). It is not this page's address unless this page is served by that machine.</small></label><div class="pair-code-actions pair-address-actions"><button id="pair-use-page-origin" type="button" data-page-origin="${escapeAttr(pairingDraft.pageOrigin)}">Use this page's address (${escapeHtml(pairingDraft.pageOrigin)})</button></div><label>Machine label<input name="label" required placeholder="Studio Mac" value="${escapeAttr(pairingDraft.machineLabel)}"><small class="field-hint">How this machine is listed in Cassy Cloud.</small></label><fieldset><legend>Scopes requested</legend>${scopeChecks(pairingDraft.scopes, invitationScopes)}</fieldset>${scopeCeilingHint(pageOrigin, invitationScopes)}`}<label>Device label<input name="device" required autofocus value="${escapeAttr(pairingDraft.deviceLabel)}"><small class="field-hint">How this browser is listed on the machine.</small></label><label>Operator label<input name="operator" required placeholder="Your name" value="${escapeAttr(pairingDraft.operatorLabel)}"><small class="field-hint">Who is pairing this browser; the machine records it.</small></label>${pairStatusMarkup(pairingStatus)}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button><button type="submit" class="primary" ${pairingExchangeInFlight ? "disabled" : ""}>${pairingExchangeInFlight ? "Pairing…" : "Pair"}</button></div></form></dialog>`;
  }
  const relayAction = relayOrigin
    ? `<button id="pair-create" type="button" class="primary" ${pairingCreateInFlight ? "disabled" : ""}>${pairingCreateInFlight ? "Creating…" : "Create pairing code"}</button>`
    : '<p class="pairing-disabled-reason">Page-initiated pairing is unavailable because this Cassy Cloud build has no reviewed relay origin.</p>';
  // One state, one next action. Without an invitation there is nothing to
  // Pair, so no Pair control exists here at all; a link printed by the machine
  // opens the confirmation form directly and never passes through this step.
  return `<dialog id="pair-dialog">${cloudBrand()}<section class="pair-flow" tabindex="-1" autofocus><h2>Pair a machine</h2><p>Create a ten-minute code, approve it on the machine you want to pair, then confirm the exact Cassy Cloud origin and scopes here.</p>${pairingDetails(pageOrigin, DEFAULT_PAIRING_SCOPES)}<label>Email code (optional)<input id="pair-email" type="email" autocomplete="email" placeholder="operator@example.com" value="${escapeAttr(pairingDraft.email)}"></label>${pairStatusMarkup(pairingStatus)}<p class="pair-alternative">Already have a link? Open the pairing URL that <code>cas hub pair</code> printed on the machine; it continues straight to confirmation.</p><div class="dialog-actions"><button id="pair-close" type="button">${pairingCreateInFlight ? "Cancel" : "Close"}</button>${relayAction}</div></section></dialog>`;
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
