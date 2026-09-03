/**
 * When a rebuild that was deferred while the operator was typing is allowed to
 * happen.
 *
 * Deferring on focus (cas-8434) kept a structural change from yanking the
 * keyboard mid-sentence, but flushing it on `focusout` alone broke clicking:
 * pointerdown moves focus off the field, focusout fires, the shell is rebuilt,
 * and the button under the finger is replaced before the browser dispatches
 * the click — so the click lands on nothing and the handler never runs
 * (cas-c142, measured on the pairing dialog's Cancel).
 *
 * A rebuild therefore waits for the whole pointer gesture, not just the focus
 * change. `afterGesture` must schedule work that runs *after* the click event
 * the gesture is about to produce.
 */
export interface DeferredRenderOptions {
  readonly render: () => void;
  readonly afterGesture: (run: () => void) => void;
}

export class DeferredRenderScheduler {
  private owed = false;
  private gestureDepth = 0;

  constructor(private readonly options: DeferredRenderOptions) {}

  /** A structural render was skipped because a field had focus. */
  defer(): void {
    this.owed = true;
  }

  /** The page rebuilt for its own reasons, so nothing is owed any more. */
  settled(): void {
    this.owed = false;
  }

  get pending(): boolean {
    return this.owed;
  }

  gestureStarted(): void {
    this.gestureDepth += 1;
  }

  gestureEnded(): void {
    if (this.gestureDepth === 0) return;
    this.gestureDepth = 0;
    // The click has not been dispatched yet; rebuilding now would still delete
    // the button the operator is pressing.
    this.options.afterGesture(() => this.flush());
  }

  /** A gesture that will never produce a click still releases the rebuild. */
  gestureCancelled(): void {
    this.gestureEnded();
  }

  /** Focus left an editable control. */
  focusLeft(): void {
    if (this.gestureDepth > 0) return;
    this.flush();
  }

  private flush(): void {
    if (!this.owed) return;
    this.owed = false;
    this.options.render();
  }
}
