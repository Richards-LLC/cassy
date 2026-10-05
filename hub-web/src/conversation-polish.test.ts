// @vitest-environment jsdom
import { afterEach, expect, it } from "vitest";
import { installAttentionObjects } from "./attention-objects";
import { ConversationHistory } from "./conversation-history";
import { ConversationView } from "./conversation-view";
import { entryText } from "./context-rail";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.replaceChildren(); });
it("shows a free-text question once, with no invented choices and a clean waiting jump", () => {
  dispose = installAttentionObjects();
  const history = new ConversationHistory();
  history.reply({ notification_id: 91, reply_to: null, message: "**Which plan** should we use?", summary: "", device_id: "d", kind: "ask" }, Date.now());
  const view = new ConversationView(document, history, { supervisor: "atlas-sup", respond: () => {} });
  document.body.append(view.element, view.pinned);
  view.update();
  expect(document.querySelectorAll("button.chip")).toHaveLength(0);
  expect(document.querySelectorAll('.obj[data-kind="ask"]')).toHaveLength(1);
  expect(view.element.querySelector(".obj-body strong")?.textContent).toBe("Which plan");
  expect(view.pinned.textContent).not.toContain("**");
  expect(view.pinned.querySelector(".obj")).toBeNull();
});
it("strips supported markdown before truncating a waiting preview", () => {
  expect(entryText("**Which plan** and `cas-1234`?\nDetails follow.")).toBe("Which plan and cas-1234?");
});
