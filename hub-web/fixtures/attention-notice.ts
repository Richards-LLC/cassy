import { createAttentionItem } from "../src/attention";
import { renderAttentionPanel } from "../src/attention-view";
import { NOTICE_KIND, planNotice } from "../src/operator-notices";

/** Real Attention renderer, yesterday's watchdog diagnostic, and opened Details. */
export function renderAttentionNoticeFixture(app: HTMLElement): void {
  const now = Date.now();
  const reply = { notification_id: 901, reply_to: null, message: "The supervisor (happy-cheetah-1, Codex) was told 9 minutes ago that worker died: daring-robin-43, and the message never reached it.", summary: "Supervisor hasn't seen: worker died: daring-robin-43 (9m)", device_id: "*", notice: { source: "relay-watchdog", subject: 890 } };
  const plan = planNotice("atlas", "Accounting-rapid-gazelle-52", reply, () => false);
  if (plan.action !== "raise") throw new Error("expected notice");
  const main = document.createElement("main");
  main.style.cssText = "display:block;max-width:32rem;width:100%;margin:0 auto;padding:var(--space-4);box-sizing:border-box;overflow:auto;height:100dvh";
  const heading = document.createElement("h1");heading.textContent = "Attention";
  const panel = document.createElement("section");panel.id = "attention-panel";
  const item = createAttentionItem({ id: "notice-901", machineId: "atlas", machineLabel: "Atlas · Linux", session: "Accounting-rapid-gazelle-52", kind: NOTICE_KIND, createdAt: new Date(now - 86_400_000).toISOString() }, plan.content);
  renderAttentionPanel(panel, [item], { dismiss: () => {}, act: () => {}, copy: () => {} }, { now });
  panel.querySelector("details")!.open = true;
  main.append(heading, panel);app.replaceChildren(main);
}
