// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { applySheetSemantics, sheetKeydown } from "./attention-sheet";

/** A conversation shell as conversationShellMarkup builds it: sidebar, main (badge), rail (sheet). */
function shell(): { root: HTMLElement; badge: HTMLButtonElement; rail: HTMLElement; close: HTMLButtonElement; details: HTMLElement; send: HTMLButtonElement } {
  const root = document.createElement("div"); root.className = "conversation-shell thread-open";
  root.innerHTML = '<aside class="conversation-sidebar"><nav id="conversation-list"><button class="conversation-row">row</button></nav></aside>'
    + '<main class="conversation-main"><button id="conversation-attention" aria-expanded="false">1</button><button id="message-send">Send</button></main>'
    + '<aside class="conversation-context" aria-label="Conversation context"><button class="context-sheet-close">×</button><section data-section="attention"><button class="dismiss">Dismiss</button><details><summary>Details</summary></details></section></aside>';
  document.body.replaceChildren(root);
  return {
    root,
    badge: root.querySelector("#conversation-attention")!,
    rail: root.querySelector(".conversation-context")!,
    close: root.querySelector(".context-sheet-close")!,
    details: root.querySelector("summary")!,
    send: root.querySelector("#message-send")!,
  };
}
const all = () => true;

describe("phone Attention sheet (cas-a5c6)", () => {
  it("is a modal dialog over an inert page while open, and a plain rail again once closed", () => {
    const { root, rail } = shell();
    applySheetSemantics(root, true);
    expect(rail.getAttribute("role")).toBe("dialog");
    expect(rail.getAttribute("aria-modal")).toBe("true");
    expect(rail.getAttribute("aria-label")).toBe("Attention for this session");
    expect(root.querySelector(".conversation-sidebar")!.hasAttribute("inert")).toBe(true);
    expect(root.querySelector(".conversation-main")!.hasAttribute("inert")).toBe(true);
    expect(rail.hasAttribute("inert")).toBe(false);
    // Closing, or the viewport becoming a desktop, takes every trace of the dialog away.
    applySheetSemantics(root, false);
    expect(rail.hasAttribute("role")).toBe(false);
    expect(rail.hasAttribute("aria-modal")).toBe(false);
    expect(rail.getAttribute("aria-label")).toBe("Conversation context");
    expect(root.querySelectorAll("[inert]")).toHaveLength(0);
    expect(root.classList.contains("attention-sheet-open")).toBe(false);
  });

  it("cycles Tab and Shift+Tab within the sheet and pulls stray focus back in", () => {
    const { rail, close, details, send } = shell();
    const shut = vi.fn();
    details.focus();
    expect(sheetKeydown({ key: "Tab", shiftKey: false }, rail, document.activeElement, shut, all)).toBe(true);
    expect(document.activeElement).toBe(close);
    expect(sheetKeydown({ key: "Tab", shiftKey: true }, rail, document.activeElement, shut, all)).toBe(true);
    expect(document.activeElement).toBe(details);
    // Between its own controls it steps in document order, skipping what Tab cannot reach.
    rail.querySelector<HTMLButtonElement>(".dismiss")!.tabIndex = -1;
    close.focus();
    expect(sheetKeydown({ key: "Tab", shiftKey: false }, rail, document.activeElement, shut, all)).toBe(true);
    expect(document.activeElement).toBe(details);
    // Focus behind the sheet (the composer's Send) comes back to it.
    send.focus();
    expect(sheetKeydown({ key: "Tab", shiftKey: true }, rail, document.activeElement, shut, all)).toBe(true);
    expect(document.activeElement).toBe(details);
    expect(shut).not.toHaveBeenCalled();
  });

  it("closes on Escape wherever focus is", () => {
    const { rail, send } = shell();
    const shut = vi.fn();
    send.focus();
    expect(sheetKeydown({ key: "Escape", shiftKey: false }, rail, document.activeElement, shut, all)).toBe(true);
    expect(shut).toHaveBeenCalledOnce();
    expect(sheetKeydown({ key: "a", shiftKey: false }, rail, document.activeElement, shut, all)).toBe(false);
  });
});
