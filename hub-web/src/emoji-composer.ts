import type { openEmojiPicker } from "./emoji-picker";

type PickerModule = { openEmojiPicker: typeof openEmojiPicker };
const bound = new WeakSet<HTMLButtonElement>();

/** Keep the dataset off the initial route; the ordinary input handler owns the draft. */
export function bindEmojiComposer(field: HTMLTextAreaElement, trigger: HTMLButtonElement, load: () => Promise<PickerModule> = () => import("./emoji-picker")): void {
  if (bound.has(trigger)) return;
  bound.add(trigger);
  let dismiss: (() => void) | undefined;
  let opening = false;
  let generation = 0;
  trigger.onclick = async () => {
    if (dismiss) { dismiss(); return; }
    if (opening) { generation++; opening = false; trigger.removeAttribute("aria-busy"); return; }
    const attempt = ++generation;
    const thread = field.dataset.threadKey;
    const start = field.selectionStart;
    const end = field.selectionEnd;
    const original = field.value;
    const current = () => field.isConnected && trigger.isConnected && field.dataset.threadKey === thread;
    opening = true;
    trigger.setAttribute("aria-busy", "true");
    try {
      const module = await load();
      if (attempt !== generation || !current()) return;
      // A draft may have changed during the import (typing or dictation). Do not
      // replace a stale selection in that newer text.
      if (field.value !== original) return;
      const previousError = field.closest(".message")?.querySelector<HTMLElement>("#message-status");
      if (previousError?.textContent === "Couldn't open emoji. Choose Insert emoji to try again.") { previousError.hidden = true; previousError.textContent = ""; }
      const observer = new MutationObserver(() => { if (!current()) dismiss?.(); });
      const closed = () => {
        observer.disconnect(); dismiss = undefined;
        trigger.setAttribute("aria-expanded", "false"); trigger.removeAttribute("aria-controls");
      };
      dismiss = module.openEmojiPicker(trigger, emoji => {
        if (!current() || field.value !== original) { dismiss?.(); return; }
        field.setRangeText(emoji, start, end, "end");
        field.dispatchEvent(new Event("input", { bubbles: true }));
        field.focus({ preventScroll: true });
      }, closed);
      observer.observe(field.ownerDocument.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-thread-key"] });
      trigger.setAttribute("aria-expanded", "true");
    } catch {
      if (attempt !== generation || !current()) return;
      const status = field.closest(".message")?.querySelector<HTMLElement>("#message-status");
      if (status) { status.hidden = false; status.textContent = "Couldn't open emoji. Choose Insert emoji to try again."; }
    } finally {
      if (attempt === generation) { opening = false; trigger.removeAttribute("aria-busy"); }
    }
  };
}
