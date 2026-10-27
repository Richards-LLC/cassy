import { test, expect, journeyPart } from "./journey";
import { ATLAS } from "./world";

const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a0l8AAAAASUVORK5CYII=";
for (const [width, height, colorScheme] of [[1280, 800, "light"], [1280, 800, "dark"], [390, 844, "light"], [390, 844, "dark"]] as const) {
  test.describe(`composer paste ${width} ${colorScheme}`, () => {
    test.use({ viewport: { width, height }, colorScheme });
    const run: Parameters<typeof test>[2] = async ({ page, journey }) => {
      const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
      const uploads = new Map<string, { name: string; data: string; path: string }>();
      let interrupted = false;
      const attempts: string[] = [];
      await page.route("https://*.test/v1/sessions/*/uploads/*", async route => {
        const id = route.request().url().split("/").at(-1)!;
        const body = route.request().postDataJSON() as { name: string; data: string };
        attempts.push(id);
        const prior = uploads.get(id);
        if (prior) expect(body).toEqual({ name: prior.name, data: prior.data });
        const stored = prior ?? { ...body, path: `/fixture/project/artifacts/${id}/${body.name}` };
        uploads.set(id, stored);
        if (!interrupted) { interrupted = true; await route.abort("connectionreset"); return; }
        await route.fulfill({ json: { artifact_id: `art-${id}`, path: stored.path, mime: body.name.endsWith("png") ? "image/png" : "text/plain" }, headers: { "access-control-allow-origin": "*" } });
      });
      const field = page.getByRole("textbox", { name: "Your message" });
      const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
      await journey.stage("Paste plain text inline", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        await field.focus();
        await page.evaluate(() => navigator.clipboard.writeText("A pasted message\nwith a second line"));
        await field.press("ControlOrMeta+V");
        await expect(field).toHaveValue("A pasted message\nwith a second line");
      });
      await journey.stage("Preview a long paste, screenshot and picked file together", async () => {
        await field.evaluate((element, png) => {
          const text = new DataTransfer(); text.setData("text/plain", "Long pasted content ".repeat(1000));
          element.dispatchEvent(new ClipboardEvent("paste", { clipboardData: text, bubbles: true, cancelable: true }));
          const image = new DataTransfer();
          image.items.add(new File([Uint8Array.from(atob(png), c => c.charCodeAt(0))], "screenshot.png", { type: "image/png" }));
          element.dispatchEvent(new ClipboardEvent("paste", { clipboardData: image, bubbles: true, cancelable: true }));
        }, PNG);
        const chooser = page.waitForEvent("filechooser");
        await page.getByRole("button", { name: "Attach files", exact: true }).click();
        await (await chooser).setFiles({ name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("Project file from the phone picker") });
        await expect(page.getByRole("button", { name: "Remove pasted-text.txt", exact: true })).toBeVisible();
        await expect(page.getByRole("button", { name: "Remove screenshot.png", exact: true })).toBeVisible();
        await expect(page.locator(".composer-file img")).toBeVisible();
        await expect(page.getByRole("button", { name: "Remove notes.txt", exact: true })).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      });
      await journey.stage("A connection loss keeps the files visibly, then reuses the upload identity", async () => {
        await send.click();
        await expect(page.locator(".composer-uploads")).toContainText("Files were not sent");
        await expect(page.getByRole("button", { name: "Remove screenshot.png", exact: true })).toBeVisible();
        expect(uploads.size).toBe(1);
        const next = hub.nextSend();
        await send.click();
        const message = await next;
        expect(message.text).toContain("A pasted message\nwith a second line");
        expect(uploads.size).toBe(3);
        for (const upload of uploads.values()) expect(message.text).toContain(upload.path);
        expect(attempts[0]).toBe(attempts[1]);
        await expect(page.locator(".composer-uploads")).toBeHidden();
        await expect(field).toHaveValue("");
      });
    };
    if (width === 1280 && colorScheme === "light") test("HUB-J24 paste content into the composer", run);
    else test(`HUB-J24 paste content ${width} ${colorScheme}`, journeyPart, run);
  });
}
