#!/usr/bin/env node
// Vendor or check the Cassy Cloud token stylesheet (cas-eaa3). No
// dependencies: the cloud repo runs it with Node 18+ from a cassy checkout or
// a copy of this one file.
//
//   node sync-cassy-tokens.mjs --to app/cassy-tokens.css            # copy the published file
//   node sync-cassy-tokens.mjs --to app/cassy-tokens.css --check    # exit 1 on drift (CI)
//   node sync-cassy-tokens.mjs --from <url|path> --to <path> [--check]
//
// The source defaults to the published copy the hub serves. Either way the
// source must carry an intact `cassy-tokens-sha256:` header (the SHA-256 of
// everything after the header), and --check fails when the vendored copy was
// edited by hand or no longer matches the source.
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

export const DEFAULT_SOURCE = "https://hub.petrastella.io/commander/cassy-tokens.css";

/** The declared and the actual body hash of one token stylesheet. */
export function inspect(css) {
  const end = css.indexOf("*/\n");
  const declared = css.slice(0, end).match(/cassy-tokens-sha256: ([0-9a-f]{64})/)?.[1] ?? null;
  const actual = end < 0 ? null : createHash("sha256").update(css.slice(end + 3)).digest("hex");
  return { declared, actual, intact: Boolean(declared) && declared === actual };
}

async function load(source) {
  if (/^https:\/\//.test(source)) {
    const response = await fetch(source, { redirect: "error", credentials: "omit" });
    if (!response.ok) throw new Error(`${source}: HTTP ${response.status}`);
    return response.text();
  }
  if (/^[a-z]+:\/\//i.test(source)) throw new Error(`${source}: only https URLs or local paths`);
  return readFileSync(source, "utf8");
}

export async function sync({ from = DEFAULT_SOURCE, to, check = false }) {
  if (!to) throw new Error("--to <path> is required");
  const source = await load(from);
  const upstream = inspect(source);
  if (!upstream.intact) throw new Error(`${from}: cassy-tokens-sha256 header missing or does not match its body`);
  if (!check) {
    writeFileSync(to, source);
    return { status: "written", sha: upstream.declared };
  }
  let local = "";
  try { local = readFileSync(to, "utf8"); } catch { return { status: "missing", sha: upstream.declared }; }
  const vendored = inspect(local);
  if (!vendored.intact) return { status: "edited", sha: vendored.actual };
  if (vendored.declared !== upstream.declared) return { status: "drift", sha: vendored.declared, upstream: upstream.declared };
  return { status: "current", sha: upstream.declared };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const { values } = parseArgs({ options: {
    from: { type: "string", default: DEFAULT_SOURCE },
    to: { type: "string" },
    check: { type: "boolean", default: false },
  } });
  try {
    const result = await sync(values);
    const short = (sha) => sha?.slice(0, 12);
    const lines = {
      written: `wrote ${values.to} (cassy-tokens ${short(result.sha)})`,
      current: `${values.to} is current (cassy-tokens ${short(result.sha)})`,
      missing: `${values.to} is missing; run without --check to vendor it`,
      edited: `${values.to} was edited by hand; its body no longer matches its cassy-tokens-sha256 header`,
      drift: `${values.to} is cassy-tokens ${short(result.sha)}, the source is ${short(result.upstream)}; run without --check to update`,
    };
    console.log(lines[result.status]);
    process.exit(result.status === "written" || result.status === "current" ? 0 : 1);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(2);
  }
}
