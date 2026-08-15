import {
  attentionCounts,
  attentionPayload,
  groupAttention,
  relativeTime,
  type AttentionAction,
  type AttentionCard,
  type AttentionCounts,
  type AttentionGroup,
  type AttentionSeverity,
} from "./attention";
import type { AttentionItem } from "./types";

export interface AttentionPanelCallbacks {
  dismiss(items: AttentionItem[]): Promise<void> | void;
  act(item: AttentionItem, action: AttentionAction): Promise<void> | void;
  copy(payload: string): Promise<void> | void;
}

export interface AttentionPanelOptions {
  now?: number;
  animateIds?: ReadonlySet<string>;
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
  for (const severity of ["critical", "warning", "info"] as const) {
    const badge = document.createElement("span");
    badge.className = `attention-count attention-count--${severity}`;
    badge.textContent = String(counts[severity]);
    container.append(badge);
  }
  return container;
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

function renderCard(card: AttentionCard, callbacks: AttentionPanelCallbacks, options: AttentionPanelOptions): HTMLElement {
  const severity = card.content.severity;
  const article = document.createElement("article");
  article.className = `attention-item attention-item--${severity}`;
  if (severity === "critical" && options.animateIds?.has(card.latest.id)) {
    article.classList.add("attention-item--new-critical");
  }

  const eyebrow = document.createElement("div");
  eyebrow.className = "attention-eyebrow";
  const identity = document.createElement("span");
  identity.className = "attention-identity";
  identity.append(severityDot(severity));
  const session = document.createElement("span");
  session.className = "attention-session";
  session.textContent = card.latest.session ?? card.latest.machineLabel;
  identity.append(session);
  if (card.count > 1) {
    const repeated = document.createElement("span");
    repeated.className = "attention-repeat";
    repeated.textContent = `×${card.count}`;
    identity.append(repeated);
  }
  const time = document.createElement("time");
  time.dateTime = card.latest.createdAt;
  time.textContent = relativeTime(card.latest.createdAt, options.now);
  eyebrow.append(identity, time);
  if (severity !== "critical") {
    const dismiss = button("×", "attention-dismiss", () => void callbacks.dismiss(card.items));
    dismiss.setAttribute("aria-label", `Dismiss ${severity} event`);
    eyebrow.append(dismiss);
  }

  const headline = document.createElement("p");
  headline.className = "attention-title";
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
    : `${group.machineLabel}${group.session ? ` / ${group.session}` : ""}`;
  const count = document.createElement("span");
  count.className = "attention-group-count";
  count.textContent = String(group.count);
  toggle.append(label, count);
  const allItems = group.cards.flatMap((card) => card.items);
  header.append(toggle, button("Dismiss group", "attention-dismiss-group", () => void callbacks.dismiss(allItems)));
  const body = document.createElement("div");
  body.className = "attention-group-body";
  for (const card of group.cards) body.append(renderCard(card, callbacks, options));
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
    const empty = document.createElement("p");
    empty.className = "attention-empty";
    empty.textContent = "No events need triage.";
    container.append(empty);
    return;
  }
  const list = document.createElement("div");
  list.className = "attention-list";
  for (const group of groups) list.append(renderGroup(group, callbacks, options));
  container.append(list);
}
