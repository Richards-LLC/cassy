// Opt-in strict polish proof for the rich-content reading surface.
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { rm, mkdir } from "node:fs/promises";
import { buildFixtureSite, serveDirectory, closeServer } from "./visual-qa.mjs";
import { runVisualQa } from "../../scripts/visual-qa.mjs";

const root = fileURLToPath(new URL("../..", import.meta.url));
const artifacts = process.argv[2];
if (!artifacts) throw new Error("Pass the task artifact directory");
const build = resolve(root, "target/rich-content-visual-fixture");
await mkdir(build, { recursive: true });
await buildFixtureSite(build);
const { server, origin } = await serveDirectory(build);
try {
  const result = await runVisualQa({
    urls: [`${origin}/?fixture=conversation-rich-content`],
    artifactDir: resolve(artifacts, "qa/visual-qa"),
    schemes: ["light", "dark"],
    viewports: [{ name: "desktop", width: 1280, height: 800 }, { name: "phone", width: 390, height: 844 }],
    allowlistPath: resolve(root, "hub-web/visual-qa-allowlist.json"),
    readySelector: ".rich-diagram svg",
    strict: true,
  });
  console.log(JSON.stringify({ status: result.status, findings: result.findings, screenshots: result.screenshots.length }));
  process.exitCode = result.exitCode;
} finally { await closeServer(server); await rm(build, { recursive: true, force: true }); }
