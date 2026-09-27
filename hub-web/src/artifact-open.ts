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

/**
 * Whether a failure describes the file itself (never uploaded, gone), which
 * stays true, rather than the connection or Cloud right now, which the next
 * reconnect makes out of date (cas-c808 QA F01).
 */
export function artifactFailureIsAboutTheFile(result: ArtifactViewResult | undefined): boolean {
  return result !== undefined && !result.ok && ["artifact_not_in_cloud", "not_found", "cloud_artifact_not_found"].includes(result.code ?? "");
}

/** Whether the machine said the file never left it: the one failure worth remembering (journey F6). */
export function artifactIsLocalOnly(result: ArtifactViewResult | undefined): boolean {
  return result !== undefined && !result.ok && result.code === "artifact_not_in_cloud";
}

/**
 * Why an artifact did not open, in the operator's words. `machineLive` is
 * whether this page's connection to the machine is up: a request that got no
 * answer from a connected machine must not claim the machine is off while the
 * header says Live (journey F6). The words work for a click and a tap alike.
 */
export function artifactOpenFailure(result: Extract<ArtifactViewResult, { ok: false }>, machineLabel: string, machineLive = false): string {
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
      return "Cassy Cloud couldn't open the file right now. Wait a minute, then open it again.";
    default:
      if (result.status === 401 || result.status === 403) {
        return "This device isn't allowed to open files on this machine. Pair it again.";
      }
      // No answer at all: the machine is off, asleep or off the network
      // (cas-e503), so the fix is on the machine, not in Cloud.
      if (result.status === 0) {
        return machineLive
          ? `${machineLabel} is connected but didn't send the file. Try again in a moment.`
          : `Couldn't reach ${machineLabel}. Check that it's on and connected, then open the file again.`;
      }
      return "The file didn't open. Try again.";
  }
}

export interface ArtifactOpenDeps {
  /** Ask the machine for the view URL. */
  readonly fetchView: () => Promise<ArtifactViewResult>;
  /** Open the new tab; must run synchronously inside the tap. */
  readonly openWindow: () => Window | null;
  /** Say something to the operator; `result` is the machine's answer, when there was one. */
  readonly notify: (message: string, result?: ArtifactViewResult) => void;
  readonly machineLabel: string;
  /** Whether this page's connection to the machine is live right now. */
  readonly machineLive?: () => boolean;
  /**
   * The machine already said this file was never uploaded (journey F6). No
   * tab is opened for it; the machine is asked again only to learn whether
   * that has changed.
   */
  readonly knownLocalOnly?: boolean;
  /** The file's name, shown in the tab while the machine answers. */
  readonly fileName?: string;
}

/**
 * Open one artifact. Call it synchronously from the click handler: the tab
 * is opened before the first await. Resolves to true once the tab points at
 * the signed URL.
 */
export async function openArtifact(deps: ArtifactOpenDeps): Promise<boolean> {
  const tab = deps.knownLocalOnly ? null : deps.openWindow();
  if (tab) {
    // The opened page must not be able to reach back into Commander.
    try { tab.opener = null; } catch { /* a closed or cross-origin tab */ }
    try { tab.document.title = "Opening file…"; } catch { /* not yet navigable */ }
    // A slow machine leaves this tab up for the whole request: it says what
    // it is waiting for rather than sitting blank (journey F6).
    try { tab.document.body.textContent = `Opening ${deps.fileName ?? "the file"} from ${deps.machineLabel}…`; } catch { /* not yet navigable */ }
  }
  let result: ArtifactViewResult;
  try {
    result = await deps.fetchView();
  } catch {
    result = { ok: false, status: 0 };
  }
  if (!result.ok) {
    tab?.close();
    deps.notify(artifactOpenFailure(result, deps.machineLabel, deps.machineLive?.() ?? false), result);
    return false;
  }
  if (deps.knownLocalOnly) {
    // No tab was opened, and one opened after an await would be blocked.
    deps.notify("This file is in Cloud now. Open it again to view it.", result);
    return false;
  }
  if (!tab) {
    deps.notify("Your browser blocked the new tab. Allow pop-ups for Commander and open the file again.", result);
    return false;
  }
  tab.location.href = result.view.url;
  return true;
}
