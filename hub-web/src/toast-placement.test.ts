import { describe, expect, it } from "vitest";
import { toastPlacementInThread, toastTopClearOfBanner, type Box } from "./toast-placement";

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

