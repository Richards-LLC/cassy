/**
 * When a render is allowed to rebuild the shell.
 *
 * `render()` used to assign `app.innerHTML` on every hub push, so a five-second
 * status frame destroyed and re-created every live control in the page. The
 * composer was replaced six times and blurred six times inside ten seconds of
 * typing, and on a phone each blur closes the soft keyboard.
 *
 * The shell markup only has to be rebuilt when something it interpolates
 * structurally actually changed. Heartbeat data — latency, attention counts,
 * the status payload, message status, the stale-age sentence — is written into
 * persistent nodes instead, so the composer node survives the whole session.
 */

export type RenderDecision = "shell" | "regions" | "defer";

const FIELD_SEPARATOR = "\u0001";

export interface ShellSignatureParts {
  /** Selected machine and session, which choose whole regions of the markup. */
  readonly machineId: string | undefined;
  readonly session: string | undefined;
  /** Catalog identity: a machine appearing or leaving changes the rail. */
  readonly machineIds: readonly string[];
  /** `machine/session` pairs, which drive the palette's jump commands. */
  readonly sessionKeys: readonly string[];
  readonly catalogLoaded: boolean;
  readonly drawerOpen: boolean;
  readonly attentionCollapsed: boolean;
  readonly contextTab: string;
  readonly fleetEmpty: boolean;
  readonly supervisor: string | undefined;
  readonly backLabel: string | undefined;
  readonly compatibility: string | undefined;
  /**
   * Lease identity, not lease timestamps. bindEvents captures the lease in its
   * handlers, so a change of controller has to rebuild rather than leave a
   * stale closure behind a live button.
   */
  readonly leaseHeldByMe: boolean;
  readonly leaseController: string | undefined;
  readonly controlDisabled: boolean;
  readonly commandPaletteOpen: boolean;
  readonly sessionPickerOpen: boolean;
  /**
   * The pairing dialog's *step*: which flow is showing, which request it
   * identifies, its expiry, and whether cancellation cleanup is outstanding.
   * The status sentence and the in-flight flags are deliberately absent — they
   * are written into the dialog's own nodes as live regions, so a failed
   * exchange re-enables Pair without rebuilding the form under the operator's
   * focus (F1).
   */
  readonly pairingView: string;
}

/**
 * Deliberately excludes everything a heartbeat changes. If a value belongs
 * here, a five-second frame rebuilds the page; if it belongs in the live
 * regions, it does not. That trade is the whole design.
 */
export function shellSignature(parts: ShellSignatureParts): string {
  return [
    parts.machineId ?? "",
    parts.session ?? "",
    parts.machineIds.join(","),
    parts.sessionKeys.join(","),
    parts.catalogLoaded ? "loaded" : "loading",
    parts.drawerOpen ? "drawer" : "",
    parts.attentionCollapsed ? "collapsed" : "expanded",
    parts.contextTab,
    parts.fleetEmpty ? "empty" : "",
    parts.supervisor ?? "",
    parts.backLabel ?? "",
    parts.compatibility ?? "",
    parts.leaseHeldByMe ? "control" : "observe",
    parts.leaseController ?? "",
    parts.controlDisabled ? "disabled" : "",
    parts.commandPaletteOpen ? "palette" : "",
    parts.sessionPickerOpen ? "picker" : "",
    parts.pairingView,
  // A control character no field can contain: without a separator, moving a
  // character from the machine id into the session name would produce the same
  // string and silently skip a rebuild the page needed.
  ].join(FIELD_SEPARATOR);
}

/**
 * A control the operator is actively using. Deferring on focus is what keeps a
 * structural change — a machine appearing, a session list arriving — from
 * yanking the keyboard out from under a half-typed message.
 */
export function isEditableElement(element: { tagName?: string; isContentEditable?: boolean } | null | undefined): boolean {
  if (!element) return false;
  if (element.isContentEditable === true) return true;
  const tag = element.tagName?.toUpperCase();
  return tag === "TEXTAREA" || tag === "INPUT" || tag === "SELECT";
}

export interface RenderDecisionInput {
  readonly signatureChanged: boolean;
  /** An editable control inside the app has focus. */
  readonly composing: boolean;
  /** The pairing dialog moved to a different step (see `pairingView`). */
  readonly pairingStepChanged?: boolean;
  /** The focused control is inside the pairing dialog itself. */
  readonly focusInPairingDialog?: boolean;
}

/**
 * A pairing step change while the focus is inside the pairing dialog is the
 * operator's own doing — they submitted, cancelled, or their code was
 * approved — and the new step is where focus belongs. Deferring it left the
 * old form on screen until something blurred it. The composer and every other
 * field keep their protection: a step change never overrides focus elsewhere.
 */
export function renderDecision(input: RenderDecisionInput): RenderDecision {
  if (!input.signatureChanged) return "regions";
  if (!input.composing) return "shell";
  return input.pairingStepChanged && input.focusInPairingDialog ? "shell" : "defer";
}
