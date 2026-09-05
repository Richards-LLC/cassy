import { describe, expect, it } from "vitest";
import { FirstConnectionAnnouncer, installPairedMachine } from "./first-connection";

const live = { phase: "live" as const, degraded: false };
const degraded = { phase: "live" as const, degraded: true };
const dialing = { phase: "dialing" as const, degraded: false };
const backoff = { phase: "backoff" as const, degraded: false };
const failed = { phase: "failed" as const, degraded: false };

describe("saved-to-live notification lifecycle (cas-8051 F8)", () => {
  it("announces connected once, only after a healthy live phase", () => {
    const announcer = new FirstConnectionAnnouncer();
    announcer.expect("m1");
    expect(announcer.observe("m1", "Studio Mac", dialing)).toBeUndefined();
    expect(announcer.observe("m1", "Studio Mac", degraded)).toBeUndefined();
    expect(announcer.observe("m1", "Studio Mac", live)).toBe("Studio Mac connected");
    // Heartbeats keep the phase live; nothing more is said.
    expect(announcer.observe("m1", "Studio Mac", live)).toBeUndefined();
    expect(announcer.isPending("m1")).toBe(false);
  });

  it("says nothing for a machine that failed or never connected", () => {
    const announcer = new FirstConnectionAnnouncer();
    announcer.expect("m1");
    expect(announcer.observe("m1", "Attic Linux", failed)).toBeUndefined();
    expect(announcer.observe("m1", "Attic Linux", backoff)).toBeUndefined();
    // Still owed: if it does come up later, that is the first connection.
    expect(announcer.isPending("m1")).toBe(true);
    expect(announcer.observe("m1", "Attic Linux", live)).toBe("Attic Linux connected");
  });

  it("does not re-announce a reconnect after a disconnect", () => {
    const announcer = new FirstConnectionAnnouncer();
    announcer.expect("m1");
    expect(announcer.observe("m1", "Studio Mac", live)).toBe("Studio Mac connected");
    expect(announcer.observe("m1", "Studio Mac", backoff)).toBeUndefined();
    expect(announcer.observe("m1", "Studio Mac", dialing)).toBeUndefined();
    expect(announcer.observe("m1", "Studio Mac", live)).toBeUndefined();
  });

  it("keeps one announcement per machine when a second is paired before the first connects", () => {
    const announcer = new FirstConnectionAnnouncer();
    announcer.expect("m1");
    announcer.expect("m2");
    expect(announcer.observe("m2", "Attic Linux", live)).toBe("Attic Linux connected");
    expect(announcer.observe("m1", "Studio Mac", live)).toBe("Studio Mac connected");
  });

  it("announces a re-paired machine's new credential once more", () => {
    const announcer = new FirstConnectionAnnouncer();
    announcer.expect("m1");
    expect(announcer.observe("m1", "Studio Mac", live)).toBe("Studio Mac connected");
    // Re-pair: a replacement credential is saved and its connection restarts.
    announcer.expect("m1");
    expect(announcer.observe("m1", "Studio Mac", dialing)).toBeUndefined();
    expect(announcer.observe("m1", "Studio Mac", live)).toBe("Studio Mac connected");
    expect(announcer.observe("m1", "Studio Mac", live)).toBeUndefined();
  });

  it("owes nothing for a live phase that arrives before the expectation is registered", () => {
    // The wiring registers expect() before the connection is created; if it
    // did not, a fast local hub's first live would be swallowed here.
    const announcer = new FirstConnectionAnnouncer();
    expect(announcer.observe("m1", "Studio Mac", live)).toBeUndefined();
    announcer.expect("m1");
    expect(announcer.observe("m1", "Studio Mac", live)).toBe("Studio Mac connected");
  });

  it("forgets a machine removed before it connected and ignores machines it never expected", () => {
    const announcer = new FirstConnectionAnnouncer();
    announcer.expect("m1");
    announcer.forget("m1");
    expect(announcer.observe("m1", "Studio Mac", live)).toBeUndefined();
    expect(announcer.observe("m9", "Unknown", live)).toBeUndefined();
  });
});

/**
 * The installation seam, executed: a connection starter stands in for
 * replaceMachineConnection + createConnection and drives onState exactly as
 * the real supervisor would — including synchronously.
 */
describe("installation seam orchestration (review 25649)", () => {
  interface Machine { id: string; label: string }

  function seam() {
    const announcer = new FirstConnectionAnnouncer();
    const notices: string[] = [];
    const onState = (machine: Machine, state: { phase: "live" | "backoff" | "failed" | "dialing"; degraded: boolean }) => {
      const notice = announcer.observe(machine.id, machine.label, state);
      if (notice) notices.push(notice);
    };
    return { announcer, notices, onState };
  }

  it("yields exactly [Access saved, connected] when the connection reports healthy live synchronously", () => {
    const { announcer, notices, onState } = seam();
    installPairedMachine({ id: "m1", label: "Studio Mac" }, {
      announcer,
      notify: (text) => notices.push(text),
      startConnection: (machine) => { onState(machine, { phase: "live", degraded: false }); },
    });
    expect(notices).toEqual(["Access saved — connecting to Studio Mac…", "Studio Mac connected"]);
    // Heartbeats after that say nothing more.
    onState({ id: "m1", label: "Studio Mac" }, { phase: "live", degraded: false });
    expect(notices).toHaveLength(2);
  });

  it("says only Access saved when the connection goes offline", () => {
    const { announcer, notices, onState } = seam();
    installPairedMachine({ id: "m1", label: "Attic Linux" }, {
      announcer,
      notify: (text) => notices.push(text),
      startConnection: (machine) => { onState(machine, { phase: "dialing", degraded: false }); onState(machine, { phase: "failed", degraded: false }); onState(machine, { phase: "backoff", degraded: false }); },
    });
    expect(notices).toEqual(["Access saved — connecting to Attic Linux…"]);
    // Coming up later is still that machine's first connection.
    onState({ id: "m1", label: "Attic Linux" }, { phase: "live", degraded: false });
    expect(notices).toEqual(["Access saved — connecting to Attic Linux…", "Attic Linux connected"]);
  });

  it("names the installed machine, not the selected one, and ignores the selected machine's live", () => {
    const { announcer, notices, onState } = seam();
    const selected: Machine = { id: "m-selected", label: "Studio Mac" };
    const installed: Machine = { id: "m-new", label: "Attic Linux" };
    installPairedMachine(installed, {
      announcer,
      notify: (text) => notices.push(text),
      // The connection layer keeps reporting the previously selected machine.
      startConnection: () => { onState(selected, { phase: "live", degraded: false }); },
    });
    expect(notices).toEqual(["Access saved — connecting to Attic Linux…"]);
    onState(installed, { phase: "live", degraded: false });
    expect(notices).toEqual(["Access saved — connecting to Attic Linux…", "Attic Linux connected"]);
  });

  it("starts the connection only after the announcement is armed", () => {
    const { announcer, notices } = seam();
    const order: string[] = [];
    installPairedMachine({ id: "m1", label: "Studio Mac" }, {
      announcer,
      notify: (text) => { notices.push(text); order.push("notify"); },
      startConnection: (machine) => { order.push(`start:${announcer.isPending(machine.id) ? "armed" : "unarmed"}`); },
    });
    expect(order).toEqual(["notify", "start:armed"]);
  });
});
