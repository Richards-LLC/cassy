// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { announceTo } from "./live-region";

/** Records every text the region holds, as a screen reader would hear it. */
function observed(region: HTMLElement): { heard: string[]; flush(): Promise<void> } {
  const heard: string[] = [];
  const observer = new MutationObserver(() => { if (region.textContent) heard.push(region.textContent); });
  observer.observe(region, { childList: true, characterData: true, subtree: true });
  return { heard, flush: () => new Promise((resolve) => setTimeout(resolve, 0)) };
}

describe("live-region announcements (cas-1380)", () => {
  it("announces a first result once and a repeated identical refusal again", async () => {
    const region = document.createElement("p");
    document.body.append(region);
    const frames: (() => void)[] = [];
    const nextFrame = (run: () => void) => { frames.push(run); };
    const { heard, flush } = observed(region);

    announceTo(region, "Enter the folder to grant.", { repeat: true }, nextFrame);
    await flush();
    expect(heard).toEqual(["Enter the folder to grant."]);
    expect(frames).toHaveLength(0);

    // The same refusal again: cleared now, set again on the next frame.
    announceTo(region, "Enter the folder to grant.", { repeat: true }, nextFrame);
    await flush();
    expect(region.textContent).toBe("");
    frames.splice(0).forEach((run) => run());
    await flush();
    expect(heard).toEqual(["Enter the folder to grant.", "Enter the folder to grant."]);
  });

  it("keeps the old no-repeat behaviour and never overwrites a newer announcement", async () => {
    const region = document.createElement("p");
    const frames: (() => void)[] = [];
    const nextFrame = (run: () => void) => { frames.push(run); };
    announceTo(region, "swift-lark-3 stopped.", {}, nextFrame);
    announceTo(region, "swift-lark-3 stopped.", {}, nextFrame);
    expect(frames).toHaveLength(0);
    expect(region.textContent).toBe("swift-lark-3 stopped.");

    announceTo(region, "swift-lark-3 stopped.", { repeat: true }, nextFrame);
    announceTo(region, "Write access granted.", {}, nextFrame);
    frames.splice(0).forEach((run) => run());
    expect(region.textContent).toBe("Write access granted.");
    // An empty text never schedules a repeat.
    announceTo(region, "", { repeat: true }, nextFrame);
    announceTo(region, "", { repeat: true }, nextFrame);
    expect(frames).toHaveLength(0);
  });
});
