// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { InboxSnapshot, InboxState, OperatorInboxController } from "./controller";
import { InboxView, browserLabel, unverifiedCount, withheldCopy } from "./inbox-view";
import type { InboxEvent } from "./store";
import type { PresenceSnapshot } from "./presence";

function event(eventId: string, verification: InboxEvent["verification"]): InboxEvent {
  return {
    accountId: "acct",
    feedGeneration: "g1",
    sequence: eventId,
    eventId,
    scope: "session",
    producerKind: "principal",
    hubId: "hub-1",
    projectId: "proj",
    sessionId: "sess",
    keyEpoch: "e1",
    digest: "d",
    storedAt: "2026-10-05T12:00:00Z",
    expiresAt: "2027-01-03T12:00:00Z",
    plaintext: null,
    verification,
    failure: verification === "verified" ? null : "open_failed",
    acked: false,
  };
}

function view(state: InboxState, events: InboxEvent[] = [], presence?: PresenceSnapshot, manage = false) {
  const snapshot: InboxSnapshot = { state, events, commands: [], machines: presence ? [{ machineId: "machine", hubId: "hub-1", label: "Atlas", status: "active", projects: [] }] : [], generationWarning: null, expiredThrough: null, presence };
  const controller = {
    subscribe: () => () => undefined,
    current: () => state,
    snapshot: async () => snapshot,
    beginSignIn: vi.fn(async () => undefined),
    pollSignIn: vi.fn(async () => null),
    commandScopes: () => [],
    machines: async () => [],
    refreshPresence: vi.fn(async () => undefined),
    canManageMonitoring: () => manage,
    setMonitoring: vi.fn(async () => undefined),
  };
  const inbox = new InboxView(controller as unknown as OperatorInboxController, { defaultLabel: "This browser" });
  (inbox as unknown as { snapshot: InboxSnapshot }).snapshot = snapshot;
  inbox.render();
  return { inbox, controller };
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("operator inbox view (cas-9b7d QA round 1)", () => {
  it("names an unavailable observer and separate component status without claiming power state", () => {
    const presence: PresenceSnapshot = {
      observerStatus: "unavailable", observerCheckedAt: null,
      machines: [{ machineId: "machine", hubId: "hub-1", monitoring: "enabled", monitoringGeneration: "1", presence: "observed",
        lastReportAt: "2026-10-06T11:00:00Z", leaseExpiresAt: null, deadlineAt: null, silence: null, openOutage: null,
        components: [{ component: "serve", state: "degraded", observedAt: "2026-10-06T10:59:59Z" }],
      }],
    };
    const { inbox } = view({ kind: "ready", accountHint: null, label: "Phone" }, [], presence);
    const text = inbox.dialog.textContent!;
    expect(text).toContain("Observer unavailable");
    expect(text).toContain("Reporting to Cassy Cloud");
    expect(text).toContain("Serve: degraded");
    expect(text).toContain("Last report");
    expect(text).not.toMatch(/powered off|sleeping/);
    expect(inbox.dialog.querySelector('[id^="presence-toggle-"]')).toBeNull();
  });

  it("keeps keyboard focus and consent disclosure through a snapshot update, and names the account action", async () => {
    const presence: PresenceSnapshot = {
      observerStatus: "ok", observerCheckedAt: new Date().toISOString(),
      machines: [{ machineId: "machine", hubId: "hub-1", monitoring: "disabled", monitoringGeneration: "0", presence: null,
        lastReportAt: null, leaseExpiresAt: null, deadlineAt: null, silence: null, openOutage: null, components: [],
      }],
    };
    const { inbox, controller } = view({ kind: "ready", accountHint: null, label: "Phone" }, [], presence, true);
    const details = inbox.dialog.querySelector<HTMLDetailsElement>("#presence-consent-machine")!;
    details.open = true;
    const summary = inbox.dialog.querySelector<HTMLElement>("#presence-details-machine")!;
    summary.tabIndex = 0;
    summary.focus();
    inbox.render();
    expect(inbox.dialog.querySelector<HTMLDetailsElement>("#presence-consent-machine")!.open).toBe(true);
    expect(document.activeElement?.id).toBe("presence-details-machine");
    const enable = inbox.dialog.querySelector<HTMLButtonElement>("#presence-toggle-machine")!;
    expect(enable.textContent).toBe("Enable alerts for Atlas");
    expect(details.textContent).toContain("90 days");
    enable.click();
    await vi.waitFor(() => expect(controller.setMonitoring).toHaveBeenCalledWith("machine", true));
  });

  it("renders a verified machine notice outside conversation bubbles", () => {
    const notice: InboxEvent = { ...event("1", "verified"), scope: "machine", producerKind: "cloud_observer", projectId: null, sessionId: null,
      plaintext: { type: "psc.operator.machine_presence", v: 1, account_id: "acct", machine_id: "machine", hub_id: "hub-1", kind: "machine_unobserved", outage_epoch: "1", ref_event_id: null, detected_at: "2026-10-06T11:00:00Z" },
    };
    const { inbox } = view({ kind: "ready", accountHint: null, label: "Phone" }, [notice]);
    expect(inbox.dialog.querySelector('[data-presence-event="1"]')?.textContent).toContain("Machine unreachable");
    expect(inbox.dialog.querySelector(".operator-inbox-bubble")).toBeNull();
  });
  it("F02: the browser name and Sign in are one form, so Enter in the field signs in", async () => {
    const { inbox, controller } = view({ kind: "signed_out" });
    const input = inbox.dialog.querySelector<HTMLInputElement>("#operator-inbox-label")!;
    const button = inbox.dialog.querySelector<HTMLButtonElement>("#operator-inbox-signin")!;
    const form = input.closest("form")!;
    expect(form).not.toBeNull();
    expect(button.form).toBe(form);
    expect(button.type).toBe("submit");
    input.value = "  Kitchen laptop ";
    const submit = new Event("submit", { cancelable: true });
    form.dispatchEvent(submit);
    expect(submit.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(controller.beginSignIn).toHaveBeenCalledWith("Kitchen laptop"));
  });

  it("F03: an envelope that failed to open or verify is named, never shown", () => {
    const events = [event("1", "verified"), event("2", "undecryptable"), event("3", "unverified")];
    expect(unverifiedCount({ state: { kind: "signed_out" }, events, commands: [], machines: [], generationWarning: null, expiredThrough: null })).toBe(2);
    expect(withheldCopy(1)).toBe("1 message couldn’t be verified, so it isn’t shown.");
    expect(withheldCopy(2)).toBe("2 messages couldn’t be verified, so they aren’t shown.");

    const { inbox } = view({ kind: "ready", accountHint: null, label: "This browser" }, [event("2", "undecryptable")]);
    const hint = inbox.dialog.querySelector(".operator-inbox-withheld");
    expect(hint?.textContent).toBe("1 message couldn’t be verified, so it isn’t shown.");
    expect(hint?.getAttribute("role")).toBe("status");
  });

  it("F03: a fully verified inbox carries no withheld line", () => {
    const { inbox } = view({ kind: "ready", accountHint: null, label: "This browser" }, [event("1", "verified")]);
    expect(inbox.dialog.querySelector(".operator-inbox-withheld")).toBeNull();
  });
});

describe("operator inbox copy (cas-97d58 F21)", () => {
  it("names this browser the way a person would", () => {
    expect(browserLabel("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36")).toBe("Chrome on Linux");
    expect(browserLabel("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1")).toBe("Safari on iPhone");
    expect(browserLabel("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36 Edg/130.0")).toBe("Edge on Windows");
    expect(browserLabel("")).toBe("Browser on this device");
  });

  it("says who can read the messages and keeps the command line behind a disclosure", () => {
    const { inbox } = view({ kind: "signed_out" });
    expect(inbox.dialog.textContent).toContain("Cassy Cloud can read them as well as you");
    expect(inbox.dialog.textContent).not.toContain("Petra Stella");
    const approval = view({ kind: "awaiting_approval", userCode: "L72E-G3B8", approvalUrl: "https://cloud.example/approve", expiresAt: new Date(Date.now() + 600_000).toISOString() });
    const details = approval.inbox.dialog.querySelector<HTMLDetailsElement>("details.operator-inbox-cli")!;
    expect(details.open).toBe(false);
    expect(details.querySelector("code")?.textContent).toBe("cas hub operator approve L72E-G3B8");
  });
});
