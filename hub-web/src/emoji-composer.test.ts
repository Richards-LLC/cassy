// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { composerMarkup } from "./composer-markup";
import { bindEmojiComposer } from "./emoji-composer";
import { openEmojiPicker } from "./emoji-picker";

beforeEach(() => { document.body.replaceChildren(); localStorage.clear(); });
function composer(load: () => Promise<{ openEmojiPicker: typeof openEmojiPicker }> = vi.fn(async () => ({ openEmojiPicker }))) {
  document.body.innerHTML = composerMarkup("supervisor");
  const field = document.querySelector<HTMLTextAreaElement>("textarea")!;
  field.dataset.threadKey = "machine:one";
  const trigger = document.querySelector<HTMLButtonElement>("#message-emoji")!;
  bindEmojiComposer(field, trigger, load);
  return { field, trigger, load };
}

it("loads on demand, searches, inserts at the selection through input, and remembers the pick", async () => {
  const { field, trigger, load } = composer();
  const input = vi.fn(); field.oninput = input;
  field.value = "Hello replace world"; field.setSelectionRange(6, 13);
  expect(load).not.toHaveBeenCalled(); trigger.click();
  await vi.waitFor(() => expect(document.querySelector('[role="dialog"]')).not.toBeNull());
  const search = document.querySelector<HTMLInputElement>('[aria-label="Search emoji"]')!;
  expect(document.activeElement).toBe(search);
  search.value = "thumbs up: medium skin tone"; search.dispatchEvent(new Event("input"));
  search.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  expect(document.activeElement?.getAttribute("aria-label")).toBe("thumbs up: medium skin tone");
  (document.activeElement as HTMLButtonElement).click();
  expect(field.value).toBe("Hello 👍🏽 world");
  expect(field.selectionStart).toBe("Hello 👍🏽".length);
  expect(input).toHaveBeenCalledOnce(); expect(document.activeElement).toBe(field);
  trigger.click();
  await vi.waitFor(() => expect(document.querySelector('[aria-label="Recent emoji"] button')?.textContent).toBe("👍🏽"));
  document.querySelector('[role="dialog"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(document.querySelector('[role="dialog"]')).toBeNull(); expect(document.activeElement).toBe(trigger);
});

it("does not open a delayed picker for a different conversation", async () => {
  let resolve!: (module: { openEmojiPicker: typeof openEmojiPicker }) => void;
  const { field, trigger } = composer(vi.fn(() => new Promise<{ openEmojiPicker: typeof openEmojiPicker }>(r => { resolve = r; })));
  trigger.click(); field.dataset.threadKey = "machine:two"; resolve({ openEmojiPicker });
  await Promise.resolve(); await Promise.resolve();
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(trigger.getAttribute("aria-busy")).not.toBe("true");
});

it("offers retry after a load failure and works when recent storage is blocked", async () => {
  const load = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue({ openEmojiPicker });
  const { trigger } = composer(load);
  trigger.click(); await vi.waitFor(() => expect(document.querySelector('#message-status')?.textContent).toContain("try again"));
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
  trigger.click(); await vi.waitFor(() => expect(document.querySelector('[role="dialog"]')).not.toBeNull());
  document.querySelector<HTMLButtonElement>('[aria-label="Emoji results"] button')!.click();
  expect(document.querySelector('textarea')!.value).not.toBe("");
  vi.restoreAllMocks();
});
