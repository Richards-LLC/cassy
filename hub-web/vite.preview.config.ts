import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

// Builds the clickable Commander preview (real src/main.ts + in-page fixture hub).
// Output preview-dist/ is a static site; nothing here ships in hub-web/dist.
export default defineConfig({
  root: fileURLToPath(new URL("./preview", import.meta.url)),
  define: { __HUB_BUILD__: JSON.stringify("preview") },
  resolve: { alias: { "node:crypto": fileURLToPath(new URL("./preview/crypto-shim.ts", import.meta.url)) } },
  build: {
    outDir: fileURLToPath(new URL("./preview-dist/commander/preview", import.meta.url)), emptyOutDir: true, target: "es2022",
    lib: { entry: fileURLToPath(new URL("./preview/main.ts", import.meta.url)), formats: ["es"], fileName: () => "shim.js" },
  },
});
