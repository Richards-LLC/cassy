#!/usr/bin/env node

import { createServer } from "node:http";
import { mkdir, readFile, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { extname, join, normalize, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

import { runVisualQa } from "../../scripts/visual-qa.mjs";

export const FIXTURE_NAMES = [
  "fleet-populated",
  "fleet-empty",
  "session-canvas",
  "transcript",
  "attention-0",
  "attention-12",
  "connection-failed-retry",
  "pairing-step-1",
  "pairing-cleanup",
];

export const REQUIRED_SCHEMES = ["light", "dark"];
export const REQUIRED_VIEWPORTS = [
  { name: "desktop", width: 1280, height: 800 },
  { name: "phone", width: 390, height: 844 },
];

const here = fileURLToPath(new URL(".", import.meta.url));
const repoRoot = resolve(here, "../..");
const fixtureRoot = join(repoRoot, "hub-web", "fixtures");
const allowlistPath = join(repoRoot, "hub-web", "visual-qa-allowlist.json");
const defaultArtifactDir = join(repoRoot, "docs", "design", "hub-web", "visual-qa");

function contentType(path) {
  return {
    ".css": "text/css; charset=utf-8",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".map": "application/json; charset=utf-8",
    ".svg": "image/svg+xml",
    ".woff2": "font/woff2",
  }[extname(path)] ?? "application/octet-stream";
}

async function buildFixtureSite(outputDir) {
  await build({
    configFile: false,
    logLevel: "silent",
    root: fixtureRoot,
    base: "/",
    build: {
      outDir: outputDir,
      emptyOutDir: true,
      assetsInlineLimit: 0,
      rollupOptions: {
        input: join(fixtureRoot, "index.html"),
        output: {
          entryFileNames: "fixture.js",
          chunkFileNames: "chunk-[name].js",
          assetFileNames: "assets/[name][extname]",
        },
      },
    },
  });
}

async function serveDirectory(root) {
  const server = createServer(async (request, response) => {
    try {
      const requestUrl = new URL(request.url ?? "/", "http://127.0.0.1");
      const requested = decodeURIComponent(requestUrl.pathname === "/" ? "/index.html" : requestUrl.pathname);
      const target = resolve(root, `.${requested}`);
      const rootRelative = relative(root, target);
      if (rootRelative.startsWith("..") || rootRelative.includes(".." + "/") || !existsSync(target)) {
        response.writeHead(404);
        response.end("Not found");
        return;
      }
      const body = await readFile(target);
      response.writeHead(200, { "content-type": contentType(normalize(target)), "cache-control": "no-store" });
      response.end(body);
    } catch (error) {
      response.writeHead(500);
      response.end(error instanceof Error ? error.message : String(error));
    }
  });
  await new Promise((resolvePromise, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolvePromise);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Fixture server did not expose a TCP port.");
  return { server, origin: `http://127.0.0.1:${address.port}` };
}

function closeServer(server) {
  return new Promise((resolvePromise, reject) => {
    server.close((error) => error ? reject(error) : resolvePromise());
  });
}

export async function runFixtureVisualQa({ artifactDir = defaultArtifactDir, broken = false } = {}) {
  const outputDir = join(repoRoot, ".cas", "visual-qa-fixtures-build");
  await rm(outputDir, { recursive: true, force: true });
  await mkdir(outputDir, { recursive: true });
  await buildFixtureSite(outputDir);
  const { server, origin } = await serveDirectory(outputDir);
  try {
    const urls = FIXTURE_NAMES.map((name) => `${origin}/?fixture=${name}${broken ? "&broken=1" : ""}`);
    try {
      return await runVisualQa({
        urls,
        artifactDir,
        schemes: REQUIRED_SCHEMES,
        viewports: REQUIRED_VIEWPORTS,
        allowlistPath,
        strict: true,
      });
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      if (/browser|chrom(?:e|ium)|executable/i.test(detail)) {
        throw new Error(`${detail}\nInstall Chromium with: npm exec --yes --package=playwright -- playwright install chromium`);
      }
      throw error;
    }
  } finally {
    await closeServer(server);
    await rm(outputDir, { recursive: true, force: true });
  }
}

function parseArgs(argv) {
  const options = { artifactDir: process.env.VISUAL_QA_ARTIFACT_DIR ?? defaultArtifactDir, broken: false, help: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--artifact-dir") options.artifactDir = resolve(repoRoot, argv[++index]);
    else if (arg === "--broken") options.broken = true;
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new Error(`Unknown option ${arg}`);
  }
  return options;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const options = parseArgs(process.argv.slice(2));
    if (options.help) {
      console.log("Usage: npm run visual-qa [--artifact-dir DIR] [--broken]");
      process.exitCode = 0;
    } else {
      const result = await runFixtureVisualQa(options);
      if (result.status === "PASS") console.log(`PASS ${FIXTURE_NAMES.length} fixtures x ${REQUIRED_SCHEMES.length} schemes x ${REQUIRED_VIEWPORTS.length} viewports`);
      else for (const finding of result.findings) console.log(`FAIL ${finding.type} ${finding.elementPath} (${finding.url} · ${finding.scheme} · ${finding.viewport.name})`);
      process.exitCode = result.exitCode;
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 2;
  }
}
