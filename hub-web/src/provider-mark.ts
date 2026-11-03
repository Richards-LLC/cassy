import { escapeHtml } from "./cloud-brand";

/** Harnesses a session or worker may report (hub catalog and worker_status). */
export type Harness = "claude" | "codex" | "grok" | "opencode";
export type AvatarState = "working" | "idle" | "needs-you" | "offline";

/** The provider behind each harness, as the operator names the model's maker. */
export const HARNESS_PROVIDER: Readonly<Record<Harness, string>> = { claude: "Claude", codex: "OpenAI", grok: "Grok", opencode: "OpenCode" };
/** The harness as a product name, for the header line when no model is reported. */
export const HARNESS_NAME: Readonly<Record<Harness, string>> = { claude: "Claude Code", codex: "Codex", grok: "Grok", opencode: "OpenCode" };

const ray = (angle: number, length: number) => {
  const r = (angle * Math.PI) / 180;
  const x = (12 + Math.cos(r) * length).toFixed(2), y = (12 + Math.sin(r) * length).toFixed(2);
  return `<path d="M12 12L${x} ${y}"/>`;
};
const petal = (angle: number) => `<rect x="9" y="2.6" width="6" height="12.4" rx="3" transform="rotate(${angle} 12 12)"/>`;

/**
 * Single-colour provider marks on a 24-unit grid (DESIGN.md Avatars). Claude's
 * spark keeps its terracotta in both schemes; the others draw in the avatar's
 * ink so they hold contrast on either disc.
 */
const MARKS: Readonly<Partial<Record<Harness, string>>> = {
  claude: `<g stroke="#D97757" stroke-width="2.3" stroke-linecap="round">${[0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330].map((angle, index) => ray(angle - 90, index % 2 ? 7.4 : 10)).join("")}</g>`,
  codex: `<g fill="none" stroke="currentColor" stroke-width="1.6">${[0, 60, 120, 180, 240, 300].map(petal).join("")}</g>`,
  grok: `<g fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M17.6 6.9A8 8 0 1 0 18.9 15"/><path d="M4.5 20.5L20.5 3.5"/></g>`,
};

/**
 * The header's model label: "Claude Opus 5.5 · high" from a model ID such as
 * claude-opus-5-5, or the harness name when no model is reported (DESIGN.md
 * Conversation header).
 */
export function modelLabel(harness: string | undefined, model?: string | null, effort?: string | null): string | undefined {
  const known = isHarness(harness) ? harness : undefined;
  const id = model?.trim();
  if (!id) return known ? HARNESS_NAME[known] : undefined;
  const words = id.replace(/(\d+)[-_](?=\d)/g, "$1.").split(/[-_\s]+/).filter(Boolean).map(word => /^gpt$/i.test(word) ? "GPT" : word[0]!.toUpperCase() + word.slice(1));
  let name = words.join(" ").replace(/^GPT (\d)/, "GPT-$1");
  if (known === "claude" && !/^claude\b/i.test(name)) name = `Claude ${name}`;
  return effort?.trim() ? `${name} · ${effort.trim()}` : name;
}

export function isHarness(value: unknown): value is Harness {
  return typeof value === "string" && Object.hasOwn(HARNESS_PROVIDER, value);
}

/** Two letters for a harness or name with no drawn mark ("OpenCode" → "OC"). */
export function avatarLetters(name: string): string {
  const words = name.match(/[A-Z]?[a-z0-9]+|[A-Z]+(?![a-z])/g) ?? [];
  const letters = words.length > 1 ? words[0]![0]! + words[1]![0]! : (words[0] ?? name).slice(0, 2);
  return letters.toUpperCase() || "?";
}

/**
 * The avatar of a session or worker: its provider's mark at a legible size, a
 * two-letter fallback for a harness without one, or `fallback` (the machine
 * monogram) when no harness is reported. The status dot rides the edge.
 */
export function providerAvatarMarkup(options: { harness?: string | null; fallback: string; state?: AvatarState; size?: "row" | "lane"; className?: string }): string {
  const harness = isHarness(options.harness) ? options.harness : undefined;
  const label = options.state ? { working: "Working", idle: "Idle", "needs-you": "Needs you", offline: "Offline" }[options.state] : undefined;
  const dot = options.state ? `<span class="supervisor-state ${options.state}" role="img" aria-label="${label}" title="${label}"></span>` : "";
  // No harness on a surface with its own avatar class: that avatar, unchanged.
  if (!harness && options.className) return `<span class="${options.className}"><span aria-hidden="true">${escapeHtml(options.fallback)}</span>${dot}</span>`;
  const mark = harness ? MARKS[harness] : undefined;
  const face = mark
    ? `<svg class="provider-mark" viewBox="0 0 24 24" aria-hidden="true" focusable="false">${mark}</svg>`
    : `<span class="provider-letters" aria-hidden="true">${escapeHtml(harness ? avatarLetters(HARNESS_PROVIDER[harness]) : options.fallback)}</span>`;
  const classes = ["provider-avatar", options.size === "lane" ? "provider-avatar--lane" : "", harness ? "harness-mark" : "", options.className ?? ""].filter(Boolean).join(" ");
  return `<span class="${classes}"${harness ? ` data-harness="${harness}" title="${HARNESS_PROVIDER[harness]}"` : ""}>${face}${dot}</span>`;
}
