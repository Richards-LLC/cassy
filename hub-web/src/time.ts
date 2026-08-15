export function absoluteTimestamp(value: string | number): string {
  return new Date(value).toISOString();
}

export function relativeTimestamp(value: string | number | undefined, now = Date.now()): string {
  if (value === undefined) return "waiting";
  const timestamp = typeof value === "number" ? value : Date.parse(value);
  const elapsed = Math.max(0, now - timestamp);
  if (elapsed < 5_000) return "now";
  if (elapsed < 60_000) return `${Math.floor(elapsed / 1_000)}s`;
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)}m`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)}h`;
  return `${Math.floor(elapsed / 86_400_000)}d`;
}
