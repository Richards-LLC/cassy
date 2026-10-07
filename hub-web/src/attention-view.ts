import {
  attentionCounts,
  attentionPayload,
  groupAttention,
  attentionTimeLabel,
  type AttentionAction,
  type AttentionCard,
  type AttentionGroup,
  type AttentionSeverity,
} from "./attention";
import type { AttentionItem } from "./types";
import { absoluteTimestamp } from "./time";
import { stampLabel } from "./thread-model";

export interface AttentionPanelCallbacks {
  dismiss(items: AttentionItem[]): Promise<void> | void;
  act(item: AttentionItem, action: AttentionAction): Promise<void> | void;
  copy(payload: string): Promise<void> | void;
}

export interface AttentionPanelOptions {
  now?: number;
  /** Catalog project and supervisor names, independent of the durable session id. */
  sessionLabel?: (item: Pick<AttentionItem, "machineId" | "session">) => string | undefined;
  /**
   * A connection the rail covers is down ("Atlas · Linux is reconnecting").
   * With no events, the empty state says this instead of "All clear": the
   * banner beside it is saying the same thing (cas-edcd).
   */
  outage?: string;
  animateIds?: ReadonlySet<string>;
  reclassifyIds?: ReadonlySet<string>;
  /**
   * cas-d043 G12: the conversation on screen. "Open conversation" on an item
   * about it would reopen what is already open and change nothing, so the
   * item offers only Dismiss.
   */
  openConversation?: { readonly machineId: string; readonly session?: string };
}

/** The item's action, less one that would only reopen the conversation already open (cas-d043 G12). */
export function effectiveAttentionAction(item: Pick<AttentionItem, "machineId" | "session">, action: AttentionAction, open?: AttentionPanelOptions["openConversation"]): AttentionAction {
  if (action !== "view_pane" || !open || !item.session) return action;
  return item.machineId === open.machineId && item.session === open.session ? "none" : action;
}

const ACTION_LABEL: Record<Exclude<AttentionAction, "none">, string> = {
  repair: "Re-pair",
  // cas-97d58 F06: the Terminal view is gone; this opens the conversation.
  view_pane: "Open conversation",
  retry: "Retry",
  open_pr: "Open PR",
};

function button(label: string, className: string, onClick: () => void): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.className = className;
  element.textContent = label;
  element.onclick = (event) => {
    event.stopPropagation();
    onClick();
  };
  return element;
}

function severityDot(severity: AttentionSeverity): HTMLSpanElement {
  const dot = document.createElement("span");
  dot.className = `attention-dot attention-dot--${severity}`;
  dot.setAttribute("aria-label", severity);
  return dot;
}

function cardDetail(card: AttentionCard): string | undefined {
  if (card.content.cause && card.content.detail) return `${card.content.cause} · ${card.content.detail}`;
  return card.content.cause ?? card.content.detail;
}

function renderPayload(card: AttentionCard, callbacks: AttentionPanelCallbacks): HTMLDetailsElement {
  const details = document.createElement("details");
  details.className = "attention-payload";
  const summary = document.createElement("summary");
  summary.textContent = "Details";
  summary.dataset.role = "details";
  const body = document.createElement("div");
  body.className = "attention-payload-body";
  const payload = attentionPayload(card.latest);
  const pre = document.createElement("pre");
  pre.textContent = payload;
  const copy = button("Copy", "attention-copy", () => void callbacks.copy(payload));
  copy.dataset.role = "copy";
  body.append(copy, pre);
  details.append(summary, body);
  return details;
}

function ownerLabel(item: AttentionItem, options: AttentionPanelOptions): string {
  if (!item.session) return item.machineLabel;
  return options.sessionLabel?.(item)
    ?? `${item.machineLabel} · ${/([a-z]+-[a-z]+-\d+)$/i.exec(item.session)?.[1] ?? "Session unavailable"}`;
}

