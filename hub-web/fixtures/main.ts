import "../src/styles.css";
import "../src/glass.css";
import { renderAttentionNoticeFixture } from "./attention-notice";
import { renderFleetOpsFixture } from "./fleet-ops";
import { renderConversationFixture } from "./conversations";
import { pairingDialogCancellationActive } from "../src/pairing-dialog";
import { pairingExchangeFailure } from "../src/pairing-messages";
import { pairDialogMarkup } from "../src/pair-dialog-markup";
import { createPairingDraft } from "../src/pairing-draft";
import { DEFAULT_PAIRING_SCOPES } from "../src/pairing-relay";
import type { Scope } from "../src/types";
import { LaunchSheet, type LaunchHost, type LaunchResult } from "../src/launch-session";

export const FIXTURE_NAMES = [
  "paired-machines", "paired-machines-down", "conversations-machine-down-long", "conversations-machine-label-overlong", "conversations-list", "conversation", "conversation-replied", "conversation-error",
  "conversation-thread", "conversation-evidence",
  "conversation-ask", "conversation-ask-answered", "conversation-blocker", "conversation-pairs",
  "conversation-attachment", "conversation-empty", "conversation-empty-long-machine", "conversation-composer", "conversation-keyboard",
  "conversation-sessions", "conversation-earlier", "conversation-dated", "conversation-clock-ahead", "conversations-sessions",
  "conversations-session-ended",
  "conversations-session-end-error",
  "conversation-needs-pairing",
  "conversation-interrupt-unavailable",
  "conversation-raw-output",
  "attention-notice-details",
  "fleet-ops-menu",
  "fleet-ops-confirm",
  "fleet-ops-undo",
  "fleet-ops-write-access",
  "fleet-ops-write-access-confirm",
  "connection-failed-retry",
  "connection-fatal-browser",
  "pairing-step-1",
  "pairing-email",
  "pairing-code",
  "pairing-cleanup",
  "conversation-long-status",
  "conversation-loading-earlier",
  "conversation-opening",
  "conversation-mic-idle",
  "conversation-mic-listening",
  "conversation-mic-unavailable",
  "conversation-draft-too-long",
  "conversation-draft-not-saved",
  "conversation-unconfirmed-dismissed",
  "conversations-loading",
  "conversations-unpaired",
  "launch-form",
  "launch-browse",
  "launch-error",
  "launch-starting",
  "launch-grant",
  "launch-offline",
  "launch-account",
  "launch-account-unavailable",
  "launch-account-default-out",
  "launch-grant-command",
] as const;

export type FixtureName = (typeof FIXTURE_NAMES)[number];

const fixtureParam = new URLSearchParams(window.location.search).get("fixture") ?? "conversations-list";
const fixtureName: FixtureName = FIXTURE_NAMES.includes(fixtureParam as FixtureName)
  ? fixtureParam as FixtureName
  : "conversations-list";

const appRoot = document.querySelector<HTMLDivElement>("#app");
if (!appRoot) throw new Error("Fixture shell is missing #app");
const app: HTMLDivElement = appRoot;

/** The app's toast region at rest: present, hidden from assistive tech, as main.ts leaves it between notices. */
function renderRestingToast(): void {
  const toast = document.createElement("div");
  toast.textContent = "A previous hub notice is resting.";
  toast.id = "toast";
  toast.setAttribute("role", "status");
  toast.setAttribute("aria-hidden", "true");
  document.body.append(toast);
}

type PairingFixture = "pairing-step-1" | "pairing-email" | "pairing-code" | "pairing-cleanup";

/**
 * The production pairing dialog (the same pairDialogMarkup main.ts renders)
 * for one fixture state: the entry step with its email field, the email field
 * live under focus, the relay code, and the cleanup step.
 */
