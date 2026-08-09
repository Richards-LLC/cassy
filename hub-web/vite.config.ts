import { defineConfig } from "vite";

export default defineConfig({
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