function renderCard(card: AttentionCard, callbacks: AttentionPanelCallbacks, options: AttentionPanelOptions, groupLabel?: string): HTMLElement {
  const severity = card.content.severity;
  const article = document.createElement("article");
  article.className = `attention-item attention-item--${severity}`;
  // cas-a5c6 QA round 3: a stable identity for this notice, so focus and an
  // opened Details follow the notice across redraws, never its position.
  article.dataset.attentionId = card.key;
  if (card.content.enrichmentPending) article.classList.add("attention-item--enriching");
  if (severity === "critical" && options.animateIds?.has(card.latest.id)) {
    article.classList.add("attention-item--new-critical");
  }
  if (options.reclassifyIds?.has(card.latest.id)) article.classList.add("attention-item--reclassified");

  const eyebrow = document.createElement("div");
  eyebrow.className = "attention-eyebrow";
  const identity = document.createElement("span");
  identity.className = "attention-identity";
  identity.append(severityDot(severity));
  // The group header already names the session; repeating it on every card
  // in that group spent the eyebrow's width on the one thing it did not need
  // to say. A card grouped under another label still states its own.
  const owner = ownerLabel(card.latest, options);
  if (owner !== groupLabel) {
    const session = document.createElement("span");
    session.className = "attention-session";
    session.textContent = owner;
    identity.append(session);
  }
  if (card.count > 1) {
    const repeated = document.createElement("span");
    repeated.className = "attention-repeat";
    repeated.textContent = `×${card.count}`;
    identity.append(repeated);
  }
  // A card raised from a task carries its ticket; naming it saves the operator
  // from opening the card to find out which task it is about.
  if (card.content.ticketId) {
    const ticket = document.createElement("span");
    ticket.className = "attention-ticket";
    ticket.textContent = card.content.ticketId;
    identity.append(ticket);
  }
  if (card.content.enrichmentPending) {
    const pending = document.createElement("span");
    pending.className = "attention-enriching";
    pending.textContent = "Enriching…";
    identity.append(pending);
  }
  const time = document.createElement("time");
  time.dateTime = card.latest.createdAt;
  time.textContent = attentionTimeLabel(card.latest.createdAt, options.now);
  time.title = absoluteTimestamp(card.latest.createdAt);
  time.className = "attention-time";
  eyebrow.append(time, identity);

  const headline = document.createElement("p");
  headline.className = "attention-title";
  if (card.content.enrichmentPending) headline.setAttribute("aria-label", "Summary pending AI enrichment");
  headline.textContent = card.content.headline;
  article.append(eyebrow, headline);

  const detail = cardDetail(card);
  if (detail) {
    const detailLine = document.createElement("p");
    detailLine.className = "attention-detail";
    detailLine.textContent = detail;
    article.append(detailLine);
  }

  const actions = document.createElement("div");
  actions.className = "attention-actions";
  const action = effectiveAttentionAction(card.latest, card.content.action, options.openConversation);
  if (action !== "none") {
    const act = button(ACTION_LABEL[action], "attention-action", () => {
      void Promise.resolve(callbacks.act(card.latest, action)).then(() => {
        if (severity === "critical") return callbacks.dismiss(card.items);
      });
    });
    act.dataset.role = "action";
    actions.append(act);
  }
  const dismiss = button("Dismiss", severity !== "critical" ? "attention-dismiss" : "attention-explicit-dismiss", () => void callbacks.dismiss(card.items));
  dismiss.setAttribute("aria-label", `Dismiss ${severity} event`);
  dismiss.dataset.role = "dismiss";
  if (action === "none") dismiss.classList.add("attention-action");
  actions.append(dismiss);
  actions.append(renderPayload(card, callbacks));
  article.append(actions);
  return article;
}

