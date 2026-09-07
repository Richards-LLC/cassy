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
