import { expect, type Locator, type Page, type TestInfo } from "@playwright/test";

/** Observe DOM writes in the real bundle; no product debug hook or timer. */
export async function installDraftDiagnostic(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const ids = new WeakMap<HTMLTextAreaElement, number>();
    let next = 0;
    const describe = (field: HTMLTextAreaElement | null | undefined) => {
      if (!field) return null;
      if (!ids.has(field)) ids.set(field, ++next);
      return { node: ids.get(field), connected: field.isConnected, thread: field.dataset.threadKey,
        value: field.value, start: field.selectionStart, end: field.selectionEnd };
    };
    const current = () => document.querySelector<HTMLTextAreaElement>("#message-text");
    let lastRenderReason: unknown;
    let lastValueWrite: unknown;
    let lastInput: unknown;
    // The render reason is its actual bundle caller stack, including function
    // names and file:line:column, rather than an inferred network event.
    const html = Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML")!;
    Object.defineProperty(Element.prototype, "innerHTML", { ...html, set(value: string) {
      if (this.id === "app") lastRenderReason = {
        kind: "shell replacement", at: performance.now(), before: describe(current()), caller: new Error().stack,
      };
      html.set!.call(this, value);
    } });
    const text = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!;
    Object.defineProperty(HTMLTextAreaElement.prototype, "value", { ...text, set(value: string) {
      if (this.id === "message-text") lastValueWrite = {
        at: performance.now(), before: describe(this), value, caller: new Error().stack,
      };
      text.set!.call(this, value);
    } });
    document.addEventListener("input", event => {
      if (event.target instanceof HTMLTextAreaElement && event.target.id === "message-text") {
        lastInput = { at: performance.now(), field: describe(event.target) };
      }
    }, true);
    (window as unknown as { __casDraftDiagnostic: () => unknown }).__casDraftDiagnostic = () => {
      const reference = (window as unknown as { __draftField?: HTMLTextAreaElement }).__draftField;
      return { title: document.title, focus: document.activeElement?.id,
        current: describe(current()), reference: describe(reference), sameNode: current() === reference,
        lastInput, lastRenderReason, lastValueWrite };
    };
  });
}

/** A mismatch keeps the failure red and makes its trace explain the DOM loss. */
export async function expectDraft(page: Page, field: Locator, expected: string, testInfo: TestInfo): Promise<void> {
  try {
    await expect(field).toHaveValue(expected);
  } catch (error) {
    const observed = await page.evaluate(() =>
      (window as unknown as { __casDraftDiagnostic: () => unknown }).__casDraftDiagnostic(),
    ).catch(reason => ({ diagnosticError: String(reason) }));
    await testInfo.attach("composer-draft-mismatch", {
      body: JSON.stringify({ expected, observed }, null, 2), contentType: "application/json",
    }).catch(reason => {
      testInfo.annotations.push({ type: "draft-diagnostic-error", description: String(reason) });
    });
    throw error;
  }
}
