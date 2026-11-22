import { readFileSync } from "node:fs";
import type { Locator, Page } from "@playwright/test";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { journeyNow } from "./clock";

const file = (id: string, name: string, mime: string) => ({ artifact_id: id, name, mime, size_bytes: 1024, sha256: "9f".repeat(32) });
const patch = ["--- a/launch.ts", "+++ b/launch.ts", "@@ -1,25 +1,25 @@", "-launchWithoutEvidence()", "+launchWithEvidence()", ...Array.from({ length: 24 }, (_, i) => ` context ${i}`)].join("\n");
const variants = [
  { name: "desktop light", width: 1280, height: 800, scheme: "light" },
  { name: "desktop dark", width: 1280, height: 800, scheme: "dark" },
  { name: "phone light", width: 390, height: 844, scheme: "light" },
  { name: "phone dark", width: 390, height: 844, scheme: "dark" },
] as const;

// A row is named "<project> on <machine>, …"; the row's own controls are not.
const conversationRow = (page: Page) => page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /^cas-src on / });
// The header's connection chip; attribute checks read it even while it carries no words.
const connection = (page: Page) => page.getByRole("status", { name: "Connection", exact: true, includeHidden: true });
const readingThread = (page: Page) => page.getByTestId("conversation-thread");
const literal = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
// An attachment's card is a link named "<file name>, <type>, <size>, from …. Open".
const attachmentLink = (scope: Page | Locator, name: string) => scope.getByRole("link", { name: new RegExp(`^${literal(name)}[,.]`) });
const attachmentCard = (page: Page, name: string) => page.getByRole("log").getByTestId("rich-attachment").filter({ has: attachmentLink(page, name) });
const previewNote = (link: Locator) => link.getByRole("status", { name: "Preview status", exact: true });
const codeFrames = (page: Page) => page.getByRole("log").getByTestId("code-frame");
const codeFrame = (page: Page, label: string) => codeFrames(page).filter({ has: page.getByRole("button", { name: `Copy source from ${label}`, exact: true }) });
// A rendered diagram is the frame's figure; it stays mounted (hidden) while the source shows.
const diagram = (scope: Page | Locator) => scope.getByRole("figure", { includeHidden: true });

