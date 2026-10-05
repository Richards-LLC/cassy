/** Shared JavaScript QA helpers used by typed journey producers. */
export function redactQaText(value: unknown, secrets?: readonly string[]): string;
export function redactQaValue<T>(value: T, secrets?: readonly string[]): T;
export function saveQaTrace(
  context: { tracing: { stop(options: { path: string }): Promise<void> } },
  path: string,
  options?: { secrets?: readonly string[] },
): Promise<void>;
