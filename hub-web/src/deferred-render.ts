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
 *
 * A touch is later still: the browser fires pointerup when the finger lifts,
 * and only then dispatches the tap's mousedown (which moves focus off the
 * field) and its click, in later tasks. Releasing on pointerup let focusout
 * flush the rebuild between them, so the tapped control was replaced and the
 * tap did nothing (measured on the pairing dialog's Technical details). A
 * lifted touch therefore holds the rebuild until its click, or until
 * `touchWindow` passes without one.
 */
export interface DeferredRenderOptions {
  readonly render: () => void;
  readonly afterGesture: (run: () => void) => void;
  /** Runs `run` once a lifted touch has had time to produce its click. */
  readonly touchWindow?: (run: () => void) => void;
}

export class DeferredRenderScheduler {
  private owed = false;
  private gestureDepth = 0;
  private awaitingTap = false;
  /** Which lifted touch a `touchWindow` timer belongs to; a stale one is ignored. */
  private touches = 0;

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
    this.awaitingTap = false;
    if (this.gestureDepth === 0) return;
    this.gestureDepth = 0;
    // The click has not been dispatched yet; rebuilding now would still delete
    // the button the operator is pressing.
    this.options.afterGesture(() => this.flush());
  }

  /** A finger lifted; its click, if any, is still to come. */
  touchEnded(): void {
    if (this.gestureDepth === 0) return;
    if (!this.options.touchWindow) {
      this.gestureEnded();
      return;
    }
    this.awaitingTap = true;
    const touch = ++this.touches;
    this.options.touchWindow(() => {
      if (this.awaitingTap && touch === this.touches) this.gestureEnded();
    });
  }

  /** A click was dispatched; a lifted touch waiting for it is over. */
  clicked(): void {
    if (this.awaitingTap) this.gestureEnded();
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
