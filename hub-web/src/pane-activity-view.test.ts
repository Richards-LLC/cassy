// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { fitPaneActivityCaption, fitPaneActivityCaptions, updatePaneActivityCaption } from "./pane-activity-view";

afterEach(() => { document.body.replaceChildren(); vi.restoreAllMocks(); });

const earlier = { text: "Earlier output", title: "Output from before this page opened; nothing new since" };
function caption(connected = true) {
  const element = document.createElement("span"); element.className = "pane-last-activity";
  if (connected) document.body.append(element);
  let box = new DOMRect(0, 0, 26, 17), words = new DOMRect(0, 17, 26, 17);
  vi.spyOn(element, "getBoundingClientRect").mockImplementation(() => box);
  vi.spyOn(element, "getClientRects").mockImplementation(() => [box] as unknown as DOMRectList);
  const createRange = document.createRange.bind(document);
  vi.spyOn(document, "createRange").mockImplementation(() => {
    const range = createRange();
    Object.defineProperty(range, "getBoundingClientRect", { value: () => words });
    return range;
  });
  return { element, geometry: (width: number, textWidth: number, top = 0) => {
    box = new DOMRect(0, 0, width, 17); words = new DOMRect(0, top, textWidth, 17);
  } };
}

describe("whole-or-absent activity uses real text geometry (cas-846c)", () => {
  it("removes a below-line caption from reading/AX content, retaining its tooltip and slot", () => {
    const { element } = caption(); updatePaneActivityCaption(element, earlier);
    expect(element.textContent).toBe(""); expect(element.getAttribute("aria-hidden")).toBe("true");
    expect(element.title).toBe(earlier.title); expect(element.getBoundingClientRect().width).toBe(26);
  });
  it("restores the long caption on wider layout and hides it again on reverse resize", () => {
    const { element, geometry } = caption(); updatePaneActivityCaption(element, earlier);
    geometry(110, 100); fitPaneActivityCaptions(document);
    expect(element.textContent).toBe(earlier.text); expect(element.hasAttribute("aria-hidden")).toBe(false);
    geometry(26, 100, 17); fitPaneActivityCaptions(document);
    expect(element.textContent).toBe(""); expect(element.getAttribute("aria-hidden")).toBe("true");
  });
  it("a new short live-output label recovers without losing its current tooltip", () => {
    const { element, geometry } = caption(); updatePaneActivityCaption(element, earlier);
    geometry(26, 24); updatePaneActivityCaption(element, { text: "now", title: "2026-10-04T02:00:00.000Z" });
    expect(element.textContent).toBe("now"); expect(element.hasAttribute("aria-hidden")).toBe(false);
    expect(element.title).toBe("2026-10-04T02:00:00.000Z");
  });
  it("rejects fractional clipping that rounded scroll widths could miss", () => {
    const { element, geometry } = caption(); geometry(100, 100.05);
    updatePaneActivityCaption(element, earlier); expect(element.textContent).toBe("");
    geometry(100, 99.99); fitPaneActivityCaption(element);
    expect(element.textContent).toBe(earlier.text); expect(element.hasAttribute("aria-hidden")).toBe(false);
  });
  it("recovers after detached creation is placed in the real header", () => {
    const { element, geometry } = caption(false); geometry(110, 100);
    updatePaneActivityCaption(element, earlier); expect(element.textContent).toBe("");
    document.body.append(element); fitPaneActivityCaption(element);
    expect(element.textContent).toBe(earlier.text); expect(element.title).toBe(earlier.title);
  });
});
