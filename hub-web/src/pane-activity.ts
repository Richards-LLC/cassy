/**
 * The time in a Terminal view pane header (cas-010f, journey F42).
 *
 * It is the pane's last output this page received. A pane that opened on
 * output from before this page (a keyframe or scrollback replay) shows that
 * output, so its header must not say "No output yet" above it.
 */

import { absoluteTimestamp, relativeTimestamp } from "./time";

const ESC = 0x1b;
const BEL = 0x07;

/**
 * Whether terminal bytes draw any visible glyph: a printable character
 * outside an escape sequence. Cursor moves, clears, colours and titles alone
 * leave a pane blank. Stops at the first glyph.
 */
export function paneShowsOutput(bytes: ArrayLike<number>): boolean {
  let index = 0;
  while (index < bytes.length) {
    const byte = bytes[index]!;
    if (byte === ESC) {
      const kind = bytes[index + 1];
      index += 2;
      if (kind === 0x5b) {
        // CSI: parameters, then one final byte in @–~.
        while (index < bytes.length && !(bytes[index]! >= 0x40 && bytes[index]! <= 0x7e)) index += 1;
        index += 1;
      } else if (kind === 0x5d || kind === 0x50 || kind === 0x5f || kind === 0x5e) {
        // OSC, DCS, APC, PM: a string ended by BEL or ESC \.
        while (index < bytes.length && bytes[index] !== BEL && !(bytes[index] === ESC && bytes[index + 1] === 0x5c)) index += 1;
        index += bytes[index] === BEL ? 1 : 2;
      } else if (kind !== undefined && kind >= 0x20 && kind <= 0x2f) {
        // ESC ( B and the like: intermediates, then one final byte.
        while (index < bytes.length && bytes[index]! >= 0x20 && bytes[index]! <= 0x2f) index += 1;
        index += 1;
      }
      continue;
    }
    // Printable ASCII (not space) or any byte of a multi-byte UTF-8 character.
    if ((byte > 0x20 && byte < 0x7f) || byte >= 0x80) return true;
    index += 1;
  }
  return false;
}

export interface PaneActivityLabel {
  readonly text: string;
  readonly title: string;
}

/**
 * The pane header's time and its tooltip: the last output this page saw;
 * else, when the pane shows output from before this page opened, says that;
 * else "No output yet".
 */
export function paneActivityLabel(lastOutputAt: number | undefined, showsEarlierOutput: boolean, now = Date.now()): PaneActivityLabel {
  if (lastOutputAt !== undefined) return { text: relativeTimestamp(lastOutputAt, now), title: absoluteTimestamp(lastOutputAt) };
  if (showsEarlierOutput) return { text: "Earlier output", title: "Output from before this page opened; nothing new since" };
  return { text: "No output yet", title: "No output received since this page opened" };
}
