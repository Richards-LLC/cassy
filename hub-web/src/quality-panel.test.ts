// @vitest-environment jsdom
import { afterEach, beforeAll, expect, it, vi } from "vitest";
import { QualitySheet } from "./quality-panel";
import type { LedgerRelease } from "./quality-model";

beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function() { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close = function() { this.removeAttribute("open"); this.dispatchEvent(new Event("close")); };
});
afterEach(() => { document.body.innerHTML = ""; });

const turns = { wakes: 1, no_op_wakes: 0, input_tokens: 1e6, output_tokens: 0 };
const release = (name: string, to: string | null, fields: Partial<LedgerRelease>): LedgerRelease => ({
  release: name, published: to !== null, from: "2026-11-10T00:00:00Z", to, turns, interventions_total: 0, interventions: {}, checks: {}, escapes: [], friction: [], ...fields,
});

it("opens a release's merged deliveries and friction, with recorded links, from a metric's data", async () => {
  const sheet = new QualitySheet({
    load: async () => ({ releases: [
      release("v4.12.0", "2026-11-10T12:00:00Z", { deliveries: { unknown: "the merge record starts inside this window" } }),
      release("unreleased", null, {
        deliveries: 2,
        merged: [{ task: "cas-a1", title: "First", merged_at: "2026-11-10T13:00:00Z" }, { task: "cas-b2", title: "Second", merged_at: "2026-11-10T14:00:00Z" }],
        friction: [{ reference: "Richards-LLC/cassy/issues/900", link: "https://github.com/Richards-LLC/cassy/issues/900", severity: "p1", title: "Gate loops", state: "open" }],
      }),
    ] }),
    returnFocus: vi.fn(),
  });
  sheet.open("cas-src");
  await vi.waitFor(() => expect(document.querySelector("[data-quality-release]")).not.toBeNull());
  const records = document.querySelector<HTMLElement>(".quality-records")!;
  expect(records.hidden).toBe(true);

  document.querySelector<HTMLButtonElement>('.quality-rubric [data-quality-release="1"]')!.click();
  expect(records.hidden).toBe(false);
  expect([...records.querySelectorAll<HTMLElement>("[data-task]")].map(row => row.dataset.task)).toEqual(["cas-a1", "cas-b2"]);
  expect(records.querySelector<HTMLAnchorElement>('[data-reference] a')!.href).toBe("https://github.com/Richards-LLC/cassy/issues/900");
  expect(document.activeElement).toBe(records.querySelector("#quality-records-title"));

  // A window the merge record does not cover lists no merges rather than a partial count.
  document.querySelector<HTMLButtonElement>('.quality-rubric [data-quality-release="0"]')!.click();
  expect(records.querySelectorAll("[data-task]")).toHaveLength(0);
});
