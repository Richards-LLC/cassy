/**
 * AbortSignal.any is Chrome 116 / Firefox 124 / Safari 17.4. Calling it bare
 * threw a TypeError on every older engine — Android WebViews sit years behind —
 * which took out the hub event stream and, with it, every terminal attach
 * (report cas-b652, defect D3). The fallback is small enough that the app has
 * no reason to carry that version floor.
 */

type AnySignalFactory = (signals: readonly AbortSignal[]) => AbortSignal;

function nativeAnySignal(): AnySignalFactory | undefined {
  const candidate = (AbortSignal as unknown as { any?: unknown }).any;
  return typeof candidate === "function" ? candidate.bind(AbortSignal) as AnySignalFactory : undefined;
}

export function hasNativeAnySignal(): boolean {
  return nativeAnySignal() !== undefined;
}

export function anySignal(signals: readonly AbortSignal[]): AbortSignal {
  const native = nativeAnySignal();
  if (native) return native(signals);
  const controller = new AbortController();
  const detach: (() => void)[] = [];
  const stopListening = (): void => {
    for (const remove of detach.splice(0)) remove();
  };
  for (const signal of signals) {
    if (signal.aborted) {
      stopListening();
      controller.abort(signal.reason);
      return controller.signal;
    }
    const onAbort = (): void => {
      stopListening();
      controller.abort(signal.reason);
    };
    signal.addEventListener("abort", onAbort, { once: true });
    detach.push(() => signal.removeEventListener("abort", onAbort));
  }
  return controller.signal;
}
