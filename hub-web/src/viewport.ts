/**
 * One definition of "this is a phone", shared by the stylesheet and by the
 * layout logic in main.ts.
 *
 * A width-only breakpoint is not a phone test, it is a narrow-window test. A
 * Pixel 7 rotated is 915 CSS px wide and 412 tall, so it cleared the 848px
 * breakpoint and received the three-column desktop console — rails, a 320px
 * context column and a worker strip — on a screen with less vertical room than
 * a keyboard (report defect D5). The rule below asks the two questions that
 * actually distinguish the device: is either axis too small for desktop chrome,
 * and is this a finger rather than a mouse.
 *
 * The pointer clause is what keeps a desktop out of the phone layout when its
 * window is dragged short, and the width clause is unconditional so the
 * documented 53rem breakpoint keeps behaving exactly as it did for narrow
 * windows of every kind.
 */
export const PHONE_MAX_WIDTH_REM = 53;
export const PHONE_MAX_SHORT_AXIS_REM = 30;
/** Media-query lengths resolve against the initial root font size, not html's. */
export const ROOT_FONT_PX = 16;

export const LANDSCAPE_PHONE_MEDIA_QUERY = `(max-height: ${PHONE_MAX_SHORT_AXIS_REM}rem) and (pointer: coarse)`;
export const PHONE_MEDIA_QUERY = `(max-width: ${PHONE_MAX_WIDTH_REM}rem), ${LANDSCAPE_PHONE_MEDIA_QUERY}`;
/**
 * Deliberately width-only, and deliberately not the phone rule. This one asks
 * how many columns fit across the mount — the question behind the 80-column PTY
 * floor and the transcript default — and a phone in landscape genuinely has the
 * width for a wider grid. Phone chrome is a short-axis question; the column
 * floor is not.
 */
export const COMPACT_MEDIA_QUERY = `(max-width: ${PHONE_MAX_WIDTH_REM}rem)`;

export interface ViewportEnvironment {
  readonly width: number;
  readonly height: number;
  readonly coarsePointer: boolean;
}

/** The same rule as PHONE_MEDIA_QUERY, in a form a test can measure. */
export function isPhoneLayout(environment: ViewportEnvironment): boolean {
  return environment.width <= PHONE_MAX_WIDTH_REM * ROOT_FONT_PX || isLandscapePhone(environment);
}

/** A phone whose short axis is the one that ran out: the rotated case. */
export function isLandscapePhone(environment: ViewportEnvironment): boolean {
  return environment.coarsePointer && environment.height <= PHONE_MAX_SHORT_AXIS_REM * ROOT_FONT_PX;
}