function renderPairing(view: PairingFixture): HTMLDialogElement {
  const origin = window.location.origin;
  const cleanup = view === "pairing-cleanup";
  const code = view === "pairing-code";
  const template = document.createElement("template");
  template.innerHTML = pairDialogMarkup({
    cleanupFailed: cleanup,
    cleanupContext: { cause: "failure", storeOpen: true, rollbackPending: true },
    pendingPairing: code
      ? { kind: "relay-request", pairingRequestId: "fixture", userCode: "K7MW-4H2Q", pollSecret: "fixture", controllerOrigin: origin, requestedScopes: DEFAULT_PAIRING_SCOPES, expiresAt: new Date(Date.now() + 600_000).toISOString(), interval: 5 }
      : null,
    draft: createPairingDraft(origin),
    status: cleanup ? "Pairing failed; cleanup is still being verified." : code ? "Waiting for a machine to claim the code…" : "",
    createInFlight: false,
    exchangeInFlight: false,
    relayOrigin: "https://petra-stella-cloud.vercel.app",
    pageOrigin: origin,
  });
  const dialog = template.content.querySelector<HTMLDialogElement>("#pair-dialog");
  if (!dialog) throw new Error("pairDialogMarkup rendered no #pair-dialog");
  // Exercise the production cancellation policy at the fixture seam. The
  // result is intentionally not shown; it prevents a fixture from drifting
  // into a flow that the real dialog would not allow.
  dialog.dataset.cancellationActive = String(pairingDialogCancellationActive({ createInFlight: false, exchangeInFlight: false, hasPendingPairing: code || cleanup }));
  const failure = pairingExchangeFailure({ status: 502, body: "", controllerOrigin: origin });
  dialog.dataset.failureCopy = failure.message;
  return dialog;
}

function appendOpenPairingDialog(view: PairingFixture): void {
  const dialog = renderPairing(view);
  app.append(dialog);
  dialog.showModal();
  if (!dialog.open) throw new Error("Pairing fixture dialog did not open");
  if (view === "pairing-email") {
    // The live field: focused and empty, so its placeholder and edge are what is measured (D4).
    const email = dialog.querySelector<HTMLInputElement>("#pair-email");
    if (!email) throw new Error("Production pairing dialog has no #pair-email field");
    email.focus();
  }
}

type LaunchFixture = "launch-form" | "launch-browse" | "launch-error" | "launch-starting" | "launch-grant" | "launch-grant-command" | "launch-offline" | "launch-account" | "launch-account-unavailable" | "launch-account-default-out";

/**
 * The production New session sheet (LaunchSheet, cas-0f51) driven through its
 * own controls to one state, over a host that answers like a hub.
 */
