import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

// A content receipt stays stable after commit and identifies the actual bundle.
const root = fileURLToPath(new URL('.', import.meta.url));
const digest = createHash('sha256');
for (const dir of ['src', 'public']) {
  for (const path of readdirSync(join(root, dir), { recursive: true }).map(String).sort()) {
    if (/\.(ts|css|svg)$/.test(path) && !path.endsWith('.test.ts')) digest.update(path).update(readFileSync(join(root, dir, path)));
  }
}
for (const file of ['index.html', 'vite.config.ts', 'package-lock.json']) digest.update(readFileSync(join(root, file)));
const hubVersion = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(join(root, "../cas-cli/Cargo.toml"), "utf8"))![1];
const hubBuild = digest.update(hubVersion).digest('hex').slice(0, 8);
import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  define: { __HUB_BUILD__: JSON.stringify(hubBuild), __HUB_VERSION__: JSON.stringify(hubVersion) },
  base: "/commander/",
  plugins: [{
    name: "rich-content-licenses",
    apply: "build",
    renderChunk: {
      order: "post",
      handler(code, chunk) {
        if (chunk.name === "emoji-picker") {
          const license = readFileSync(join(root, "licenses/emojibase-MIT.txt"), "utf8");
          return { code: `${code}\n/*! Emojibase data\n${license}\n*/\n`, map: null };
        }
        if (chunk.name !== "rich-diagram") return null;
        // Append after minification, before hashing: legal text stays in this
        // lazy chunk and adds no legal-comment bytes to first paint.
        const notices = ["pele-MIT.txt", "DOMPurify-LICENSE.txt"].map((name) => readFileSync(join(root, "licenses", name), "utf8").trim()).join("\n\n");
        const header = readFileSync(join(root, "node_modules/dompurify/dist/purify.es.mjs"), "utf8").split("\n")[0];
        return { code: `${code}\n/*! Pele and DOMPurify notices\n${notices}\n*/\n${header}\n`, map: null };
      },
    },
  }],
  // cas-9b7d: keep @hpke/common's Node-only `import("crypto")` fallback out
  // of the bundle; dist must hold exactly the files the hub embeds.
  resolve: { alias: { crypto: fileURLToPath(new URL("./src/shims/node-crypto-unavailable.ts", import.meta.url)) } },
  // Playwright specs live under e2e/ and must not be collected by vitest.
  // check-journey-clock.test.mjs uses node:test and runs via `npm run check:journey-clock`.
  test: { exclude: [...configDefaults.exclude, "e2e/**", "scripts/check-journey-clock.test.mjs"] },
  build: {
    assetsInlineLimit: 0,
    sourcemap: false,
    rollupOptions: {
      output: {
        entryFileNames: "app.js",
        // cas-397a embeds these lazy assets in the production hub too.
        chunkFileNames: (chunk) => `chunk-${chunk.name.replace(/[^A-Za-z0-9_-]/g, "-")}-[hash:8].js`,
        assetFileNames(asset) {
          const name = asset.names[0] ?? "asset";
          if (name === "pdf.worker.min.mjs") return "chunk-pdf-worker-[hash:8].js";
          // Vite names the HTML entry's stylesheet index.css. Only that
          // entry owns app.css; every lazy stylesheet uses the hub chunk route.
          if (name === "index.css") return "app.css";
          if (name.endsWith(".css")) return `chunk-${name.slice(0, -4).replace(/[^A-Za-z0-9_-]/g, "-")}-[hash:8].css`;
          // cas-4427: Commander's bundled UI and mono faces (fonts.css).
          if (name.endsWith("Inter-latin-wght.woff2")) return "inter.woff2";
          if (name.endsWith("JetBrainsMono-latin-wght.woff2")) return "jetbrains-mono.woff2";
          // Any other asset keeps its name, and dist-manifest.test.ts fails
          // until the hub embeds it (cas-f285 removed the Ghostty WASM and
          // symbols font).
          return "[name][extname]";
        }
      }
    }
  }
});