test("HUB-J22 a cached failed thumbnail recovers after an offscreen reconnect", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const log = page.getByRole("log");
  const cachedLink = attachmentLink(log, "Cached image.png");
  const preview = cachedLink.getByTestId("rich-media");
  let attempts = 0;
  let recover = false;
  let beforeReconnect = 0;
  await page.route(`**/v1/sessions/${PELICAN}/artifacts/art-cached-image/ticket`, async route => {
    attempts++;
    if (!recover) await route.fulfill({ status: 503, json: { error: "temporarily_unavailable" } });
    else await route.fallback();
  });
  await page.route(`**/v1/sessions/${PELICAN}/artifacts/art-cached-image/content?*`, route => route.fulfill({
    contentType: "image/png", body: readFileSync(new URL("../media/evidence.png", import.meta.url)),
  }));
  await journey.stage("A failed image collapses into its own card", async () => {
    await journey.open();
    await conversationRow(page).click();
    await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
    await hub.socketOpen(PELICAN);
    hub.supervisorSays(PELICAN, "Keep this image while reading later updates.", { attachments: [file("art-cached-image", "Cached image.png", "image/png")] });
    await expect(preview).toHaveAttribute("data-preview-state", "waiting");
    await expect(preview).toBeHidden();
    await expect(previewNote(cachedLink)).toBeVisible();
  });
  const cached = (await preview.elementHandle())!;
  await journey.stage("Scroll the failed card out of the DOM, then reconnect the machine", async () => {
    // More than twenty groups makes the real thread window cache the first
    // card; fewer than sixty keeps that same node available for restoration.
    for (let i = 0; i < 84; i++) hub.supervisorSays(PELICAN, `Later update ${i + 1}: the operator is reading the latest work.`);
    const thread = readingThread(page);
    await thread.evaluate(node => { node.scrollTop = node.scrollHeight; node.dispatchEvent(new Event("scroll")); });
    await expect(preview).toHaveCount(0);
    expect(await cached.evaluate(node => node.isConnected)).toBe(false);
    beforeReconnect = attempts;
    expect(beforeReconnect).toBeGreaterThan(0);
    hub.hold(PELICAN); hub.drop(PELICAN);
    await expect(connection(page)).toHaveAttribute("data-phase", /^(?:backoff|resolving|dialing|auth|attaching|failed)$/);
    hub.release(PELICAN);
    await expect(connection(page)).toContainText("Live", { timeout: 30_000 });
    expect(attempts).toBe(beforeReconnect);
  });
  await journey.stage("Scroll back to the same cached card and decode its recovered image", async () => {
    // Keep the failure real until the machine has reconnected offscreen.
    // Initial catalog paints may reattach the card before it leaves the DOM.
    recover = true;
    await readingThread(page).evaluate(node => { node.scrollTop = 0; node.dispatchEvent(new Event("scroll")); });
    await expect(preview).toHaveCount(1);
    expect(await cached.evaluate(node => node.isConnected)).toBe(true);
    const image = preview.getByRole("img", { name: "Cached image.png", exact: true });
    await expect(image).toBeVisible();
    await expect.poll(() => image.evaluate((node: HTMLImageElement) => node.naturalWidth)).toBe(640);
    await expect(previewNote(cachedLink)).toHaveCount(0);
    expect(attempts).toBe(beforeReconnect + 1);
  });
});

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
        await conversationRow(page).click();
        await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
        await hub.socketOpen(PELICAN);
        hub.supervisorSays(PELICAN, "Evidence is ready to inspect.");
        await expect(log.getByText("Evidence is ready to inspect.")).toBeVisible();
        expect(richRequests).toEqual([]);
      });

      await journey.stage("Copy exact source and wrap a code frame with the keyboard", async () => {
        const source = `// Literal source stays selectable\nconst message = "<script>&amp;雪 ${"long_source_".repeat(18)}";  \n\tconsole.log(message);\n| literal | source |\n| --- | --- |`;
        let release!: () => void;
        const gate = new Promise<void>(resolve => { release = resolve; });
        await page.route("**/chunk-rich-syntax-*.js", async route => { await gate; await route.continue(); });
        hub.supervisorSays(PELICAN, `# Source ready to copy\n\n\`\`\`ts src/example.ts\n${source}\n\`\`\`\n\n~~~unknown\nUnknown languages stay readable.\n~~~`);
        const frame = codeFrame(page, "src/example.ts");
        await expect(frame.getByTestId("code-frame-label")).toHaveText("src/example.ts");
        await expect(frame.getByRole("code")).toHaveText(source, { useInnerText: false });
        await expect(frame.getByTestId("markdown-code")).toHaveAttribute("data-syntax-state", "loading");
        const selected = await frame.getByRole("code").evaluate(code => {
          const selection = getSelection()!;
          selection.setBaseAndExtent(code.firstChild!, 52, code.firstChild!, 8);
          return selection.toString();
        });
        release();
        await expect(frame.getByTestId("markdown-code")).toHaveAttribute("data-syntax-state", "ready");
        // The first keyword token in source order is the declaration.
        await expect(frame.getByTestId("syntax-keyword").first()).toHaveText("const");
        await expect(frame.getByTestId("syntax-comment")).toHaveText("// Literal source stays selectable");
        await expect(frame.getByTestId("syntax-string")).toContainText("<script>");
        expect(await page.evaluate(() => getSelection()!.toString())).toBe(selected);
        hub.supervisorSays(PELICAN, "Source review is still open.");
        await expect(log.getByText("Source review is still open.", { exact: true })).toBeVisible();
        expect(await page.evaluate(() => getSelection()!.toString())).toBe(selected);
        await page.evaluate(() => getSelection()!.removeAllRanges());
        const wrap = frame.getByRole("button", { name: "Wrap lines in src/example.ts" });
        await wrap.focus(); await page.keyboard.press("Enter");
        await expect(wrap).toHaveAttribute("aria-pressed", "true");
        await expect(log.getByRole("button", { name: "Wrap lines in unknown" })).toHaveAttribute("aria-pressed", "false");
        expect(await frame.getByTestId("markdown-code").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        const copy = frame.getByRole("button", { name: "Copy source from src/example.ts" });
        await copy.focus(); await page.keyboard.press("Enter");
        await expect(frame.getByRole("status")).toBeVisible();
        if (index === 0) expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(source);
        await expect(codeFrame(page, "unknown").getByTestId("code-frame-label")).toHaveText("unknown");
        // A refused browser clipboard produces a visible recovery action.
        await page.evaluate(() => { Object.defineProperty(navigator.clipboard, "writeText", { configurable: true, value: () => Promise.reject(new Error("Permission denied")) }); });
        await copy.click();

        await expect(copy).toBeEnabled();
        await page.evaluate(() => { Reflect.deleteProperty(navigator.clipboard, "writeText"); });
        const statusSource = "const count = 2;\nconsole.log(count);";
        hub.supervisorSays(PELICAN, `\`\`\`ts smoke.ts\n${statusSource}\n\`\`\``, { kind: "status" });
        const statusFrame = codeFrame(page, "smoke.ts");
        await expect(statusFrame.getByTestId("code-frame-label")).toHaveText("smoke.ts");
        await statusFrame.getByRole("button", { name: "Copy source from smoke.ts" }).click();
        await expect(statusFrame.getByRole("status")).toBeVisible();
        expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(statusSource);
      });

      await journey.stage("Read a diagram and inspect its source; bad input stays readable", async () => {
        hub.supervisorSays(PELICAN, "# Evidence flows with the work\n\n```mermaid\nflowchart LR\nA[Build] --> B[Inspect]\nB --> C[Ship]\n```\n\n```mermaid\ninvalid diagram source\n```");
        await expect(log.getByRole("img", { name: "Mermaid diagram", exact: true })).toBeVisible();
        const frame = codeFrames(page).filter({ has: diagram(page) });
        const source = frame.getByRole("button", { name: "Show source from mermaid" });
        await source.focus(); await page.keyboard.press("Enter");
        await expect(frame.getByTestId("markdown-code")).toBeVisible();
        await expect(diagram(frame)).toBeHidden();
        // Only the source can overflow or wrap, so only the source view offers Expand and Wrap.
        const wrap = frame.getByRole("button", { name: "Wrap lines in mermaid" });
        await expect(wrap).toBeVisible();
        const expand = frame.getByRole("button", { name: /^(?:Expand|Collapse) mermaid$/ });
        await expand.focus(); await page.keyboard.press("Enter");
        await expect(expand).toHaveAttribute("aria-expanded", "true");
        await page.keyboard.press("Enter");
        await expect(expand).toHaveAttribute("aria-expanded", "false");
        await frame.getByRole("button", { name: "Show preview from mermaid" }).focus(); await page.keyboard.press("Enter");
        await expect(diagram(frame)).toBeVisible();
        await expect(expand).toBeHidden();
        await expect(wrap).toBeHidden();
        await expect(log.getByLabel("Diagram source — preview unavailable", { exact: true })).toHaveText("invalid diagram source");
        expect(richRequests.some((url) => url.includes("chunk-rich-diagram"))).toBe(true);
        // The reply guidance's own flowchart shows whole, with no action to reach its end.
        hub.supervisorSays(PELICAN, "```mermaid\nflowchart TD\n  A[Worker parks the task] --> B[Supervisor merges to the epic]\n  B --> C[Merge queue runs the suite]\n  C -->|green| D[main]\n  C -->|red| B\n```");
        const flow = codeFrames(page).filter({ hasText: "mermaid" }).filter({ has: page.getByText("Merge queue runs the suite") });
        const flowchart = flow.getByRole("img", { name: "Mermaid diagram", exact: true });
        await expect(flowchart).toBeVisible();
        // The SVG can be visible and still have no box for a moment on a fast runner.
        await expect.poll(async () => (await flowchart.boundingBox())?.height ?? 0, { timeout: 5_000 }).toBeGreaterThan(288);
        expect(await flow.getByTestId("code-frame-content").evaluate((node) => node.scrollHeight - node.clientHeight)).toBe(0);
      });

      await journey.stage("Expand a unified diff", async () => {
        hub.supervisorSays(PELICAN, `The launch now requires evidence.\n\n\`\`\`diff\n${patch}\n\`\`\``);
        const summary = log.getByTestId("rich-diff").getByText(/^Show diff · \d+ lines$/);
        await expect(summary).toBeVisible();
        const diffLines = log.getByTestId("rich-diff").getByTestId("diff-line");
        await expect(diffLines).toHaveCount(0);
        await summary.click();
        await expect(diffLines.filter({ hasText: "+launchWithEvidence()" })).toHaveAttribute("data-change", "add");
        await expect(diffLines.filter({ hasText: "-launchWithoutEvidence()" })).toHaveAttribute("data-change", "remove");
        await summary.click();
        // Counts stay local until the ordinary batched flush; read that journal.
        await expect.poll(() => page.evaluate(() => {
          const stored = JSON.parse(localStorage.getItem("commander.usage.v1") ?? "null") as { days?: Record<string, Record<string, number>> } | null;
          return Object.values(stored?.days ?? {}).some((actions) => (actions["rich-content.inspect"] ?? 0) >= 2);
        })).toBe(true);
        // The same stage, written by hand with no @@ line, tints its changes too.
        hub.supervisorSays(PELICAN, "Delivery got faster.\n\n```diff\n- delivered after 11.5 minutes\n+ delivered in about 1 second\n```");
        const handwritten = log.getByTestId("rich-diff").filter({ hasText: "delivered in about 1 second" });
        const tint = (change: string) => handwritten.locator(`[data-change="${change}"]`).evaluate((node) => getComputedStyle(node).backgroundColor);
        const [added, removed] = [await tint("add"), await tint("remove")];
        expect(added).not.toBe(removed);
        expect([added, removed]).not.toContain("rgba(0, 0, 0, 0)");
        await handwritten.screenshot({ path: testInfo.outputPath("handwritten-diff.png") });
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
          // Preview tickets are visibility-driven; inspect the attachment before
          // waiting for its request, even after a tall wrapped code block.
          await attachmentCard(page, "Evidence screenshot.png").scrollIntoViewIfNeeded();
          await expect.poll(() => attempts).toBe(1);
          hub.hold(PELICAN); hub.drop(PELICAN);
          await expect(connection(page)).toHaveAttribute("data-phase", /^(?:backoff|resolving|dialing|auth|attaching|failed)$/);
          abortTicket();
          const preview = attachmentCard(page, "Evidence screenshot.png").getByTestId("rich-media");
          await expect(previewNote(attachmentLink(log, "Evidence screenshot.png"))).toBeVisible();
          await expect(preview).toBeHidden();
          hub.release(PELICAN);
          await expect(connection(page)).toContainText("Live", { timeout: 30_000 });
          await expect(image).toBeAttached(); await image.scrollIntoViewIfNeeded();
          expect(attempts).toBe(2);
          const caption = attachmentLink(log, "Evidence screenshot.png");
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
        const preview = attachmentCard(page, "Evidence report.pdf").getByTestId("rich-media");
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
        await expect(attachmentLink(log, "Evidence report.pdf")).toBeVisible();
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
        for (const name of ["Evidence video.webm", "Evidence audio.wav"]) {
          const attachment = attachmentCard(page, name);
          const preview = attachment.getByTestId("rich-media");
          // Exercise an unfocused reader's scroll while the preview is lazy.
          const thread = readingThread(page);
          const box = (await thread.boundingBox())!;
          const delta = await preview.evaluate(node => {
            const thread = node.closest<HTMLElement>(".thread")!;
            return node.getBoundingClientRect().top - thread.getBoundingClientRect().top - thread.clientHeight / 4;
          });
          await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
          await page.mouse.wheel(0, delta);
          await expect(preview).toHaveAttribute("data-preview-state", "ready");
          // The player is named for its file, as the card's preview box names "<file> preview".
          const player = log.getByLabel(name, { exact: true });
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
        const unavailable = attachmentCard(page, "Local screenshot.png");
        // Claim this card as the reader's destination before scrolling: a pending
        // thread resize can otherwise restore the previous reading anchor.
        await unavailable.getByRole("link").focus();
        await unavailable.scrollIntoViewIfNeeded();
        await expect(previewNote(unavailable.getByRole("link"))).toBeVisible();
        await expect(unavailable.getByTestId("rich-media")).toBeHidden();
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


test("HUB-J22 unavailable syntax color retains source and reading controls", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  await page.route("**/chunk-rich-syntax-*.js", route => route.abort("failed"));
  await journey.stage("Open source when its color chunk is unavailable", async () => {
    await journey.open();
    await conversationRow(page).click();
    await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
    const source = 'const unsafe = "<script>still text</script>"; // no colors';
    await hub.socketOpen(PELICAN);
    hub.supervisorSays(PELICAN, `\`\`\`ts fallback.ts\n${source}\n\`\`\``);
    const frame = codeFrames(page);
    await expect(frame.getByTestId("markdown-code")).toHaveAttribute("data-syntax-state", "plain");
    await expect(frame.getByRole("code")).toHaveText(source, { useInnerText: false });
    await expect(frame.getByTestId("code-syntax-status")).toBeVisible();
    await expect(frame.locator("script")).toHaveCount(0);
    const wrap = frame.getByRole("button", { name: "Wrap lines in fallback.ts" });
    await wrap.focus(); await page.keyboard.press("Enter");
    await expect(wrap).toHaveAttribute("aria-pressed", "true");
    await frame.getByRole("button", { name: "Copy source from fallback.ts" }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(source);
    await expect(frame.getByRole("status")).toBeVisible();
  });
});

for (const variant of variants) {
  test.describe(`inline reply images ${variant.name}`, () => {
    test.use({ viewport: { width: variant.width, height: variant.height }, colorScheme: variant.scheme });
    test(`HUB-J22 inline reply images · ${variant.name}`, journeyPart, async ({ page, journey }, testInfo) => {
      const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
      const png = readFileSync(new URL("../media/evidence.png", import.meta.url));
      const shown = "cas-85bfb/after/desktop.png";
      const local = `local-${Buffer.from(shown).toString("base64url")}`;
      const tickets: string[] = [];
      // The machine re-proves each reference; here it admits the artifacts-directory screenshot only.
      await page.route(`**/v1/sessions/${PELICAN}/artifacts/*/ticket`, route => {
        const id = decodeURIComponent(new URL(route.request().url()).pathname.split("/")[5]!);
        tickets.push(id);
        return id === local ? route.fulfill({ json: { ticket: "fixture-local", expires_at: new Date(journeyNow() + 60_000).toISOString() } }) : route.fulfill({ status: 404, json: { error: "not_found" } });
      });
      await page.route(`**/v1/sessions/${PELICAN}/artifacts/${local}/content?*`, route => route.fulfill({ contentType: "image/png", body: png, headers: { "cache-control": "private, no-store" } }));
      const log = page.getByRole("log");
      await journey.stage("A supervisor reply shows its screenshot inline and keeps refused images as text", async () => {
        await journey.open();
        await conversationRow(page).click();
        await expect(connection(page)).toHaveAttribute("data-phase", "live"); await expect(connection(page)).toHaveAttribute("data-degraded", "false");
        await hub.socketOpen(PELICAN);
        hub.supervisorSays(PELICAN, `Epic progress, ready for review.\n\n![Epic progress at desktop width](${shown})\n\n![Outside the artifacts directory](/etc/shadow.png)\n\n![Remote](https://example.com/x.png)`);
        const image = log.getByRole("img", { name: "Epic progress at desktop width" });
        await expect(image).toBeVisible();
        // cas-a16f: visible passes on the placeholder; the opener turns ready on the image's load event.
        await expect(log.getByRole("button", { name: "Epic progress at desktop width. Enlarge image" })).toHaveAttribute("data-state", "ready");
        expect(await image.evaluate(node => (node as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
        await expect(log).toContainText("![Outside the artifacts directory](/etc/shadow.png)");
        await expect(log).toContainText("![Remote](https://example.com/x.png)");
        expect(tickets).not.toContain("https://example.com/x.png");
        const box = (await image.boundingBox())!;
        expect(box.width).toBeLessThanOrEqual(variant.width);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        await page.screenshot({ path: testInfo.outputPath(`inline-reply-image-${variant.width}-${variant.scheme}.png`) });
        if (process.env.REPLY_IMAGE_QA) await page.screenshot({ path: `${process.env.REPLY_IMAGE_QA}/inline-reply-image-${variant.width}-${variant.scheme}.png` });
      });
      await journey.stage("Tap the image to enlarge it, then close back to the reply", async () => {
        const opener = log.getByRole("button", { name: "Epic progress at desktop width. Enlarge image" });
        await opener.click();
        const viewer = page.getByRole("dialog", { name: "Image viewer" });
        await expect(viewer).toBeVisible();
        if (process.env.REPLY_IMAGE_QA) await page.screenshot({ path: `${process.env.REPLY_IMAGE_QA}/inline-reply-image-${variant.width}-${variant.scheme}-enlarged.png` });
        await page.keyboard.press("Escape");
        await expect(viewer).toBeHidden();
        await expect(opener).toBeFocused();
      });
    });
  });
}
