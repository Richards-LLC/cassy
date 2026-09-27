import { describe, expect, it } from "vitest";
import { toastPlacementInThread, toastTopAboveAction, toastTopClearOfBanner, type Box } from "./toast-placement";

const box = (left: number, top: number, width: number, height: number): Box => ({ left, top, width, height, right: left + width, bottom: top + height });

describe("toastTopClearOfBanner (cas-00cc)", () => {
  const banner = box(8, 64, 374, 40); // phone: the banner just below the 56 px thread header
  it("moves a toast that would land on the banner to just below it", () => {
    expect(toastTopClearOfBanner(64, box(12, 64, 366, 46), banner)).toBe(112);
  });
  it("leaves a toast that does not touch the banner where the stylesheet put it", () => {
    expect(toastTopClearOfBanner(12, box(12, 12, 366, 46), banner)).toBeUndefined(); // on the list, above it
    expect(toastTopClearOfBanner(16, box(1075, 16, 189, 46), box(390, 136, 620, 40))).toBeUndefined(); // desktop: other column
  });
  it("ignores a missing or hidden banner", () => {
    expect(toastTopClearOfBanner(64, box(12, 64, 366, 46), undefined)).toBeUndefined();
    expect(toastTopClearOfBanner(64, box(12, 64, 366, 46), box(0, 0, 0, 0))).toBeUndefined();
  });
});

describe("toast below the list brand row (journey F8, dist 3126b032)", () => {
  // Phone 390: the list's top row (brand + appearance) spans 12–56 px.
  const brandRow = box(12, 12, 366, 44);
  it("drops a toast at the phone's top edge below the brand row", () => {
    expect(toastTopClearOfBanner(12, box(12, 12, 366, 46), brandRow)).toBe(64);
  });
  it("leaves the desktop toast alone: the list column is not under it", () => {
    expect(toastTopClearOfBanner(16, box(1075, 16, 189, 46), box(16, 16, 328, 44))).toBeUndefined();
  });
  it("ignores a brand row scrolled off the top", () => {
    expect(toastTopClearOfBanner(12, box(12, 12, 366, 46), box(12, -80, 366, 44))).toBeUndefined();
  });
});

describe("toastPlacementInThread (3.30.0 journey F8)", () => {
  // Desktop 1280: list 0–360, thread 360–960, context rail 960–1280.
  const header = box(360, 0, 600, 96);
  const column = box(360, 0, 600, 800);
  it("puts the toast in the thread column, just below its header, clear of the context rail", () => {
    expect(toastPlacementInThread(header, column, 1280)).toEqual({ top: 104, right: 328 });
  });
  it("leaves the phone layout (a full-width thread) to the stylesheet", () => {
    expect(toastPlacementInThread(box(0, 0, 390, 56), box(0, 0, 390, 844), 390)).toBeUndefined();
  });
  it("leaves the stylesheet's position when no conversation is open or it is not laid out", () => {
    expect(toastPlacementInThread(undefined, column, 1280)).toBeUndefined();
    expect(toastPlacementInThread(header, undefined, 1280)).toBeUndefined();
    expect(toastPlacementInThread(box(0, 0, 0, 0), column, 1280)).toBeUndefined();
  });
  it("never pushes the toast off the right edge", () => {
    expect(toastPlacementInThread(box(360, 0, 920, 96), box(360, 0, 930, 800), 1280)!.right).toBe(8);
  });
});


describe("toastTopAboveAction (cas-71af, dfb2 QA F01)", () => {
  const box = (top: number, height: number, left = 12, width = 366): Box => ({ top, bottom: top + height, left, right: left + width, width, height });
  it("puts the toast just above the list's bottom action, clear of the title row", () => {
    expect(toastTopAboveAction(box(0, 46), box(752, 44), box(76, 40))).toBe(752 - 8 - 46);
  });
  it("gives up when there is no action on screen or no room below the title row", () => {
    expect(toastTopAboveAction(box(0, 46), undefined, box(76, 40))).toBeUndefined();
    expect(toastTopAboveAction(box(0, 46), box(0, 0, 0, 0), box(76, 40))).toBeUndefined();
    expect(toastTopAboveAction(box(0, 46), box(150, 44), box(76, 40))).toBeUndefined();
  });
});
