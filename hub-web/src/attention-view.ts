import {
  attentionCounts,
  attentionPayload,
  attentionSummary,
  groupAttention,
  relativeTime,
  type AttentionAction,
  type AttentionCard,
  type AttentionCounts,
  type AttentionGroup,
  type AttentionSeverity,
} from "./attention";
import type { AttentionItem } from "./types";
import { absoluteTimestamp } from "./time";

export interface AttentionPanelCallbacks {
  dismiss(items: AttentionItem[]): Promise<void> | void;
  act(item: AttentionItem, action: AttentionAction): Promise<void> | void;
  copy(payload: string): Promise<void> | void;
}

export interface AttentionPanelOptions {
  now?: number;
  animateIds?: ReadonlySet<string>;
  reclassifyIds?: ReadonlySet<string>;
}

const ACTION_LABEL: Record<Exclude<AttentionAction, "none">, string> = {
  repair: "Re-pair",
  view_pane: "View pane",
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

export function renderAttentionCounts(counts: AttentionCounts, compact = false): HTMLSpanElement {
  const container = document.createElement("span");
  container.className = `attention-counts${compact ? " attention-counts--compact" : ""}`;
  container.setAttribute("aria-label", `${counts.critical} critical, ${counts.warning} warning, ${counts.info} info`);
  // Zeroes are not news. Only outstanding severities get a badge, so the rail
  // reads as a count instead of a row of noughts.
  for (const severity of ["critical", "warning", "info"] as const) {
    if (counts[severity] === 0) continue;
    const badge = document.createElement("span");
    badge.className = `attention-count attention-count--${severity}`;
    badge.textContent = String(counts[severity]);
    container.append(badge);
  }
  if (!container.hasChildNodes()) {
    const clear = document.createElement("span");
    clear.className = "attention-count attention-count--clear";
    clear.textContent = "0";
    container.append(clear);
  }
  return container;
}

/**
 * The phone rail's single badge: a severity dot and one labelled figure. The
 * desktop rail keeps the per-severity column; both live in the same button and
 * the compact block chooses between them, so no media query reaches JavaScript.
 */
export function renderAttentionSummary(counts: AttentionCounts): HTMLSpanElement {
  const summary = attentionSummary(counts);
  const container = document.createElement("span");
  container.className = `attention-summary attention-summary--${summary.severity}`;
  // The button carries the accessible name; these are the visual form of it.
  container.setAttribute("aria-hidden", "true");
  const dot = document.createElement("span");
  dot.className = `attention-dot attention-dot--${summary.severity}`;
  const label = document.createElement("span");
  label.className = "attention-summary-label";
  label.textContent = summary.label;
  container.append(dot, label);
  return container;
}

export function cycleAttentionGroup(container: HTMLElement, direction: number): HTMLButtonElement | undefined {
  const groups = [...container.querySelectorAll<HTMLButtonElement>(".attention-group-toggle")];
  if (groups.length === 0) return undefined;
  const activeIndex = groups.indexOf(container.ownerDocument.activeElement as HTMLButtonElement);
  const nextIndex = activeIndex < 0
    ? direction < 0 ? groups.length - 1 : 0
    : (activeIndex + (direction < 0 ? -1 : 1) + groups.length) % groups.length;
  const next = groups[nextIndex];
  next.focus();
  next.scrollIntoView({ block: "nearest" });
  return next;
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
  const body = document.createElement("div");
  body.className = "attention-payload-body";
  const payload = attentionPayload(card.latest);
  const pre = document.createElement("pre");
  pre.textContent = payload;
  body.append(button("Copy", "attention-copy", () => void callbacks.copy(payload)), pre);
  details.append(summary, body);
  return details;
}

function renderCard(card: AttentionCard, callbacks: AttentionPanelCallbacks, options: AttentionPanelOptions, groupLabel?: string): HTMLElement {
  const severity = card.content.severity;
  const article = document.createElement("article");
  article.className = `attention-item attention-item--${severity}`;
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
  const owner = card.latest.session ?? card.latest.machineLabel;
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
  const time = document.createElement("time");
  time.dateTime = card.latest.createdAt;
  time.textContent = relativeTime(card.latest.createdAt, options.now);
  time.title = absoluteTimestamp(card.latest.createdAt);
  eyebrow.append(identity, time);
  if (severity !== "critical") {
    const dismiss = button("×", "attention-dismiss", () => void callbacks.dismiss(card.items));
    dismiss.setAttribute("aria-label", `Dismiss ${severity} event`);
    eyebrow.append(dismiss);
  }

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
  if (card.content.action !== "none") {
    const action = card.content.action;
    actions.append(button(ACTION_LABEL[action], "attention-action", () => {
      void Promise.resolve(callbacks.act(card.latest, action)).then(() => {
        if (severity === "critical") return callbacks.dismiss(card.items);
      });
    }));
  }
  if (severity === "critical") {
    actions.append(button("Dismiss", "attention-explicit-dismiss", () => void callbacks.dismiss(card.items)));
  }
  actions.append(renderPayload(card, callbacks));
  article.append(actions);
  return article;
}

function renderGroup(group: AttentionGroup, callbacks: AttentionPanelCallbacks, options: AttentionPanelOptions): HTMLElement {
  const section = document.createElement("section");
  section.className = `attention-group${group.overflow ? " attention-group--overflow" : ""}`;
  const header = document.createElement("header");
  header.className = "attention-group-header";
  const toggle = button("", "attention-group-toggle", () => {
    const expanded = toggle.getAttribute("aria-expanded") === "true";
    toggle.setAttribute("aria-expanded", String(!expanded));
    body.hidden = expanded;
  });
  toggle.setAttribute("aria-expanded", "true");
  toggle.append(severityDot(group.worstSeverity));
  const label = document.createElement("span");
  label.className = "attention-group-label";
  label.textContent = group.overflow
    ? group.machineLabel
    : group.session ?? group.machineLabel;
  const count = document.createElement("span");
  count.className = "attention-group-count";
  count.textContent = String(group.count);
  toggle.append(label, count);
  const allItems = group.cards.flatMap((card) => card.items);
  header.append(toggle, button("Dismiss group", "attention-dismiss-group", () => void callbacks.dismiss(allItems)));
  const body = document.createElement("div");
  body.className = "attention-group-body";
  const groupLabel = group.overflow ? group.machineLabel : group.session ?? group.machineLabel;
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
  container.replaceChildren();
  const counts = attentionCounts(items);
  const header = document.createElement("header");
  header.className = "attention-panel-header";
  const heading = document.createElement("h2");
  heading.textContent = "Attention";
  heading.append(renderAttentionCounts(counts));
  header.append(heading);
  const infoItems = items.filter((item) => !item.acknowledgedAt && attentionCounts([item]).info === 1);
  if (infoItems.length > 0) {
    header.append(button("Dismiss all info", "attention-dismiss-info", () => void callbacks.dismiss(infoItems)));
  }
  container.append(header);

  const groups = groupAttention(items);
  if (groups.length === 0) {
    const empty = document.createElement("div");
    empty.className = "attention-empty";
    const message = document.createElement("p");
    message.textContent = "All clear";
    const latest = items.toSorted((a, b) => b.createdAt.localeCompare(a.createdAt))[0];
    const timestamp = document.createElement("time");
    timestamp.className = "attention-last-event";
    if (latest) {
      timestamp.dateTime = latest.createdAt;
      timestamp.textContent = `Last event ${new Date(latest.createdAt).toLocaleString()}`;
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
