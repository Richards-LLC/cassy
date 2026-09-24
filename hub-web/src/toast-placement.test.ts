import { describe, expect, it } from "vitest";
import { toastTopClearOfBanner, type Box } from "./toast-placement";

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
