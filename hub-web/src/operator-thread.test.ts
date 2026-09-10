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
});
