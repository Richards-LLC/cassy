export type Scheme = "light" | "dark";
export type SchemePreference = Scheme | "system";
const STORAGE_KEY = "commander.scheme";
let preference: SchemePreference = "system";
let media: MediaQueryList | undefined;

function renderScheme(): Scheme {
  const scheme = preference === "system" ? (media?.matches ? "dark" : "light") : preference;
  document.documentElement.dataset.scheme = scheme;
  document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')?.setAttribute(
    "content", getComputedStyle(document.documentElement).getPropertyValue("--bg-root").trim(),
  );
  return scheme;
}

/** Apply the stored preference at boot; track OS changes while system is selected. */
export function applyScheme(): Scheme {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    preference = stored === "light" || stored === "dark" ? stored : "system";
  } catch {
    preference = "system";
  }
  media?.removeEventListener?.("change", renderScheme);
  media = typeof window.matchMedia === "function" ? window.matchMedia("(prefers-color-scheme: dark)") : undefined;
  media?.addEventListener("change", renderScheme);
  return renderScheme();
}

/** Persist an Appearance choice. Storage denial still allows this page to change. */
export function setScheme(next: SchemePreference): Scheme {
  preference = next;
  try { localStorage.setItem(STORAGE_KEY, next); } catch { /* Preference lasts for this page. */ }
  return renderScheme();
}

/** The stored Appearance choice (not the resolved scheme): "system" stays "system". */
export function schemePreference(): SchemePreference {
  return preference;
}

const APPEARANCE_HINT: Record<SchemePreference, string> = { system: "Follow this device", light: "Use this scheme", dark: "Use this scheme" };

/**
 * Mark the Appearance row that is in effect (journey F20): a check before its
 * name, "Current" as its description, and aria-current for assistive tech.
 * Rows are updated in place, so the mark follows a choice without a rebuild.
 */
export function markAppearanceCommands(root: ParentNode, current: SchemePreference = preference): void {
  for (const row of root.querySelectorAll<HTMLElement>("[data-palette-scheme]")) {
    const scheme = row.dataset.paletteScheme as SchemePreference;
    const active = scheme === current;
    row.toggleAttribute("aria-current", active);
    if (active) row.setAttribute("aria-current", "true");
    const check = row.querySelector<HTMLElement>(".palette-check");
    if (check) check.hidden = !active;
    const hint = row.querySelector<HTMLElement>("small");
    if (hint) hint.textContent = active ? (scheme === "system" ? "Current · follows this device" : "Current") : APPEARANCE_HINT[scheme];
  }
}
