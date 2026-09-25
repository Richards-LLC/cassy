#!/usr/bin/env node
// check-feature-map — static check for a project's docs/qa/features/ map.
//
// Usage:
//   node scripts/check-feature-map.mjs [--root <project root>] [--features <dir>]
//
// --root defaults to the current directory; --features defaults to
// docs/qa/features under the root. When the default directory does not exist
// the project has not opted in: the check prints a skipped summary and exits 0.
// An explicit --features that does not exist fails.
//
// Checks (failures unless noted):
//   sections      every feature file (*.md except README.md) has the five H2s:
//                 Sub-features, How to get to it, Driving it, Gotchas, Touches
//   touches       the Touches section lists at least one glob, and every glob
//                 (backticked text of a bullet, relative to the root) matches
//                 at least one file
//   index         README.md exists and names every feature file; every .md
//                 link in README.md points at an existing feature file
//   anchor (warn) each backticked route or selector under "Driving it" still
//                 appears in a Touches file's content or path. Tokens with
//                 spaces (commands) are skipped.
//
// Output: one JSON summary on stdout.
// Exit codes: 0 no failures, 1 failures, 2 usage error.
// Node built-ins only; no dependencies.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const SECTIONS = ["Sub-features", "How to get to it", "Driving it", "Gotchas", "Touches"];
const SKIP_DIRS = new Set([".git", "node_modules", "target", ".next", ".nuxt"]);
const MAX_GREP_BYTES = 2 * 1024 * 1024;

function parseArgs(argv) {
  const opts = { root: process.cwd(), features: null };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if ((arg === "--root" || arg === "--features") && i + 1 < argv.length) {
      opts[arg.slice(2)] = argv[++i];
    } else if (arg === "-h" || arg === "--help") {
      process.stdout.write("usage: check-feature-map.mjs [--root <dir>] [--features <dir>]\n");
      process.exit(0);
    } else {
      process.stderr.write(`check-feature-map: unknown argument ${arg}\n`);
      process.exit(2);
    }
  }
  return opts;
}