async function openLaunchSheet(view: LaunchFixture): Promise<void> {
  const control: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];
  const target = (id: string) => ({ kind: "project" as const, id });
  const root = { id: "root-code", name: "code", path: "/home/dev/code" };
  const host: LaunchHost = {
    machines: () => [
      // cas-0e14: the offline view's machine is reconnecting.
      { id: "atlas", label: "Atlas · Linux", scopes: [...control, "session-launch"], ...(view === "launch-offline" ? { connection: "Reconnecting" } : {}) },
      // cas-cee5: a read-only pairing cannot allow launch here, so the sheet shows the command to run on the machine.
      { id: "studio", label: "Studio Mac · macOS", scopes: view === "launch-grant-command" ? ["machine-read", "session-read"] : control },
    ],
    currentMachineId: () => (view === "launch-grant" || view === "launch-grant-command" ? "studio" : "atlas"),
    origin: window.location.origin,
    projects: async () => ({
      projects: [
        { id: "p-ledger", name: "ledger-api", path: "/home/dev/ledger-api", last_touched_at: "2026-09-28T08:00:00Z", touch_count: 12, running_session: null, target: target("p-ledger") },
        { id: "p-cas", name: "cas-src", path: "/home/dev/cas-src", last_touched_at: "2026-09-27T18:00:00Z", touch_count: 90, running_session: "patient-pelican-9", target: target("p-cas") },
        { id: "p-old", name: "old-notes-with-a-rather-long-project-name", path: "/home/dev/archive/2025/old-notes-with-a-rather-long-project-name", last_touched_at: "2026-03-01T09:00:00Z", touch_count: 2, running_session: null, target: target("p-old") },
      ],
      browse_roots: [root],
    }),
    profiles: async () => ({
      claude: { installed: true, profiles: [
        // cas-c107: the default account logged out, so one row shows the
        // account's "Default" beside the logged-out Copy control.
        { name: "main", logged_in: view !== "launch-account-default-out", is_default: true },
        { name: "support@petrastella.io", logged_in: true, is_default: false },
        { name: "customer-success-escalations@petrastella-international.example", logged_in: true, is_default: false },
        { name: "old@petrastella.io", logged_in: false, is_default: false },
      ] },
      codex: { installed: true, profiles: [], error: "cli_probe_failed" },
      grok: { installed: true, profiles: [] },
    }),
    browse: async () => ({ root, path: "clients", truncated: true, entries: [
      { name: "archive", path: "clients/archive", launchable: false, project_id: null, target: null },
      { name: "acme-portal", path: "clients/acme-portal", launchable: true, project_id: null, target: { kind: "browse", root_id: root.id, path: "clients/acme-portal" } },
    ] }),
    launch: (): Promise<LaunchResult> => view === "launch-error"
      ? Promise.resolve({ ok: false, status: 422, code: "not_logged_in", detail: "claude: not logged in for profile main (run `claude /login`)" })
      : new Promise(() => {}),
    sessionListed: async () => false,
    open: () => {},
    copy: async () => {},
    grant: async () => {},
  };
  const sheet = new LaunchSheet(host);
  sheet.open(view === "launch-grant" || view === "launch-grant-command" ? "studio" : "atlas");
  const dialog = document.querySelector<HTMLDialogElement>("#launch-dialog");
  if (!dialog?.open) throw new Error("Launch fixture sheet did not open");
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
  await settle();
  if (view === "launch-browse") {
    dialog.querySelector<HTMLButtonElement>("#launch-tab-browse")!.click();
    await settle();
    dialog.querySelector<HTMLInputElement>('[data-launch-list="browse"] input[type=radio]')?.click();
    return;
  }
  if (view === "launch-account" || view === "launch-account-unavailable" || view === "launch-account-default-out") {
    dialog.querySelector<HTMLInputElement>('[data-launch-list="known"] input[type=radio]')!.click();
    if (view === "launch-account-unavailable") dialog.querySelector<HTMLInputElement>('input[name="launch-cli"][value="codex"]')!.click();
    else dialog.querySelector<HTMLInputElement>('input[name="launch-account"][value^="customer-success"]')!.click();
    return;
  }
  if (view === "launch-form" || view === "launch-error" || view === "launch-starting") {
    dialog.querySelector<HTMLInputElement>('[data-launch-list="known"] input[type=radio]')!.click();
    if (view === "launch-form") return;
    dialog.querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await settle();
    if (view === "launch-error") dialog.querySelector<HTMLDetailsElement>(".launch-error-detail")!.open = true;
  }
}

/**
 * A dialog fixture opens over the conversation list, where the app's Pair a
 * machine and New session controls live.
 */
function renderDialogFixture(): void {
  renderConversationFixture(app, "conversations-list");
  renderRestingToast();
  if (fixtureName.startsWith("pairing")) appendOpenPairingDialog(fixtureName as PairingFixture);
  if (fixtureName.startsWith("launch")) void openLaunchSheet(fixtureName as LaunchFixture);
}

/** The strict gate's self-test: ?broken=1 plants one deliberate contrast defect on the default fixture. */
function plantBrokenContrast(): void {
  const style = document.createElement("style");
  style.textContent = ".fixture-broken-contrast { color: var(--bg-panel); background: var(--bg-panel); }";
  document.head.append(style);
  const defect = document.createElement("p");
  defect.className = "fixture-broken-contrast";
  defect.textContent = "Deliberate contrast defect";
  document.querySelector("main")?.append(defect);
}

if (fixtureName === "attention-notice-details") renderAttentionNoticeFixture(app);
else if (fixtureName.startsWith("fleet-ops-")) renderFleetOpsFixture(app, fixtureName);
else if (fixtureName.startsWith("pairing") || fixtureName.startsWith("launch")) renderDialogFixture();
else renderConversationFixture(app, fixtureName);
if (fixtureName === "conversations-list" && new URLSearchParams(window.location.search).has("broken")) plantBrokenContrast();
