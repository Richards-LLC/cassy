import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

test("HUB-J7 answer a pinned question", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  const pinned = page.getByRole("region", { name: `Waiting on you: question from ${PELICAN}` });
  let ask = 0;

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });

  await journey.stage("The supervisor asks a question", async () => {
    ask = hub.supervisorSays(PELICAN, "The gate failed on one clippy warning. Fix it in the train, or ship with it allowlisted?", { kind: "ask", options: ["Fix in-train", "Ship with allowlist"] });
    await expect(pinned).toBeVisible();
    await expect(pinned.getByRole("button", { name: "Fix in-train" })).toBeVisible();
    // The pinned card is the one place to answer (F18): the thread keeps a
    // one-line reference and the rail does not list this question again.
    const reference = page.getByRole("log").locator(".obj.t-a.ask-collapsed");
    await expect(reference.locator(".ask-excerpt")).toHaveText("The gate failed on one clippy warning. Fix it in the train, or ship with it allowlisted?");
    await expect(reference.getByRole("button")).toHaveCount(0);
    await expect(page.locator(".conversation-context .context-jump")).toHaveCount(0);
    await expect(page.locator('.conversation-context [data-section="waiting"]')).toBeHidden();
  });

  await journey.stage("Answer with one tap", async () => {
    const sent = hub.nextSend();
    await pinned.getByRole("button", { name: "Fix in-train" }).click();
    expect(await sent).toMatchObject({ text: "Fix in-train", in_reply_to: ask });
    await expect(pinned).toBeHidden();
    // The question stays in the thread with the chosen answer and no open choices.
    await expect(page.getByRole("log").getByRole("group", { name: `Question from ${PELICAN}` })).toMatchAriaSnapshot(`
      - group "Question from ${PELICAN}":
        - paragraph: /clippy warning/
        - text: Fix in-train
    `);
    await expect(page.getByRole("button", { name: "Ship with allowlist" })).toHaveCount(0);
  });

  await journey.stage("See the supervisor act on the answer", async () => {
    hub.answerLatest(PELICAN, "Fixing the warning in the train now.");
    await expect(page.getByRole("log").getByText("Fixing the warning in the train now.")).toBeVisible();
  });
});
