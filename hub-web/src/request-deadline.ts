/** Bound the whole operation, including signing and response-body consumption.
 * Abort asks the transport to stop; racing also settles a transport that ignores it.
 * Mutation timeouts are uncertain outcomes, never evidence of non-delivery. */
export async function withRequestDeadline<T>(
  task: (signal: AbortSignal) => Promise<T>, parent?: AbortSignal, timeoutMs = 10_000,
): Promise<T> {
  const controller = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const cancel = () => controller.abort(parent?.reason ?? new DOMException("Request cancelled", "AbortError"));
  if (parent?.aborted) cancel();
  else parent?.addEventListener("abort", cancel, { once: true });
  let abort: (() => void) | undefined;
  try {
    controller.signal.throwIfAborted();
    const cancelled = new Promise<never>((_resolve, reject) => {
      abort = () => reject(controller.signal.reason);
      controller.signal.addEventListener("abort", abort, { once: true });
      if (controller.signal.aborted) abort();
    });
    timer = setTimeout(() => controller.abort(new DOMException("Request deadline exceeded", "TimeoutError")), timeoutMs);
    return await Promise.race([Promise.resolve().then(() => task(controller.signal)), cancelled]);
  } finally {
    clearTimeout(timer);
    parent?.removeEventListener("abort", cancel);
    if (abort) controller.signal.removeEventListener("abort", abort);
  }
}