function renderGroup(group: AttentionGroup, callbacks: AttentionPanelCallbacks, options: AttentionPanelOptions): HTMLElement {
  const section = document.createElement("section");
  section.className = `attention-group${group.overflow ? " attention-group--overflow" : ""}`;
  section.dataset.groupKey = group.key;
  const header = document.createElement("header");
  header.className = "attention-group-header";
  const toggle = button("", "attention-group-toggle", () => {
    const expanded = toggle.getAttribute("aria-expanded") === "true";
    toggle.setAttribute("aria-expanded", String(!expanded));
    body.hidden = expanded;
  });
  toggle.setAttribute("aria-expanded", "true");
  toggle.dataset.role = "group-toggle";
  toggle.append(severityDot(group.worstSeverity));
  const label = document.createElement("span");
  label.className = "attention-group-label";
  const groupLabel = group.overflow ? group.machineLabel : ownerLabel(group.cards[0]!.latest, options);
  label.textContent = groupLabel;
  const count = document.createElement("span");
  count.className = "attention-group-count";
  count.textContent = String(group.count);
  toggle.append(label, count);
  const allItems = group.cards.flatMap((card) => card.items);
  const dismissGroup = button("Dismiss group", "attention-dismiss-group", () => void callbacks.dismiss(allItems));
  dismissGroup.dataset.role = "group-dismiss";
  header.append(toggle);
  if (group.count > 1) header.append(dismissGroup);
  const body = document.createElement("div");
  body.className = "attention-group-body";
  for (const card of group.cards) body.append(renderCard(card, callbacks, options, groupLabel));
  section.append(header, body);
  return section;
}

export function renderAttentionPanel(
  container: HTMLElement,
  items: readonly AttentionItem[],
  callbacks: AttentionPanelCallbacks,
  options: AttentionPanelOptions = {},
): void {
  // cas-a5c6 QA F03 (rounds 2 and 3): a heartbeat redraw with nothing new
  // rebuilt the panel every 5 s, and the minute's age change rebuilt it once a
  // minute, taking the keyboard user's focus and any opened Details with it.
  // Unchanged content is left alone; only the cards' ages are refreshed, in
  // place.
  const now = options.now ?? Date.now();
  const signature = JSON.stringify([
    items,
    items.map((item) => ownerLabel(item, options)),
    options.outage ?? null,
    options.openConversation ?? null,
    [...(options.animateIds ?? [])].filter((id) => items.some((item) => item.id === id)),
    [...(options.reclassifyIds ?? [])].filter((id) => items.some((item) => item.id === id)),
  ]);
  if (container.dataset.panelSignature === signature && container.childElementCount > 0) {
    for (const time of container.querySelectorAll<HTMLTimeElement>("time.attention-time")) {
      const label = attentionTimeLabel(time.dateTime, now);
      if (time.textContent !== label) time.textContent = label;
    }
    // The empty rail's last-event time names its day once it is no longer
    // today, as thread times do, so it moves on in place too (cas-0cd1).
    for (const time of container.querySelectorAll<HTMLTimeElement>("time.attention-last-event[datetime]")) {
      const label = lastEventLabel(time.dateTime, now);
      if (time.textContent !== label) time.textContent = label;
    }
    return;
  }
  container.dataset.panelSignature = signature;
  // A real change redraws; what the operator had open, folded and focused is
  // carried over by each notice's and group's own key, never by position.
  // A panel the page has just rebuilt from scratch (a shell rebuild after a
  // phone wakes or a throttled tab catches up, cas-f486) has nothing in it
  // to read, so it takes what the panel it replaced last held.
  const memory = container.id || "attention-panel";
  const kept = container.childElementCount > 0
    ? attentionPanelState(container)
    : rememberedPanelState.get(memory) ?? { open: new Set<string>(), folded: new Set<string>() };
  // Panel evidence (outage, owner, grouping) can change while the same notice
  // stays open. Keep its Details/Copy nodes if their copied payload is unchanged.
  const payloads = new Map<string, HTMLDetailsElement>();
  for (const article of container.querySelectorAll<HTMLElement>("[data-attention-id]")) {
    const details = article.querySelector<HTMLDetailsElement>("details.attention-payload");
    if (details) payloads.set(article.dataset.attentionId!, details);
  }
  renderAttentionPanelContent(container, items, callbacks, options);
  for (const article of container.querySelectorAll<HTMLElement>("[data-attention-id]")) {
    const prior = payloads.get(article.dataset.attentionId!);
    const next = article.querySelector<HTMLDetailsElement>("details.attention-payload");
    if (prior && next && prior.querySelector("pre")?.textContent === next.querySelector("pre")?.textContent) {
      const copy = prior.querySelector<HTMLButtonElement>(".attention-copy");
      const nextCopy = next.querySelector<HTMLButtonElement>(".attention-copy");
      if (copy && nextCopy) copy.onclick = nextCopy.onclick;
      next.replaceWith(prior);
    }
  }
  restoreAttentionPanelState(container, kept);
  watchAttentionPanelState(container, memory);
  rememberedPanelState.set(memory, attentionPanelState(container));
}

