/**
 * Tailscale's 100.64/10 addresses are local in Chromium's LNA mapping.
 * Query only the granular permission: the old local-network-access alias
 * crashes some older Chromium builds. Unsupported browsers keep retrying.
 * The help carries only the remediation (cas-7c37f): the connection state
 * ("Can't reach…", "Reconnecting…") is said once, by the list or the banner.
 * https://github.com/GoogleChrome/modern-web-guidance/blob/main/skills/modern-web-guidance/guides/security/local-network-access.md
 */
export async function localNetworkAccessHelp(baseUrl: string, label: string, measured?: (permission: "denied" | "prompt" | "granted" | "unknown") => void): Promise<string | undefined> {
  const target = new URL(baseUrl);
  if (typeof location !== "undefined" && target.origin === location.origin) return;
  if (!target.hostname.endsWith(".ts.net")) return;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const status = await Promise.race([
      navigator.permissions.query({ name: "local-network" as PermissionName }),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error("Permission query timed out")), 500);
      }),
    ]);
    measured?.(status.state);
    if (status.state === "denied") {
      return `To reach ${label}, allow Local network access for this page in your browser's site settings.`;
    }
    if (status.state === "prompt") {
      return `To reach ${label}, allow this page to connect to your local network when your browser asks.`;
    }
  } catch {
    measured?.("unknown");
    // Older Chrome and other engines do not expose the granular permission.
    return `To reach ${label}, check Tailscale and this page's Local network access in your browser's site settings.`;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
