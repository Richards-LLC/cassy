import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config";

// The test runner's own settings live here, not in vite.config.ts, so they
// never change the hub build digest that vite.config.ts feeds (__HUB_BUILD__).
export default mergeConfig(viteConfig, defineConfig({
  test: {
    // cas-ad70: jsdom test files see jsdom's Web Storage on every Node.
    setupFiles: ["test/webstorage-setup.ts"],
  },
}));
