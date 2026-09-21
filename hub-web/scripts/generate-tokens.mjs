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
    "--conversation-rail-width": "374px",
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
    // Layout viewport height; the visualViewport fallback overrides it inline while a phone keyboard is up (cas-edc9).
    "--keyboard-viewport-height": "100dvh",
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
  // Pebble (EPIC cas-cac1): the PAPER conversation surface. These names are the
  // shared contract every Pebble child consumes and never redefines. House roles
  // are read from design-tokens.json; the rest are hub-only literals ported from
  // docs/design/hub-messaging/round-3/pebble.css (light) and its dark block.
  const pebbleLiterals = {
    light: {
      "--sheet-bg": "#FFFFFF", "--fold": "#EAE5DB", "--ink-soft": "#494E5C",
      "--you-fg": "#FFFFFF", "--ask-fg": "#1B1D24", "--ask-deep": "#7F5504", "--crit-fg": "#FFFFFF", "--accent-fg": "#FFFFFF",
      "--lift": "0 1px 2px rgba(18,20,26,0.05), 0 6px 18px rgba(18,20,26,0.06)",
      "--lift-strong": "0 2px 4px rgba(18,20,26,0.08), 0 14px 34px rgba(18,20,26,0.10)",
      "--lift-edge": "10px 0 30px -18px rgba(18,20,26,0.22)",
      "--lift-head": "0 8px 20px -14px rgba(18,20,26,0.30)",
    },
    dark: {
      "--sheet-bg": "#1F232D", "--fold": "#262B35", "--ink-soft": "#B4B8C4",
      "--you-fg": "#12141A", "--ask-fg": "#12141A", "--ask-deep": "#6E551C", "--crit-fg": "#12141A", "--accent-fg": "#12141A",
      "--lift": "0 1px 2px rgba(0,0,0,0.32), 0 6px 18px rgba(0,0,0,0.34)",
      "--lift-strong": "0 2px 4px rgba(0,0,0,0.40), 0 14px 34px rgba(0,0,0,0.46)",
      "--lift-edge": "10px 0 30px -18px rgba(0,0,0,0.60)",
      "--lift-head": "0 8px 20px -14px rgba(0,0,0,0.70)",
    },
  };
  // Per-machine accent set. Index N is the class `machine-accent-N` chosen by
  // hub-web/src/machine-accent.ts (FNV-1a → jump consistent hash), so a fourth
  // accent is APPENDED here and MACHINE_ACCENT_COUNT bumped; never reorder the
  // first three or every paired machine changes colour. Every pair rendered on
  // these values is measured at >= 4.5:1 by machine-accent.test.ts.
  const machineAccents = [
    { name: "indigo", light: { accent: "#2E3A9F", soft: "#DDE1F7", sup: "#E7EAF7" }, dark: { accent: "#A9B3FF", soft: "#232838", sup: "#262B38" } },
    { name: "green", light: { accent: "#226845", soft: "#D7E8DE", sup: "#E3EFE8" }, dark: { accent: "#5FC492", soft: "#1C2A23", sup: "#1F2E27" } },
    { name: "violet", light: { accent: "#5B3E8C", soft: "#E2DAF1", sup: "#EBE6F4" }, dark: { accent: "#C0A3F0", soft: "#262034", sup: "#282334" } },
  ];
  const accent = (scheme, index) => ({
    "--accent": machineAccents[index][scheme].accent,
    "--accent-soft": machineAccents[index][scheme].soft,
    "--sup-bg": machineAccents[index][scheme].sup,
    "--sup-fg": token(`color.${scheme}.ink`),
  });
  function pebble(scheme) {
    return {
      "--canvas": token(`color.${scheme}.bg`),
      "--panel": token(`color.${scheme}.surface`),
      "--ink": token(`color.${scheme}.ink`),
      "--ink-mid": token(`color.${scheme}.ink-muted`),
      "--you-bg": token(`color.${scheme}.action`),
      // The ask amber is the dark warning value in both schemes: the light value
      // (#7F5504) only clears 4.5:1 as a near-brown, so it serves as the tray.
      "--ask-bg": token("color.dark.warning"),
      "--warn-text": token(`color.${scheme}.warning`),
      "--crit-bg": token(`color.${scheme}.danger`),
      ...pebbleLiterals[scheme],
      // Unscoped default: the first accent, so a surface outside any machine
      // still resolves; rows and thread roots carry machine-accent-N.
      ...accent(scheme, 0),
    };
  }
  const declarations = (props) => Object.entries(props).map(([name, value]) => `  ${name}: ${value};`).join("\n");
  const block = (selector, props, scheme = "light dark") => `${selector} {\n  color-scheme: ${scheme};\n${declarations(props)}\n}\n`;
  const plain = (selector, props) => `${selector} {\n${declarations(props)}\n}\n`;
  const indent = (text) => text.trimEnd().split("\n").map((line) => `  ${line}`).join("\n");
  const light = { ...common, ...colors("light"), ...pebble("light") };
  const dark = { ...common, ...colors("dark"), ...pebble("dark") };
  const accentBlocks = machineAccents.map((set, index) => `/* ${index}: ${set.name} */\n`
    + plain(`.machine-accent-${index}`, accent("light", index))
    + "@media (prefers-color-scheme: dark) {\n" + indent(plain(`.machine-accent-${index}`, accent("dark", index))) + "\n}\n"
    + plain(`html[data-scheme="light"] .machine-accent-${index}`, accent("light", index))
    + plain(`html[data-scheme="dark"] .machine-accent-${index}`, accent("dark", index))).join("\n");
  const output = "/* Generated by hub-web/scripts/generate-tokens.mjs. Do not edit.\n * Source: docs/design/design-tokens.json + docs/design/hub-web/token-map.md.\n */\n\n"
    + block(":root", light)
    + "\n@media (prefers-color-scheme: dark) {\n" + indent(block(":root", dark)) + "\n}\n\n"
    + block('html[data-scheme="light"]', light, "light") + "\n"
    + block('html[data-scheme="dark"]', dark, "dark") + "\n"
    + "/* Pebble machine accents (hub-web/src/machine-accent.ts): append a set, never reorder. */\n"
    + accentBlocks + "\n"
    + "/* Dark wells: color.dark.* + color.series-neutral.dark; derived surfaces use those roles. */\n"
    + ".terminal-mount:not(.conversation-active), .transcript, .terminal-search input, .attention-payload pre,\n.connection-log pre, dialog:not(.command-palette) input {\n  color-scheme: dark;\n"
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
