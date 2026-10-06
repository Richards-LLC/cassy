// The journey clock reads process.env; in the preview "now" is the visitor's now.
(globalThis as unknown as { process: { env: Record<string, string> } }).process = {
  env: { HUB_JOURNEY_NOW: new Date(Math.floor(Date.now() / 1000) * 1000).toISOString() },
};
(globalThis as unknown as { Buffer: unknown }).Buffer = {
  from(value: string, encoding: string) {
    const b64 = encoding === "base64url" ? value.replace(/-/g, "+").replace(/_/g, "/") : value;
    return Uint8Array.from(atob(b64 + "===".slice((b64.length + 3) % 4)), (c) => c.charCodeAt(0));
  },
};
