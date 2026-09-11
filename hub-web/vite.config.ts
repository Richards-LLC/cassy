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
const hubBuild = digest.digest('hex').slice(0, 8);
import { defineConfig } from "vite";

export default defineConfig({
  define: { __HUB_BUILD__: JSON.stringify(hubBuild) },
  base: "/commander/",
  build: {
    assetsInlineLimit: 0,
    sourcemap: false,
    rollupOptions: {
      output: {
        entryFileNames: "app.js",
        chunkFileNames: "chunk-[name].js",
        assetFileNames(asset) {
          const name = asset.names[0] ?? "asset";
          if (name.endsWith(".css")) return "app.css";
          if (name.endsWith("ghostty-vt.wasm")) return "ghostty-vt.wasm";
          if (name.endsWith("ghostty-write-pty.wasm")) return "ghostty-write-pty.wasm";
          if (name.endsWith(".woff2")) return "symbols.woff2";
          return "[name][extname]";
        }
      }
    }
  }
});