/**
 * What each Attention panel last held (cas-f486), kept beside the page, not
 * in it: the page can replace the panel element wholesale, and the operator's
 * opened Details, folded groups and focus must outlive that.
 */
const rememberedPanelState = new Map<string, AttentionPanelState>();

/** Keep the remembered state current as the operator opens, folds and moves focus. */
function watchAttentionPanelState(container: HTMLElement, memory: string): void {
  if (container.dataset.stateWatched === "true") return;
  container.dataset.stateWatched = "true";
  const save = () => { if (container.isConnected) rememberedPanelState.set(memory, attentionPanelState(container)); };
  container.addEventListener("toggle", save, true);
  container.addEventListener("focusin", save);
  container.addEventListener("click", () => queueMicrotask(save));
  // Focus the operator moves elsewhere on the page is theirs: the panel stops
  // claiming it. (A panel removed with focus inside fires no focusin, so its
  // focus is still remembered for its replacement.)
  const document = container.ownerDocument;
  if (!watchedDocuments.has(document)) {
    watchedDocuments.add(document);
    document.addEventListener("focusin", (event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      for (const [key, state] of rememberedPanelState) {
        const panel = document.getElementById(key);
        if (state.focus !== undefined && !(panel?.contains(target))) rememberedPanelState.set(key, { ...state, focus: undefined });
      }
    });
  }
}
const watchedDocuments = new WeakSet<Document>();

/** Opened Details, folded groups and the focused control, keyed by notice or group and role. */
interface AttentionPanelState { open: Set<string>; folded: Set<string>; focus?: string }

/** The key of a panel control: its notice (or group, or the panel) and its role (cas-a5c6). */
export function attentionControlKey(node: Element): string | undefined {
  if (!(node instanceof HTMLElement) || !node.dataset.role) return undefined;
  const notice = node.closest<HTMLElement>("[data-attention-id]")?.dataset.attentionId;
  const group = node.closest<HTMLElement>("[data-group-key]")?.dataset.groupKey;
  const scope = notice !== undefined ? `notice:${notice}` : group !== undefined ? `group:${group}` : "panel";
  return `${scope}|${node.dataset.role}`;
}

/** The control in `root` with this key, if it is there. */
export function findAttentionControl(root: ParentNode, key: string): HTMLElement | undefined {
  return [...root.querySelectorAll<HTMLElement>("[data-role]")].find((node) => attentionControlKey(node) === key);
}

function attentionPanelState(container: HTMLElement): AttentionPanelState {
  const open = new Set<string>();
  for (const details of container.querySelectorAll<HTMLDetailsElement>("[data-attention-id] details[open]")) {
    const id = details.closest<HTMLElement>("[data-attention-id]")?.dataset.attentionId;
    if (id !== undefined) open.add(id);
  }
  const folded = new Set<string>();
  for (const toggle of container.querySelectorAll<HTMLElement>("[data-group-key] [data-role='group-toggle'][aria-expanded='false']")) {
    const key = toggle.closest<HTMLElement>("[data-group-key]")?.dataset.groupKey;
    if (key !== undefined) folded.add(key);
  }
  const active = container.ownerDocument.activeElement;
  const focus = active && container.contains(active) ? attentionControlKey(active) : undefined;
  return { open, folded, focus };
}

