import { emojiData } from "./emoji-data";

const RECENTS_KEY = "cassy.emoji.recents";
const RECENTS_LIMIT = 12;
const PAGE_SIZE = 96;
const COLUMNS = 6;
type Emoji = typeof emojiData[number];
let recentMemory: string[] = [];
let nextId = 0;
const known = new Map(emojiData.map(row => [row[0], row]));
const searchable = emojiData.map(row => ({ row, text: `${row[1]} ${row[2]} ${row[0]}`.toLocaleLowerCase("en") }));
function recents(): Emoji[] {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(RECENTS_KEY) ?? "[]");
    if (Array.isArray(saved)) recentMemory = [...new Set([...recentMemory, ...saved.filter((x): x is string => typeof x === "string" && known.has(x))])].slice(0, RECENTS_LIMIT);
  } catch { /* Private browsing and full storage keep in-page recents. */ }
  return recentMemory.flatMap(value => known.has(value) ? [known.get(value)!] : []);
}

/** One non-modal tray: normal Tab order, roving results, Escape and outside dismissal. */
export function openEmojiPicker(trigger: HTMLButtonElement, pick: (emoji: string) => void, closed: () => void): () => void {
  const doc = trigger.ownerDocument;
  const win = doc.defaultView!;
  const tray = doc.createElement("section");
  tray.id = `emoji-picker-${++nextId}`;
  tray.className = "emoji-picker";
  tray.setAttribute("role", "dialog"); tray.setAttribute("aria-label", "Insert emoji");
  tray.innerHTML = '<header><strong>Emoji</strong><button type="button" aria-label="Close emoji picker">×</button></header><label class="sr-only" for="emoji-search">Search emoji</label><input id="emoji-search" type="search" placeholder="Search emoji" aria-label="Search emoji" autocomplete="off"><p class="emoji-count" role="status" aria-live="polite"></p><div class="emoji-scroll"><div class="emoji-recent"></div><div class="emoji-grid" role="group" aria-label="Emoji results"></div><button class="emoji-more" type="button">Show more emoji</button></div>';
  const search = tray.querySelector<HTMLInputElement>("input")!;
  search.id = `${tray.id}-search`; tray.querySelector("label")!.htmlFor = search.id;
  const grid = tray.querySelector<HTMLElement>('.emoji-grid')!;
  const recent = tray.querySelector<HTMLElement>('.emoji-recent')!;
  const count = tray.querySelector<HTMLElement>('.emoji-count')!;
  const more = tray.querySelector<HTMLButtonElement>('.emoji-more')!;
  const scroll = tray.querySelector<HTMLElement>('.emoji-scroll')!;
  let matches = emojiData;
  let shown = PAGE_SIZE;
  let disposed = false;
  const close = () => {
    if (disposed) return;
    disposed = true; tray.remove();
    doc.removeEventListener("pointerdown", outside);
    doc.removeEventListener("focusin", focusOutside);
    win.removeEventListener("resize", position);
    win.removeEventListener("scroll", position, true);
    win.visualViewport?.removeEventListener("resize", position);
    win.visualViewport?.removeEventListener("scroll", position);
    closed();
  };
  const returnFocus = () => { close(); if (trigger.isConnected) trigger.focus({ preventScroll: true }); };
  const outside = (event: Event) => { if (!tray.contains(event.target as Node) && !trigger.contains(event.target as Node)) close(); };
  const focusOutside = (event: Event) => { if (!tray.contains(event.target as Node) && event.target !== trigger) close(); };
  const button = (row: Emoji) => {
    const node = doc.createElement("button"); node.type = "button"; node.className = "emoji-choice emoji-text";
    node.textContent = row[0]; node.setAttribute("aria-label", row[1]); node.title = row[1]; node.tabIndex = -1;
    node.onclick = () => {
      recentMemory = [row[0], ...recentMemory.filter(value => value !== row[0])].slice(0, RECENTS_LIMIT);
      try { localStorage.setItem(RECENTS_KEY, JSON.stringify(recentMemory)); } catch { /* The pick still works. */ }
      // Remove the focusout listener before returning the caret to the composer.
      close(); pick(row[0]);
    };
    return node;
  };
  const paint = () => {
    const isSearch = search.value.trim() !== "";
    recent.replaceChildren();
    const saved = isSearch ? [] : recents();
    if (saved.length) {
      const title = doc.createElement("p"); title.textContent = "Recent";
      const group = doc.createElement("div"); group.className = "emoji-grid"; group.setAttribute("role", "group"); group.setAttribute("aria-label", "Recent emoji");
      group.append(...saved.map(button)); recent.append(title, group);
      group.querySelector<HTMLButtonElement>("button")!.tabIndex = 0;
    }
    grid.replaceChildren(...matches.slice(0, shown).map(button));
    const first = grid.querySelector<HTMLButtonElement>("button"); if (first) first.tabIndex = 0;
    more.hidden = shown >= matches.length;
    count.textContent = matches.length ? `${matches.length} emoji${more.hidden ? "" : ` · showing ${Math.min(shown, matches.length)}`}` : "No emoji found. Try another search.";
  };
  search.oninput = () => {
    const words = search.value.toLocaleLowerCase("en").trim().split(/\s+/).filter(Boolean);
    matches = words.length ? searchable.filter(entry => words.every(word => entry.text.includes(word))).map(entry => entry.row) : emojiData;
    if (words.length) matches = [...matches].sort((a, b) => Number(b[1] === search.value.toLocaleLowerCase("en").trim()) - Number(a[1] === search.value.toLocaleLowerCase("en").trim()));
    shown = PAGE_SIZE; scroll.scrollTop = 0; paint();
  };
  search.onkeydown = event => {
    if (event.isComposing || !["ArrowDown", "ArrowUp", "Enter"].includes(event.key)) return;
    const buttons = grid.querySelectorAll<HTMLButtonElement>("button");
    if (!buttons.length) return;
    event.preventDefault();
    const first = buttons[event.key === "ArrowUp" ? buttons.length - 1 : 0]!;
    if (event.key === "Enter") first.click(); else first.focus();
  };
  more.onclick = () => {
    const previous = shown; shown += PAGE_SIZE; paint();
    grid.querySelectorAll<HTMLButtonElement>("button")[previous]?.focus();
  };
  tray.addEventListener("focusin", event => {
    const target = event.target as HTMLButtonElement;
    if (!target.classList.contains("emoji-choice")) return;
    for (const node of target.parentElement!.querySelectorAll<HTMLButtonElement>("button")) node.tabIndex = node === target ? 0 : -1;
  });
  tray.onkeydown = event => {
    if (event.defaultPrevented) return;
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); returnFocus(); return; }
    const active = event.target as HTMLButtonElement | null;
    const group = active?.closest('.emoji-grid');
    if (!group || !active) return;
    const buttons = [...group.querySelectorAll<HTMLButtonElement>("button")];
    const index = buttons.indexOf(active);
    const offset: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -COLUMNS, ArrowDown: COLUMNS };
    const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : event.key in offset ? Math.max(0, Math.min(buttons.length - 1, index + offset[event.key]!)) : undefined;
    if (next === undefined) return;
    event.preventDefault(); buttons.forEach(node => { node.tabIndex = -1; });
    buttons[next]!.tabIndex = 0; buttons[next]!.focus();
  };
  tray.querySelector<HTMLButtonElement>('header button')!.onclick = returnFocus;
  function position(): void {
    const viewport = win.visualViewport;
    const left = viewport?.offsetLeft ?? 0; const top = viewport?.offsetTop ?? 0;
    const width = viewport?.width ?? win.innerWidth; const height = viewport?.height ?? win.innerHeight;
    const anchor = (trigger.closest(".message")?.querySelector("textarea") ?? trigger).getBoundingClientRect();
    tray.style.width = `${Math.min(320, Math.max(0, width - 16))}px`;
    const above = anchor.top - top - 16;
    tray.style.maxHeight = `${Math.max(0, Math.min(height - 16, above >= 180 ? above : height - 16))}px`;
    const rect = tray.getBoundingClientRect();
    tray.style.left = `${Math.max(left + 8, Math.min(anchor.right - rect.width, left + width - rect.width - 8))}px`;
    tray.style.top = `${Math.max(top + 8, Math.min(anchor.top - rect.height - 8, top + height - rect.height - 8))}px`;
  }
  paint(); doc.body.append(tray); trigger.setAttribute("aria-controls", tray.id); position();
  doc.addEventListener("pointerdown", outside); doc.addEventListener("focusin", focusOutside);
  win.addEventListener("resize", position); win.addEventListener("scroll", position, true);
  win.visualViewport?.addEventListener("resize", position); win.visualViewport?.addEventListener("scroll", position);
  search.focus({ preventScroll: true });
  return close;
}
