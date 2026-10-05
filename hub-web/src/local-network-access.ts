/**
 * Tailscale's 100.64/10 addresses are local in Chromium's LNA mapping.
 * Query only the granular permission: the old local-network-access alias
 * crashes some older Chromium builds. Unsupported browsers keep retrying.
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
      return `Can't reach ${label}. Allow Local network access for this page in your browser's site settings. Reconnecting…`;
    }
    if (status.state === "prompt") {
      return `Can't reach ${label}. Allow this page to connect to your local network when your browser asks. Reconnecting…`;
    }
  } catch {
    measured?.("unknown");
    // Older Chrome and other engines do not expose the granular permission.
    return `Can't reach ${label}. Check Tailscale and this page's Local network access in your browser's site settings. Reconnecting…`;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
