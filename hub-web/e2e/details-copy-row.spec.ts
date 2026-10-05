import { test, expect } from "@playwright/test";
import { expectDetailsCopyRow } from "./details-copy-row";

test("Details row measurement uses the current control after a redraw", async ({ page }) => {
  await page.setContent('<article><details open><summary>Details</summary><button class="attention-copy">Copy</button><pre>The full notice</pre></details></article>');
  const article = page.locator("article");
  const staleCopy = await article.locator("button").elementHandle();
  await article.evaluate((node) => node.replaceWith(node.cloneNode(true)));
  expect(await staleCopy!.boundingBox()).toBeNull();
  await expectDetailsCopyRow(article, "the current Copy sits above the notice");
});

test("Details row measurement rejects a hidden or overlapping Copy", async ({ page }) => {
  await page.setContent('<article><details><summary>Details</summary><button class="attention-copy">Copy</button><pre>The full notice</pre></details></article>');
  const article = page.locator("article");
  await expect(expectDetailsCopyRow(article, "hidden Copy is a defect")).rejects.toThrow("hidden Copy is a defect");
  await article.locator("details").evaluate((details) => { (details as HTMLDetailsElement).open = true; });
  await article.locator("button").evaluate((button) => { button.style.position = "absolute"; button.style.top = "100px"; });
  await expect(expectDetailsCopyRow(article, "Copy below the notice is a defect")).rejects.toThrow("Copy below the notice is a defect");
});
