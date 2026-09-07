#!/usr/bin/env node
// Mapping contract: docs/design/hub-web/token-map.md. No runtime dependencies.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const { values } = parseArgs({ options: {
  source: { type: "string", default: fileURLToPath(new URL("../../docs/design/design-tokens.json", import.meta.url)) },
  output: { type: "string", default: fileURLToPath(new URL("../src/tokens.css", import.meta.url)) },
  check: { type: "boolean", default: false },
} });

try {
  const source = JSON.parse(readFileSync(values.source, "utf8"));
  function required(path) {
    const value = path.split(".").reduce((node, key) => node?.[key], source);
    if (value === undefined || value === null) throw new Error(`Missing required token path: ${path}`);
    return value;
  }
  const token = (path) => required(`${path}.$value`);
  const type = (step, field) => required(`typography.scale.${step}.$value.${field}`);
  const family = (role) => token(`typography.family.${role}`).map((name) => name.includes(" ") ? `"${name}"` : name).join(", ");
  const overlay = token("elevation.overlay");
  const overlayColor = overlay.match(/rgba?\([^)]+\)/)?.[0];
  if (!overlayColor) throw new Error("Missing colour component in required token path: elevation.overlay.$value");

  // Hub-only values explicitly retained by the map; house values come from JSON.
  const derived = {
    "--bg-raised": "color-mix(in srgb, var(--bg-panel) 96%, var(--text-hi))",
    "--bg-hover": "color-mix(in srgb, var(--bg-panel) 92%, var(--text-hi))",
    "--overlay-backdrop": "color-mix(in srgb, var(--bg-root) 72%, transparent)",
  };
  const common = {
    ...derived,
    "--bg-terminal": "#0C0E13",
    "--overlay-shadow-color": overlayColor,
    "--color-transparent": "transparent",
    "--font-ui": family("body"),
    "--font-mono": family("mono"),
    "--font-display": family("display"),
    "--fs-xs": type("eyebrow", "fontSize"),
    "--fs-meta": "13px",
    "--fs-base": type("caption", "fontSize"),
    "--fs-terminal": "13px",
    "--fs-md": type("ledger", "fontSize"),
    "--fs-lg": type("lede", "fontSize"),
    "--fs-verdict": `clamp(24px, 3vw, ${type("title", "fontSize")})`,
    "--weight-regular": type("caption", "fontWeight"),
    "--weight-medium": type("hero-number", "fontWeight"),
    "--weight-semibold": type("heading", "fontWeight"),
    "--tracking-label": type("eyebrow", "letterSpacing"),
    "--line-ui": (parseFloat(type("caption", "lineHeight")) / parseFloat(type("caption", "fontSize"))).toFixed(2),
    "--line-terminal": "1.35",
    "--root-font-size": "16px",
    ...Object.fromEntries([1, 2, 3, 4, 6, 8, 12, 16].map((step) => [`--space-${step}`, token(`space.${step}`)])),
    "--radius-card": token("radius.chip"),
    "--radius-pane": token("radius.panel"),
    "--radius-pill": "999px",
    "--line-width": token("chart.hairline"),
    "--state-rule-width": "2px",
    "--focus-ring-width": "2px",
    "--shadow-overlay": overlay,
    "--rule-verdict": token("chart.mark-decisive"),
    "--rule-hero": "3px",
    "--machine-rail-width": "48px",
    "--machine-drawer-width": "280px",
    "--context-panel-width": "320px",
    "--session-header-height": "44px",
    "--pane-secondary-min-width": "280px",
    "--pane-header-height": "32px",
    "--worker-collapsed-width": "240px",
    "--toolbar-height": "72px",
    "--button-height": "40px",
    "--button-compact-height": "28px",
    "--dialog-width": "520px",
    "--terminal-state-width": "360px",
    "--pair-detail-label-width": "140px",
    "--mobile-drawer-max-height": "520px",
    "--mobile-pane-min-width": "260px",
    "--mobile-attention-label-width": "200px",
    "--fleet-board-max-width": token("layout.container"),
    "--mobile-header-chip-width": "72px",
    "--mobile-context-pill-width": "152px",
    "--rail-item-min": "44px",
    "--landscape-attention-rail-width": "80px",
    "--browser-notice-height": "32px",
    "--attention-payload-max-height": "180px",
    "--attention-motion-duration": token("motion.reveal"),
    "--chrome-motion-duration": token("motion.chrome"),
    "--motion-easing": `cubic-bezier(${token("motion.easing").join(", ")})`,
    "--connection-log-max-height": "60dvh",
  };
  const roles = {
    "--bg-root": "bg", "--bg-panel": "surface", "--bg-active": "verdict-soft",
    "--line-subtle": "line", "--line-strong": "line-strong",
    "--text-hi": "ink", "--text-mid": "ink-muted",
    "--state-ok": "good", "--state-warn": "warning", "--state-crit": "danger",
    "--tint-warn": "warning-tint", "--tint-crit": "danger-tint",
    "--color-verdict": "verdict", "--color-action": "action", "--color-focus": "focus",
  };
  function colors(scheme) {
    return {
      ...Object.fromEntries(Object.entries(roles).map(([name, role]) => [name, token(`color.${scheme}.${role}`)])),
      "--state-idle": token(`color.series-neutral.${scheme}`),
      "--color-series-neutral": token(`color.series-neutral.${scheme}`),
    };
  }
  const declarations = (props) => Object.entries(props).map(([name, value]) => `  ${name}: ${value};`).join("\n");
  const block = (selector, props, scheme = "light dark") => `${selector} {\n  color-scheme: ${scheme};\n${declarations(props)}\n}\n`;
  const light = { ...common, ...colors("light") };
  const dark = { ...common, ...colors("dark") };
  const output = "/* Generated by hub-web/scripts/generate-tokens.mjs. Do not edit.\n * Source: docs/design/design-tokens.json + docs/design/hub-web/token-map.md.\n */\n\n"
    + block(":root", light)
    + "\n@media (prefers-color-scheme: dark) {\n" + block(":root", dark).trimEnd().split("\n").map((line) => `  ${line}`).join("\n") + "\n}\n\n"
    + block('html[data-scheme="light"]', light, "light") + "\n"
    + block('html[data-scheme="dark"]', dark, "dark") + "\n"
    + "/* Dark wells: color.dark.* + color.series-neutral.dark; derived surfaces use those roles. */\n"
    + ".terminal-mount, .transcript, .terminal-search input, .attention-payload pre,\n.connection-log pre, dialog:not(.command-palette) input, .pair-code {\n  color-scheme: dark;\n"
    + declarations({ ...derived, ...colors("dark") }) + "\n}\n";
  if (values.check) {
    if (readFileSync(values.output, "utf8") !== output) throw new Error("Generated tokens.css has drifted; run npm run tokens and commit the result.");
  } else {
    writeFileSync(values.output, output);
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
