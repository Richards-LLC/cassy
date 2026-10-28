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
  test.describe(variant.name, () => {
    test.use({ viewport: { width: variant.width, height: variant.height }, colorScheme: variant.scheme });
    test(`HUB-J22 read diagrams, media and diffs · ${variant.name}`, index === 0 ? {} : journeyPart, async ({ page, journey, baseURL }, testInfo) => {
      const origin = new URL(baseURL!).origin;
      await page.addInitScript(() => {
        const violations: Array<{ directive: string; source: string; blocked: string }> = [];
        (window as unknown as { richCspViolations: typeof violations }).richCspViolations = violations;
        addEventListener("securitypolicyviolation", (event) => violations.push({ directive: event.effectiveDirective, source: event.sourceFile, blocked: event.blockedURI }));
      });
      if (!process.env.HUB_RICH_HUB_URL) {
        // The static harness gets the same restrictive policy as the candidate.
        // A real hub run always uses its actual response header, untouched.
        await page.route(`${origin}/commander/`, async (route) => {
          const response = await route.fetch();
          await route.fulfill({ response, headers: { ...response.headers(), "content-security-policy": "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; media-src 'self'; font-src 'self'; connect-src 'self' https: wss: http://127.0.0.1:* ws://127.0.0.1:*; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'; worker-src 'self'; manifest-src 'self'" } });
        });
      }
      const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], baseUrls: { atlas: origin } });
      const richRequests: string[] = [];
      page.on("request", (request) => { if (/chunk-(rich-|pdf-)/.test(request.url())) richRequests.push(request.url()); });
      const content = (id: string) => `${origin}/v1/sessions/${PELICAN}/artifacts/${id}/content?*`;
      const log = page.getByRole("log");

      await journey.stage("Open plain text without loading rich renderers", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        await expect(page.locator("#conversation-connection")).toContainText("Live");
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
        // Counts stay local until the ordinary batched flush; read that journal.
        await expect.poll(() => page.evaluate(() => {
          const stored = JSON.parse(localStorage.getItem("commander.usage.v1") ?? "null") as { days?: Record<string, Record<string, number>> } | null;
          return Object.values(stored?.days ?? {}).some((actions) => (actions["rich-content.inspect"] ?? 0) >= 2);
        })).toBe(true);
      });

      await journey.stage("Decode an inline image without moving its file caption", async () => {
        let releaseImage!: () => void;
        const held = new Promise<void>((resolve) => { releaseImage = resolve; });
        let abortTicket!: () => void;
        const failedTicket = new Promise<void>((resolve) => { abortTicket = resolve; });
        let attempts = 0;
        await page.route(`${origin}/v1/sessions/${PELICAN}/artifacts/art-inline-image/ticket`, async (route) => {
          if (++attempts === 1) { await failedTicket; await route.abort("connectionrefused"); }
          else await route.fallback();
        });
        await page.route(content("art-inline-image"), async (route) => {
          await held;
          await route.fulfill({ contentType: "image/png", body: readFileSync(new URL("../media/evidence.png", import.meta.url)) });
        });
        hub.supervisorSays(PELICAN, "The screenshot is attached.", { attachments: [file("art-inline-image", "Evidence screenshot.png", "image/png")] });
        const image = log.getByRole("img", { name: "Evidence screenshot.png", exact: true });
        try {
          await expect.poll(() => attempts).toBe(1);
          hub.hold(PELICAN); hub.drop(PELICAN);
          await expect(page.locator("#conversation-connection")).toContainText("Reconnecting");
          abortTicket();
          const preview = log.locator('.rich-attachment:has(a[data-artifact-id="art-inline-image"]) .rich-media');
          await expect(log.locator('a[data-artifact-id="art-inline-image"] .preview-note')).toHaveText("Preview unavailable");
          await expect(preview).toBeHidden();
          hub.release(PELICAN);
          await expect(page.locator("#conversation-connection")).toContainText("Live", { timeout: 30_000 });
          await expect(image).toBeAttached(); await image.scrollIntoViewIfNeeded();
          expect(attempts).toBe(2);
          const caption = log.locator('a[data-artifact-id="art-inline-image"]');
          const before = await caption.boundingBox();
          releaseImage();
          await expect.poll(() => image.evaluate((node: HTMLImageElement) => node.naturalWidth)).toBe(640);
          const after = await caption.boundingBox();
          expect(after!.y).toBeCloseTo(before!.y, 0);
          expect(after!.height).toBeCloseTo(before!.height, 0);
        } finally { abortTicket(); hub.release(PELICAN); releaseImage(); }
      });

      await journey.stage("Read the real PDF first page inline", async () => {
        await page.route(content("art-inline-pdf"), (route) => route.fulfill({ contentType: "application/pdf", body: readFileSync(new URL("../media/evidence.pdf", import.meta.url)) }));
        hub.supervisorSays(PELICAN, "The report is attached.", { attachments: [file("art-inline-pdf", "Evidence report.pdf", "application/pdf")] });
        const preview = log.locator('.rich-attachment:has(a[data-artifact-id="art-inline-pdf"]) .rich-media');
        await preview.scrollIntoViewIfNeeded();
        const before = await preview.boundingBox();
        const pixels = preview.getByRole("img", { name: /PDF preview, page 1 of 1/ });
        await expect(pixels).toBeVisible();
        expect(await pixels.evaluate(async (node: HTMLImageElement) => {
          await node.decode();
          const canvas = document.createElement("canvas"); canvas.width = 100; canvas.height = 100;
          const context = canvas.getContext("2d")!; context.drawImage(node, 0, 0);
          return [...context.getImageData(50, 50, 1, 1).data];
        })).not.toEqual([0, 0, 0, 0]);
        const after = await preview.boundingBox();
        expect(after!.height).toBeCloseTo(before!.height, 0);
        await expect(log.locator('a[data-artifact-id="art-inline-pdf"]')).toBeVisible();
      });

      await journey.stage("Play video and audio on demand; unavailable files keep Open", async () => {
        const videoRanges: string[] = [];
        await page.route(content("art-inline-video"), (route) => {
          const request = route.request();
          if (new URL(request.url()).searchParams.get("t")?.endsWith("-1")) return route.fulfill({ status: 401, json: { error: "ticket_expired" } });
          const bytes = readFileSync(new URL("../media/evidence.webm", import.meta.url));
          const range = request.headers().range;
          if (range) {
            videoRanges.push(range);
            const match = /^bytes=(\d+)-(\d*)$/.exec(range)!;
            const start = Number(match[1]), end = match[2] ? Math.min(Number(match[2]), bytes.length - 1) : bytes.length - 1;
            return route.fulfill({ status: 206, contentType: "video/webm", headers: { "accept-ranges": "bytes", "content-range": `bytes ${start}-${end}/${bytes.length}` }, body: bytes.subarray(start, end + 1) });
          }
          return route.fulfill({ contentType: "video/webm", body: bytes });
        });
        await page.route(content("art-inline-audio"), (route) => route.fulfill({ contentType: "audio/wav", body: readFileSync(new URL("../media/evidence.wav", import.meta.url)) }));
        hub.supervisorSays(PELICAN, "The recording and audio are attached.", { attachments: [file("art-inline-video", "Evidence video.webm", "video/webm"), file("art-inline-audio", "Evidence audio.wav", "audio/wav"), file("art-local-preview", "Local screenshot.png", "image/png")] });
        for (const tag of ["video", "audio"]) {
          await log.locator(`.rich-attachment:has(a[data-artifact-id="art-inline-${tag}"]) .rich-media`).scrollIntoViewIfNeeded();
          const player = log.locator(tag);
          await expect(player).toBeVisible();
          await expect(player).toHaveAttribute("preload", "none");
          // The video deliberately expires once; the replacement resumes the user's play.
          await player.evaluate((node: HTMLMediaElement) => node.play().catch(() => undefined));
          await expect.poll(() => player.evaluateAll((nodes) => nodes.some((node) => {
            const media = node as HTMLMediaElement;
            return media.readyState > 0 && (!media.paused || media.currentTime > 0 || media.ended);
          }))).toBe(true);
          expect(await player.evaluate((node: HTMLMediaElement) => { node.pause(); return node.duration; })).toBeGreaterThan(0);
        }
        expect(videoRanges.some((range) => range.startsWith("bytes="))).toBe(true);
        const unavailable = log.locator('.rich-attachment:has(a[data-artifact-id="art-local-preview"])');
        await unavailable.scrollIntoViewIfNeeded();
        await expect(unavailable.locator('a.sheet .preview-note')).toHaveText("Preview unavailable");
        await expect(unavailable.locator('.rich-media')).toBeHidden();
        await expect(unavailable.getByRole("link")).toBeVisible();
        // Read back the transport: exactly one renewal, for the expired video only.
        for (const id of ["art-inline-image", "art-inline-pdf", "art-inline-video", "art-inline-audio", "art-local-preview"]) expect(hub.previewTicketRequests.filter((value) => value === id)).toHaveLength(id === "art-inline-video" ? 2 : 1);
        expect(hub.artifactRequests).toEqual([]);
        const csp = await page.evaluate(() => (window as unknown as { richCspViolations: Array<{ directive: string; source: string; blocked: string }> }).richCspViolations);
        await testInfo.attach("csp-events", { body: JSON.stringify(csp, null, 2), contentType: "application/json" });
        // Playwright's anonymous screencast overlay injects a blocked <style>.
        // Keep that automation evidence; no application/attribute violation is exempt.
        expect(csp.filter((event) => !(event.directive === "style-src-elem" && event.source === ""))).toEqual([]);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      });
    });
  });
}
