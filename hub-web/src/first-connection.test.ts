import { describe, expect, it } from "vitest";
import { FirstConnectionAnnouncer } from "./first-connection";

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

  it("forgets a machine removed before it connected and ignores machines it never expected", () => {
    const announcer = new FirstConnectionAnnouncer();
    announcer.expect("m1");
    announcer.forget("m1");
    expect(announcer.observe("m1", "Studio Mac", live)).toBeUndefined();
    expect(announcer.observe("m9", "Unknown", live)).toBeUndefined();
  });
});
