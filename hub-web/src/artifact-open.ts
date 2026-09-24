/**
 * Opening a supervisor's artifact from Commander (cassy#910).
 *
 * Every artifact the thread shows — the Pebble sheet, the context rail row,
 * the operator-thread link — is an `#artifact:<id>` link. A tap asks the
 * machine that published it for a short-lived signed view URL
 * (`GET /v1/sessions/<session>/artifacts/<id>/url`). The machine asks Cloud with
 * its own credentials, so the browser never holds a Cloud token, and the URL
 * it gets back points at the blob store's own origin, never Commander's.
 *
 * The tab is opened synchronously in the tap, before the request, because a
 * browser blocks a window opened after an `await`. It is pointed at the URL
 * once the machine answers, or closed with the reason said in plain words.
 */

import { ARTIFACT_LINK_PREFIX } from "./attachment-sheet";

/** What the machine answers for a viewable artifact. */
export interface ArtifactView {
  readonly artifact_id: string;
  readonly url: string;
  readonly expires_at?: string | null;
  readonly name?: string | null;
  readonly mime?: string | null;
}

/** The machine's answer: a view, or the status and stable code saying why not. */
export type ArtifactViewResult =
  | { readonly ok: true; readonly view: ArtifactView }
  | { readonly ok: false; readonly status: number; readonly code?: string; readonly detail?: string | null };

/** The artifact id a link names, or undefined for any other link. */
export function artifactIdFromHref(href: string | null | undefined): string | undefined {
  if (!href) return undefined;
  const hash = href.slice(href.indexOf("#"));
  if (!hash.startsWith(ARTIFACT_LINK_PREFIX)) return undefined;
  try {
    const id = decodeURIComponent(hash.slice(ARTIFACT_LINK_PREFIX.length)).trim();
    return id || undefined;
  } catch {
    return undefined;
  }
}

/** The artifact link a click landed in, if any. */
export function artifactLinkFor(target: EventTarget | null): HTMLAnchorElement | undefined {
  const element = target instanceof Element ? target : target instanceof Node ? target.parentElement : null;
  const link = element?.closest<HTMLAnchorElement>("a[href]");
  return link && artifactIdFromHref(link.getAttribute("href")) ? link : undefined;
}

/** Why an artifact did not open, in the operator's words. */
export function artifactOpenFailure(result: Extract<ArtifactViewResult, { ok: false }>, machineLabel: string): string {
  switch (result.code) {
    case "artifact_not_in_cloud":
      return `This file was only saved on ${machineLabel}. It was never uploaded to Cloud, so it can't open here.`;
    case "artifact_not_committed":
      return "Cloud is still checking this file. Try again in a moment.";
    case "cloud_not_configured":
      return `${machineLabel} isn't signed in to Cassy Cloud, so it can't open hosted files.`;
    case "cloud_storage_not_live":
      return "Cloud file storage isn't available yet, so this file can't open here.";
    case "not_found":
    case "cloud_artifact_not_found":
      return "This file isn't available any more.";
    case "cloud_failed":
      // The machine answered; Cloud did not (cas-e503).
      return "Cassy Cloud couldn't open the file right now. Wait a minute, then tap it again.";
    default:
      if (result.status === 401 || result.status === 403) {
        return "This device isn't allowed to open files on this machine. Pair it again.";
      }
      // No answer at all: the machine is off, asleep or off the network
      // (cas-e503), so the fix is on the machine, not in Cloud.
      if (result.status === 0) {
        return `Couldn't reach ${machineLabel}. Check that it's on and connected, then tap the file again.`;
      }
      return "The file didn't open. Try again.";
  }
}

export interface ArtifactOpenDeps {
  /** Ask the machine for the view URL. */
  readonly fetchView: () => Promise<ArtifactViewResult>;
  /** Open the new tab; must run synchronously inside the tap. */
  readonly openWindow: () => Window | null;
  /** Say something to the operator. */
  readonly notify: (message: string) => void;
  readonly machineLabel: string;
}

/**
 * Open one artifact. Call it synchronously from the click handler: the tab
 * is opened before the first await. Resolves to true once the tab points at
 * the signed URL.
 */
export async function openArtifact(deps: ArtifactOpenDeps): Promise<boolean> {
  const tab = deps.openWindow();
  if (tab) {
    // The opened page must not be able to reach back into Commander.
    try { tab.opener = null; } catch { /* a closed or cross-origin tab */ }
    try { tab.document.title = "Opening file…"; } catch { /* not yet navigable */ }
  }
  let result: ArtifactViewResult;
  try {
    result = await deps.fetchView();
  } catch {
    result = { ok: false, status: 0 };
  }
  if (!result.ok) {
    tab?.close();
    deps.notify(artifactOpenFailure(result, deps.machineLabel));
    return false;
  }
  if (!tab) {
    deps.notify("Your browser blocked the new tab. Allow pop-ups for Commander and tap the file again.");
    return false;
  }
  tab.location.href = result.view.url;
  return true;
}
