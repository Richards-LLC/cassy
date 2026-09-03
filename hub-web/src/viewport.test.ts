import { describe, expect, it } from "vitest";
import {
  isLandscapePhone,
  isPhoneLayout,
  LANDSCAPE_PHONE_MEDIA_QUERY,
  PHONE_MAX_SHORT_AXIS_REM,
  PHONE_MAX_WIDTH_REM,
  PHONE_MEDIA_QUERY,
  ROOT_FONT_PX,
} from "./viewport";

const pixel7 = { width: 412, height: 915, coarsePointer: true };
const pixel7Landscape = { width: 915, height: 412, coarsePointer: true };
const laptop = { width: 1400, height: 900, coarsePointer: false };

describe("Cassy Commander phone detection", () => {
  it("treats a phone as a phone in both orientations", () => {
    // 915x412 is a Pixel 7 rotated. Keyed on width alone it cleared the 848px
    // breakpoint and rendered the three-column desktop console on a 412px-tall
    // screen (report defect D5).
    expect(isPhoneLayout(pixel7)).toBe(true);
    expect(isPhoneLayout(pixel7Landscape)).toBe(true);
  });

  it("leaves a laptop and a short desktop window on the desktop layout", () => {
    expect(isPhoneLayout(laptop)).toBe(false);
    // A desktop window dragged short is still driven by a mouse. Without the
    // pointer clause, resizing a browser would hand a desktop the phone chrome.
    expect(isPhoneLayout({ width: 1280, height: 400, coarsePointer: false })).toBe(false);
    // A touchscreen laptop keeps the desktop layout while it has the room.
    expect(isPhoneLayout({ width: 1280, height: 800, coarsePointer: true })).toBe(false);
  });

  it("keeps the documented breakpoint exactly where it was for narrow viewports", () => {
    const breakpoint = PHONE_MAX_WIDTH_REM * ROOT_FONT_PX;
    expect(breakpoint).toBe(848);
    expect(isPhoneLayout({ width: breakpoint, height: 1000, coarsePointer: false })).toBe(true);
    expect(isPhoneLayout({ width: breakpoint + 1, height: 1000, coarsePointer: false })).toBe(false);
  });

  it("names landscape only when the short axis is the one that ran out", () => {
    expect(isLandscapePhone(pixel7Landscape)).toBe(true);
    expect(isLandscapePhone(pixel7)).toBe(false);
    expect(isLandscapePhone(laptop)).toBe(false);
    const shortAxis = PHONE_MAX_SHORT_AXIS_REM * ROOT_FONT_PX;
    expect(shortAxis).toBe(480);
    expect(isLandscapePhone({ width: 915, height: shortAxis, coarsePointer: true })).toBe(true);
    expect(isLandscapePhone({ width: 915, height: shortAxis + 1, coarsePointer: true })).toBe(false);
  });

  it("states the same rule in the media queries the stylesheet uses", () => {
    // One definition, consumed by both the stylesheet and matchMedia. The old
    // code kept "max-width: 53rem" in CSS and "max-width: 850px" in JS, so the
    // layout and the pane-mounting logic could already disagree by two pixels —
    // and would have disagreed completely once rotation entered the picture.
    expect(PHONE_MEDIA_QUERY).toBe("(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)");
    expect(LANDSCAPE_PHONE_MEDIA_QUERY).toBe("(max-height: 30rem) and (pointer: coarse)");
    expect(PHONE_MEDIA_QUERY).toContain(LANDSCAPE_PHONE_MEDIA_QUERY);
  });
});
