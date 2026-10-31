import { escapeHtml } from "./cloud-brand";

// Whole sequences, including keycaps, flags, skin tones, ZWJ families and tag flags.
// Keep digits, punctuation, code and ordinary UI text in the original typeface.
const EMOJI = /(?:[0-9#*]\uFE0F?\u20E3|\p{Regional_Indicator}{2}|\p{Extended_Pictographic}[\uFE0E\uFE0F]?\p{Emoji_Modifier}?(?:\u200D\p{Extended_Pictographic}[\uFE0E\uFE0F]?\p{Emoji_Modifier}?)*(?:[\u{E0020}-\u{E007E}]+\u{E007F})?)/gu;

export function emojiMarkup(text: string): string {
  let result = ""; let start = 0;
  for (const match of text.matchAll(EMOJI)) {
    result += escapeHtml(text.slice(start, match.index)) + `<span class="emoji-text">${escapeHtml(match[0])}</span>`;
    start = match.index + match[0].length;
  }
  return result + escapeHtml(text.slice(start));
}

export function appendEmojiText(document: Document, parent: HTMLElement, text: string): void {
  let start = 0;
  for (const match of text.matchAll(EMOJI)) {
    if (match.index > start) parent.append(document.createTextNode(text.slice(start, match.index)));
    const span = document.createElement("span"); span.className = "emoji-text"; span.textContent = match[0]; parent.append(span);
    start = match.index + match[0].length;
  }
  if (start < text.length) parent.append(document.createTextNode(text.slice(start)));
}
