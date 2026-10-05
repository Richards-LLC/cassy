import { expect, type Locator } from "@playwright/test";

/** Measure the current Details controls together, across catalog/resize redraws. */
export async function expectDetailsCopyRow(notice: Locator, message: string): Promise<void> {
  await expect.poll(() => notice.evaluate((article) => {
    const payload = article.querySelector("pre");
    const copy = article.querySelector(".attention-copy");
    if (!article.isConnected || !payload || !copy || !payload.closest("details")?.open
      || getComputedStyle(payload).visibility !== "visible" || getComputedStyle(copy).visibility !== "visible") return false;
    // One browser turn: a redraw cannot detach Copy between two protocol calls.
    const payloadBox = payload.getBoundingClientRect();
    const copyBox = copy.getBoundingClientRect();
    return payloadBox.width > 0 && payloadBox.height > 0
      && copyBox.width > 0 && copyBox.height > 0
      && payloadBox.y >= copyBox.bottom;
  }), { message }).toBe(true);
}
