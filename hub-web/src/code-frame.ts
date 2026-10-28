import "./code-frame.css";

export interface CodeFrameOptions {
  /** Literal source, independent of any later rich preview. */
  source: string;
  language?: string;
  filename?: string;
}

/** A reusable reading frame. Enhancers may replace pre inside content. */
export function createCodeFrame(document: Document, { source, language, filename }: CodeFrameOptions): {
  frame: HTMLElement;
  content: HTMLElement;
  pre: HTMLPreElement;
} {
  const frame = document.createElement("div");
  frame.className = "code-frame";
  frame.dataset.wrap = "false";
  const header = document.createElement("div");
  header.className = "code-frame-header";
  const label = document.createElement("span");
  label.className = "code-frame-label";
  label.textContent = filename || language || "Code";
  if (filename && language) label.title = `${filename} · ${language}`;
  const actions = document.createElement("div");
  actions.className = "code-frame-actions";
  const wrap = document.createElement("button");
  wrap.type = "button";
  wrap.dataset.action = "wrap";
  wrap.textContent = "Wrap";
  wrap.setAttribute("aria-label", `Wrap lines in ${label.textContent}`);
  wrap.setAttribute("aria-pressed", "false");
  wrap.addEventListener("click", () => {
    const pressed = wrap.getAttribute("aria-pressed") !== "true";
    frame.dataset.wrap = String(pressed);
    wrap.setAttribute("aria-pressed", String(pressed));
  });
  const copy = document.createElement("button");
  copy.type = "button";
  copy.dataset.action = "copy";
  copy.textContent = "Copy";
  copy.setAttribute("aria-label", `Copy source from ${label.textContent}`);
  const status = document.createElement("span");
  status.className = "code-frame-status";
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  status.setAttribute("aria-atomic", "true");
  copy.addEventListener("click", () => {
    // Call the clipboard while still inside the user gesture (Safari included).
    // A rejected or missing clipboard must never produce a success message.
    void (async () => {
      copy.disabled = true;
      status.textContent = "Copying…";
      try {
        const clipboard = document.defaultView?.navigator.clipboard;
        if (!clipboard?.writeText) throw new Error("Clipboard unavailable");
        await clipboard.writeText(source);
        status.textContent = "Copied";
      } catch {
        status.textContent = "Copy failed. Select the source and copy manually, or try again.";
      } finally {
        copy.disabled = false;
      }
    })();
  });
  actions.append(wrap, copy);
  header.append(label, actions);
  const content = document.createElement("div");
  content.className = "code-frame-content";
  const pre = document.createElement("pre");
  pre.className = "markdown-code";
  // The nowrap source is keyboard-scrollable even in browsers without automatic
  // focusability for overflow containers.
  pre.tabIndex = 0;
  const code = document.createElement("code");
  code.textContent = source;
  pre.append(code);
  content.append(pre);
  frame.append(header, content, status);
  return { frame, content, pre };
}
