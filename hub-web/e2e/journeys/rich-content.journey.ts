import { readFileSync } from "node:fs";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";

const file = (id: string, name: string, mime: string) => ({ artifact_id: id, name, mime, size_bytes: 1024, sha256: "9f".repeat(32) });
const patch = ["--- a/launch.ts", "+++ b/launch.ts", "@@ -1,25 +1,25 @@", "-launchWithoutEvidence()", "+launchWithEvidence()", ...Array.from({ length: 24 }, (_, i) => ` context ${i}`)].join("\n");
const variants = [
  { name: "desktop light", width: 1280, height: 800, scheme: "light" },
  { name: "desktop dark", width: 1280, height: 800, scheme: "dark" },
  { name: "phone light", width: 390, height: 844, scheme: "light" },
  { name: "phone dark", width: 390, height: 844, scheme: "dark" },
] as const;

for (const [index, variant] of variants.entries()) {
  test(`HUB-J22 read diagrams, media and diffs · ${variant.name}`, index === 0 ? {} : journeyPart, async ({ page, journey }) => {
    await page.setViewportSize({ width: variant.width, height: variant.height });
    await page.emulateMedia({ colorScheme: variant.scheme });
    const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
    const richRequests: string[] = [];
    page.on("request", (request) => { if (/chunk-(rich-|pdf-)/.test(request.url())) richRequests.push(request.url()); });
    const log = page.getByRole("log");

    await journey.stage("Open plain text without loading rich renderers", async () => {
      await journey.open();
      await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
      hub.supervisorSays(PELICAN, "Evidence is ready to inspect.");
      await expect(log.getByText("Evidence is ready to inspect.")).toBeVisible();
      expect(richRequests).toEqual([]);
    });

    await journey.stage("Read a diagram and inspect its source; bad input stays readable", async () => {
      hub.supervisorSays(PELICAN, "# Evidence flows with the work\n\n```mermaid\nflowchart LR\nA[Build] --> B[Inspect]\nB --> C[Ship]\n```\n\n```mermaid\ninvalid diagram source\n```");
      await expect(log.getByRole("img", { name: "Mermaid diagram", exact: true })).toBeVisible();
      const source = log.getByText("Diagram source", { exact: true });
      await source.focus(); await page.keyboard.press("Enter");
      await expect(log.locator(".rich-diagram details")).toHaveAttribute("open", "");
      await page.keyboard.press("Enter");
      await expect(log.locator('[data-rich-state="fallback"]')).toHaveText("invalid diagram source");
      expect(richRequests.some((url) => url.includes("chunk-rich-diagram"))).toBe(true);
    });

    await journey.stage("Expand a unified diff", async () => {
      hub.supervisorSays(PELICAN, `The launch now requires evidence.\n\n\`\`\`diff\n${patch}\n\`\`\``);
      const summary = log.getByText("Show diff · 29 lines", { exact: true });
      await expect(summary).toBeVisible();
      await expect(log.locator(".rich-diff .diff-line")).toHaveCount(0);
      await summary.click();
      await expect(log.locator('[data-change="add"]')).toContainText("+launchWithEvidence()");
      await expect(log.locator('[data-change="remove"]')).toContainText("-launchWithoutEvidence()");
      await summary.click();
    });

    await journey.stage("Decode an inline image without moving its file caption", async () => {
      let releaseImage!: () => void;
      const held = new Promise<void>((resolve) => { releaseImage = resolve; });
      await page.route("https://store.test/view/art-inline-image?*", async (route) => {
        await held;
        await route.fulfill({ contentType: "image/png", body: readFileSync(new URL("../media/evidence.png", import.meta.url)) });
      });
      hub.supervisorSays(PELICAN, "The screenshot is attached.", { attachments: [file("art-inline-image", "Evidence screenshot.png", "image/png")] });
      const image = log.getByRole("img", { name: "Evidence screenshot.png", exact: true });
      try {
        await expect(image).toBeAttached(); await image.scrollIntoViewIfNeeded();
        const caption = log.locator('a[data-artifact-id="art-inline-image"]');
        const before = await caption.boundingBox();
        releaseImage();
        await expect.poll(() => image.evaluate((node: HTMLImageElement) => node.naturalWidth)).toBe(640);
        const after = await caption.boundingBox();
        expect(after!.y).toBeCloseTo(before!.y, 0);
        expect(after!.height).toBeCloseTo(before!.height, 0);
      } finally { releaseImage(); }
    });

    await journey.stage("Read the real PDF first page inline", async () => {
      await page.route("https://store.test/view/art-inline-pdf?*", (route) => route.fulfill({ contentType: "application/pdf", headers: { "access-control-allow-origin": "*" }, body: readFileSync(new URL("../media/evidence.pdf", import.meta.url)) }));
      hub.supervisorSays(PELICAN, "The report is attached.", { attachments: [file("art-inline-pdf", "Evidence report.pdf", "application/pdf")] });
      const preview = log.locator('.rich-attachment:has(a[data-artifact-id="art-inline-pdf"]) .rich-media');
      await preview.scrollIntoViewIfNeeded();
      const before = await preview.boundingBox();
      const pixels = preview.getByRole("img", { name: /PDF preview, page 1 of 1/ });
      await expect(pixels).toBeVisible();
      expect(await pixels.evaluate((node: HTMLImageElement) => {
        const canvas = document.createElement("canvas"); canvas.width = 100; canvas.height = 100;
        const context = canvas.getContext("2d")!; context.drawImage(node, 0, 0);
        return [...context.getImageData(50, 50, 1, 1).data];
      })).not.toEqual([0, 0, 0, 0]);
      const after = await preview.boundingBox();
      expect(after!.height).toBeCloseTo(before!.height, 0);
      await expect(log.locator('a[data-artifact-id="art-inline-pdf"]')).toBeVisible();
    });

    await journey.stage("Play video and audio on demand; unavailable files keep Open", async () => {
      await page.route("https://store.test/view/art-inline-video?*", (route) => route.fulfill({ contentType: "video/webm", body: readFileSync(new URL("../media/evidence.webm", import.meta.url)) }));
      await page.route("https://store.test/view/art-inline-audio?*", (route) => route.fulfill({ contentType: "audio/wav", body: readFileSync(new URL("../media/evidence.wav", import.meta.url)) }));
      hub.supervisorSays(PELICAN, "The recording and audio are attached.", { attachments: [file("art-inline-video", "Evidence video.webm", "video/webm"), file("art-inline-audio", "Evidence audio.wav", "audio/wav"), file("art-local-preview", "Local screenshot.png", "image/png")] });
      for (const tag of ["video", "audio"]) {
        const player = log.locator(tag);
        await player.scrollIntoViewIfNeeded();
        await expect(player).toHaveAttribute("preload", "none");
        expect(await player.evaluate(async (node: HTMLMediaElement) => { await node.play(); node.pause(); return node.duration; })).toBeGreaterThan(0);
      }
      const unavailable = log.locator('.rich-attachment:has(a[data-artifact-id="art-local-preview"])');
      await unavailable.scrollIntoViewIfNeeded();
      await expect(unavailable).toContainText("Preview unavailable. Open the file below.");
      await expect(unavailable.getByRole("link")).toBeVisible();
      // Read back the transport boundary: each inline file requested its own session URL once.
      for (const id of ["art-inline-image", "art-inline-pdf", "art-inline-video", "art-inline-audio", "art-local-preview"]) expect(hub.artifactRequests.filter((value) => value === id)).toHaveLength(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    });
  });
}
