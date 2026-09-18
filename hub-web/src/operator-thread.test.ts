import { describe, expect, it } from "vitest";
import { operatorThreadMarkup } from "./operator-thread";

describe("Commander operator reply thread", () => {
  it("omits the thread shell when there are no replies", () => {
    expect(operatorThreadMarkup([])).toBe("");
  });

  it("keeps each reply tied to its originating notification and escapes text", () => {
    const markup = operatorThreadMarkup([{
      notification_id: 93,
      reply_to: 41,
      message: "Ready <now> & confirmed",
      summary: "deployment status",
      device_id: "phone-7",
      operator_label: "Daniel & team",
    }]);

    expect(markup).toContain('aria-label="Commander conversation"');
    expect(markup).toContain('data-reply-to="41"');
    expect(markup).toContain('data-notification-id="93"');
    expect(markup).toContain("Daniel &amp; team");
    expect(markup).toContain("Ready &lt;now&gt; &amp; confirmed");
    expect(markup).toContain("reply to #41");
  });

  it("renders typed turns and attachment link rows without redesigning the thread", () => {
    const markup = operatorThreadMarkup([{
      notification_id: 94,
      reply_to: null,
      message: "See the receipt.",
      summary: "receipt",
      device_id: "*",
      kind: "receipt",
      attachments: [{ artifact_id: "report/1", name: "report.pdf", mime: "application/pdf", size_bytes: 42, sha256: "a".repeat(64) }],
    }]);

    expect(markup).toContain('data-kind="receipt"');
    expect(markup).toContain('href="#artifact:report%2F1"');
    expect(markup).toContain('data-artifact-id="report/1"');
    expect(markup).toContain("report.pdf");
  });
});
