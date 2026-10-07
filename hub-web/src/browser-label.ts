/**
 * cas-97d58 F21: the name a person would give this browser ("Chrome on
 * Linux", "Safari on iPhone"), not "Commander on Linux x86_64".
 */
export function browserLabel(userAgent: string): string {
  const browser = /Edg(e|A|iOS)?\//.test(userAgent) ? "Edge"
    : /Firefox\/|FxiOS\//.test(userAgent) ? "Firefox"
      : /Chrome\/|CriOS\//.test(userAgent) ? "Chrome"
        : /Safari\//.test(userAgent) ? "Safari"
          : "Browser";
  const device = /iPhone/.test(userAgent) ? "iPhone"
    : /iPad/.test(userAgent) ? "iPad"
      : /Android/.test(userAgent) ? "Android"
        : /Mac OS X|Macintosh/.test(userAgent) ? "Mac"
          : /Windows/.test(userAgent) ? "Windows"
            : /CrOS/.test(userAgent) ? "ChromeOS"
              : /Linux/.test(userAgent) ? "Linux"
                : "this device";
  return `${browser} on ${device}`;
}
