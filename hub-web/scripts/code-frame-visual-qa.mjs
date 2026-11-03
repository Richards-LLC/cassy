// Strict, scoped polish evidence for the reusable code-frame reading tools.
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { rm, mkdir, mkdtemp } from "node:fs/promises";
import { chromium } from "@playwright/test";
import { buildFixtureSite, serveDirectory, closeServer } from "./visual-qa.mjs";
import { runVisualQa } from "../../scripts/visual-qa.mjs";

const root = fileURLToPath(new URL("../..", import.meta.url));
const artifacts = process.argv[2];
if (!artifacts) throw new Error("Pass the task artifact directory");
await mkdir(resolve(root, "target"), { recursive: true });
const build = await mkdtemp(resolve(root, "target/code-frame-visual-"));
await buildFixtureSite(build);
const { server, origin } = await serveDirectory(build);
try {
  const url = `${origin}/?fixture=conversation-code-frames`;
  const result = await runVisualQa({
    urls: [url],
    artifactDir: resolve(artifacts, "qa/visual-qa"),
    schemes: ["light", "dark"],
    viewports: [{ name: "desktop", width: 1280, height: 800 }, { name: "phone", width: 390, height: 844 }],
    allowlistPath: resolve(root, "hub-web/visual-qa-allowlist.json"),
    readySelector: ".code-frame",
    strict: true,
  });
  console.log(JSON.stringify({ status: result.status, findings: result.findings, screenshots: result.screenshots.length }));
  process.exitCode = result.exitCode;
  const browser = await chromium.launch();
  try {
    for (const [name, media] of Object.entries({ "forced-colors": { forcedColors: "active" }, "reduced-motion": { reducedMotion: "reduce" }, "contrast-more": { contrast: "more" } })) {
      const page = await browser.newPage({ viewport: { width: 390, height: 844 }, colorScheme: "light", ...media });
      await page.goto(url);
      await page.locator('.code-frame [data-action="wrap"]').first().click();
      await page.screenshot({ path: resolve(artifacts, `qa/a11y-${name}.png`) });
      await page.close();
    }
  } finally { await browser.close(); }
} finally { await closeServer(server); await rm(build, { recursive: true, force: true }); }