function restoreAttentionPanelState(container: HTMLElement, state: AttentionPanelState): void {
  for (const article of container.querySelectorAll<HTMLElement>("[data-attention-id]")) {
    if (!state.open.has(article.dataset.attentionId!)) continue;
    const details = article.querySelector<HTMLDetailsElement>("details");
    if (details) details.open = true;
  }
  for (const section of container.querySelectorAll<HTMLElement>("[data-group-key]")) {
    if (!state.folded.has(section.dataset.groupKey!)) continue;
    const toggle = section.querySelector<HTMLElement>("[data-role='group-toggle']");
    const body = section.querySelector<HTMLElement>(".attention-group-body");
    toggle?.setAttribute("aria-expanded", "false");
    if (body) body.hidden = true;
  }
  if (state.focus === undefined) return;
  const document = container.ownerDocument;
  if (document.activeElement && document.activeElement !== document.body && container.contains(document.activeElement)) return;
  // The same control of the same notice, or nothing: never a neighbour's
  // look-alike, which could be another notice's Dismiss (QA round 3 F01).
  const key = state.focus;
  const land = () => {
    const lost = !document.activeElement || document.activeElement === document.body;
    if (lost && container.isConnected) findAttentionControl(container, key)?.focus({ preventScroll: true });
  };
  land();
  // A rebuilt page can still be hiding the panel's column at this point (the
  // phone sheet is reopened after the panel is drawn, cas-f486): try again
  // once the frame is laid out.
  if (document.activeElement === document.body) document.defaultView?.requestAnimationFrame(land);
}

/**
 * The empty rail's last event in the app's clock (cas-0cd1): "Last event
 * 12:01" today, "Last event Sep 30, 12:01" on another day, the same as a
 * thread time. The browser's locale format ("9/30/2026, 12:01:32 PM") read as
 * a different clock from everything beside it.
 */
function lastEventLabel(createdAt: string, now: number): string {
  const at = Date.parse(createdAt);
  return Number.isFinite(at) ? `Last event ${stampLabel(at, now)}` : `Last event ${createdAt}`;
}

function renderAttentionPanelContent(
  container: HTMLElement,
  items: readonly AttentionItem[],
  callbacks: AttentionPanelCallbacks,
  options: AttentionPanelOptions,
): void {
  container.replaceChildren();
  const counts = attentionCounts(items);
  const header = document.createElement("header");
  header.className = "attention-panel-header";
  const heading = document.createElement("h2");
  heading.textContent = "Attention";
  const summary = document.createElement("p");
  summary.className = "attention-panel-summary";
  const total = counts.critical + counts.warning + counts.info;
  summary.textContent = `${total} event${total === 1 ? " needs" : "s need"} attention`;
  summary.hidden = total === 0;
  header.append(heading);
  const infoItems = items.filter((item) => !item.acknowledgedAt && attentionCounts([item]).info === 1);
  if (infoItems.length > 0) {
    const dismissInfo = button("Dismiss all info", "attention-dismiss-info", () => void callbacks.dismiss(infoItems));
    dismissInfo.dataset.role = "dismiss-info";
    header.append(dismissInfo);
  }
  container.append(header, summary);

  const groups = groupAttention(items);
  if (groups.length === 0) {
    const empty = document.createElement("div");
    empty.className = options.outage ? "attention-empty outage" : "attention-empty";
    const message = document.createElement("p");
    message.textContent = options.outage ?? "All clear";
    const latest = items.toSorted((a, b) => b.createdAt.localeCompare(a.createdAt))[0];
    const timestamp = document.createElement("time");
    timestamp.className = "attention-last-event";
    if (latest) {
      timestamp.dateTime = latest.createdAt;
      timestamp.textContent = lastEventLabel(latest.createdAt, options.now ?? Date.now());
    } else {
      timestamp.textContent = "No events recorded yet";
    }
    empty.append(message, timestamp);
    container.append(empty);
    return;
  }
  const list = document.createElement("div");
  list.className = "attention-list";
  for (const group of groups) list.append(renderGroup(group, callbacks, options));
  container.append(list);
}