// Split a markdown file into H2 sections, ignoring headings inside fences.
function sections(text) {
  const out = new Map();
  let current = null;
  let fence = null;
  for (const line of text.split(/\r?\n/)) {
    const f = line.match(/^\s*(```+|~~~+)/);
    if (f) {
      if (!fence) fence = f[1][0];
      else if (f[1][0] === fence) fence = null;
    }
    const h = !fence && !f && line.match(/^##\s+(.+?)\s*#*\s*$/);
    if (h) {
      current = h[1].trim();
      out.set(current, []);
    } else if (current) {
      out.get(current).push(line);
    }
  }
  return out;
}

function findSection(map, name) {
  const want = name.toLowerCase();
  for (const [heading, lines] of map) {
    const got = heading.toLowerCase();
    if (got === want || got.startsWith(`${want} `) || got.startsWith(`${want}(`)) return lines;
  }
  return null;
}

function touchGlobs(lines) {
  const globs = [];
  for (const line of lines) {
    const bullet = line.match(/^\s*[-*+]\s+(.*)$/);
    if (!bullet) continue;
    const tick = bullet[1].match(/`([^`]+)`/);
    const glob = (tick ? tick[1] : bullet[1].split(/\s+[—–-]\s+/)[0]).trim();
    if (glob) globs.push(glob.replace(/^\.\//, ""));
  }
  return globs;
}

function globToRegExp(glob) {
  let g = glob.endsWith("/") ? `${glob}**` : glob;
  let re = "";
  for (let i = 0; i < g.length; i++) {
    const c = g[i];
    if (c === "*") {
      if (g[i + 1] === "*") {
        const slash = g[i + 2] === "/";
        re += slash ? "(?:.*/)?" : ".*";
        i += slash ? 2 : 1;
      } else {
        re += "[^/]*";
      }
    } else if (c === "?") {
      re += "[^/]";
    } else if (c === "{") {
      const end = g.indexOf("}", i);
      if (end === -1) { re += "\\{"; continue; }
      const alts = g.slice(i + 1, end).split(",").map((a) => a.replace(/[.+^$()|[\]\\]/g, "\\$&").replace(/\*/g, "[^/]*"));
      re += `(?:${alts.join("|")})`;
      i = end;
    } else if (c === "[") {
      const end = g.indexOf("]", i);
      if (end === -1) { re += "\\["; continue; }
      re += g.slice(i, end + 1).replace(/^\[!/, "[^");
      i = end;
    } else {
      re += c.replace(/[.+^$()|\\}\]]/g, "\\$&");
    }
  }
  // A glob without wildcards may name a directory: match everything under it.
  return new RegExp(`^${re}(?:/.*)?$`);
}

function listFiles(root) {
  const git = spawnSync("git", ["-C", root, "ls-files", "--cached", "--others", "--exclude-standard", "-z"], {
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });
  if (git.status === 0) {
    return git.stdout.split("\0").filter(Boolean).filter((p) => fs.existsSync(path.join(root, p)));
  }
  const files = [];
  const walk = (dir, rel) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      if (SKIP_DIRS.has(entry.name)) continue;
      const r = rel ? `${rel}/${entry.name}` : entry.name;
      if (entry.isDirectory()) walk(path.join(dir, entry.name), r);
      else if (entry.isFile()) files.push(r);
    }
  };
  walk(root, "");
  return files;
}

// A backticked token under "Driving it" worth grepping: a route or a selector.
function anchorOf(token) {
  if (/\s/.test(token)) return null;
  const url = token.match(/^https?:\/\/[^/]+(\/.*)?$/);
  if (url) return url[1] && url[1] !== "/" ? url[1] : null;
  if (/^\/[\w\-./:[\]*]+$/.test(token) && token.length > 1) return token;
  const attr = token.match(/^\[[\w-]+[~|^$*]?=["']?([^"'\]]+)["']?\]$/);
  if (attr) return attr[1];
  const idClass = token.match(/^[#.]([A-Za-z_][\w-]*)$/);
  if (idClass) return idClass[1];
  return null;
}

function routeAnchor(token) {
  // Keep the static prefix of a route: `/users/:id/edit` → `/users`.
  const cut = token.search(/\/(?::|\[|\*)/);
  return cut > 0 ? token.slice(0, cut) : token;
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const root = path.resolve(opts.root);
  const explicit = opts.features !== null;
  const featuresDir = path.resolve(root, opts.features ?? "docs/qa/features");
  const summary = {
    ok: true,
    root,
    features_dir: path.relative(root, featuresDir) || ".",
    feature_files: 0,
    failures: [],
    warnings: [],
  };
  const fail = (file, check, detail) => summary.failures.push({ file, check, detail });
  const warn = (file, check, detail) => summary.warnings.push({ file, check, detail });

  if (!fs.existsSync(featuresDir) || !fs.statSync(featuresDir).isDirectory()) {
    if (explicit) {
      fail(summary.features_dir, "features-dir", "directory does not exist");
      summary.ok = false;
      process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
      process.exit(1);
    }
    summary.skipped = "no docs/qa/features directory; project has not opted in";
    process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
    process.exit(0);
  }

  const featureFiles = fs
    .readdirSync(featuresDir)
    .filter((f) => f.endsWith(".md") && f.toLowerCase() !== "readme.md")
    .sort();
  summary.feature_files = featureFiles.length;

  // Index.
  const readmeName = fs.readdirSync(featuresDir).find((f) => f.toLowerCase() === "readme.md");
  if (!readmeName) {
    fail(`${summary.features_dir}/README.md`, "index", "missing index README.md");
  } else {
    const readme = fs.readFileSync(path.join(featuresDir, readmeName), "utf8");
    for (const f of featureFiles) {
      if (!readme.includes(f)) fail(`${summary.features_dir}/${readmeName}`, "index", `does not list ${f}`);
    }
    for (const m of readme.matchAll(/\]\(([^)#\s]+\.md)(?:#[^)]*)?\)/g)) {
      const target = m[1].replace(/^\.\//, "");
      if (/^[a-z]+:/i.test(target) || target.includes("/")) continue;
      if (!fs.existsSync(path.join(featuresDir, target))) fail(`${summary.features_dir}/${readmeName}`, "index", `links missing file ${target}`);
    }
  }

  let files = null;
  for (const f of featureFiles) {
    const rel = `${summary.features_dir}/${f}`;
    const text = fs.readFileSync(path.join(featuresDir, f), "utf8");
    const map = sections(text);
    const missing = SECTIONS.filter((s) => findSection(map, s) === null);
    if (missing.length) fail(rel, "sections", `missing H2: ${missing.join(", ")}`);

    const touchesLines = findSection(map, "Touches");
    if (!touchesLines) continue;
    const globs = touchGlobs(touchesLines);
    if (!globs.length) {
      fail(rel, "touches", "Touches lists no globs");
      continue;
    }
    files ??= listFiles(root);
    const touched = new Set();
    for (const glob of globs) {
      const re = globToRegExp(glob);
      const hits = files.filter((p) => re.test(p));
      if (!hits.length) fail(rel, "touches", `glob matches no file: ${glob}`);
      hits.forEach((h) => touched.add(h));
    }

    const driving = findSection(map, "Driving it");
    if (!driving || !touched.size) continue;
    const contents = [];
    for (const p of touched) {
      try {
        const abs = path.join(root, p);
        if (fs.statSync(abs).size <= MAX_GREP_BYTES) contents.push(fs.readFileSync(abs, "utf8"));
      } catch {
        // unreadable file: skip
      }
    }
    const paths = [...touched].join("\n");
    for (const m of driving.join("\n").matchAll(/`([^`\n]+)`/g)) {
      let anchor = anchorOf(m[1].trim());
      if (!anchor) continue;
      if (anchor.startsWith("/")) anchor = routeAnchor(anchor);
      const pathForm = anchor.replace(/^\//, "");
      const found =
        contents.some((c) => c.includes(anchor)) || (pathForm.length > 0 && paths.includes(pathForm));
      if (!found) warn(rel, "anchor", `\`${m[1].trim()}\` not found in Touches files`);
    }
  }

  summary.ok = summary.failures.length === 0;
  process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
  process.exit(summary.ok ? 0 : 1);
}

main();
