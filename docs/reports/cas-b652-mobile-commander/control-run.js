// cas-b652 — modern-Chromium control run.
// Same hub, same session, Pixel 7 device descriptor. Separates CSS-authored
// mobile defects from Chrome-113 engine-age failures.
const fs = require("fs");
const { chromium, devices } = require("playwright");

const OUT = "/home/pippenz/.cas/artifacts/cas-b652/pw";
const HUB = "https://soundwave-linux.tailf5a734.ts.net";
const SESSION = process.env.PW_SESSION || "cas-src-young-raven-93";
const PAIR_URL = process.env.PW_PAIR_URL;

const log = (...a) => console.log("[pw]", ...a);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  fs.mkdirSync(OUT, { recursive: true });
  const browser = await chromium.launch({
    // headless-shell crashes the renderer on this app (WASM/canvas path);
    // headed Chromium on :0 loads it fine, so the control run is headed.
    headless: false,
    args: ["--no-sandbox", "--disable-dev-shm-usage", "--window-position=2000,2000"],
  });
  const context = await browser.newContext({
    ...devices["Pixel 7"],
    recordVideo: { dir: `${OUT}/video`, size: { width: 412, height: 915 } },
  });
  const page = await context.newPage();
  const consoleErrors = [];
  page.on("console", (m) => { if (m.type() === "error") consoleErrors.push(m.text()); });
  page.on("pageerror", (e) => consoleErrors.push("pageerror: " + e.message));

  log("viewport", page.viewportSize(), "dpr", await page.evaluate(() => devicePixelRatio));

  // ---- pair -------------------------------------------------------------
  await page.goto(PAIR_URL, { waitUntil: "domcontentloaded" });
  await sleep(2500);
  await page.screenshot({ path: `${OUT}/p01-loaded.png`, fullPage: false });

  await page.getByRole("button", { name: /Pair a machine/i }).first().click();
  await sleep(1200);
  await page.screenshot({ path: `${OUT}/p02-pair-dialog.png` });

  await page.fill('#pair-dialog input[name="label"]', "soundwave-linux");
  await page.fill('#pair-dialog input[name="operator"]', "Daniel");
  await page.screenshot({ path: `${OUT}/p03-form-filled.png` });
  await page.locator('#pair-dialog button[type="submit"]').click();
  await sleep(6000);
  await page.screenshot({ path: `${OUT}/p04-paired.png` });
  log("paired?", await page.locator("text=No session open").count());

  // like-for-like bottom rail crop
  const rail = page.locator(".machine-navigation, .machine-rail").first();
  if (await rail.count()) {
    await rail.screenshot({ path: `${OUT}/p05-bottom-rail.png` }).catch(() => {});
  }

  // ---- attach -----------------------------------------------------------
  const open = page.getByRole("button", { name: /Open machines|Machines/i }).first();
  if (await open.count()) { await open.click(); await sleep(2500); }
  await page.screenshot({ path: `${OUT}/p06-drawer.png` });

  const sess = page.getByText(SESSION, { exact: true }).first();
  if (await sess.count()) { await sess.click(); } else { log("session row not found"); }
  await sleep(9000);
  await page.screenshot({ path: `${OUT}/p07-attached.png` });

  // ---- streaming capture ------------------------------------------------
  fs.writeFileSync(`${OUT}/CAPTURE_START`, String(Date.now()));
  log("capture window open");
  for (let i = 0; i < 40; i++) {
    await page.screenshot({ path: `${OUT}/s${String(i).padStart(2, "0")}.png` });
    await sleep(300);
  }
  fs.writeFileSync(`${OUT}/CAPTURE_END`, String(Date.now()));
  await page.screenshot({ path: `${OUT}/p08-after-stream.png` });

  // terminal geometry + what the pane actually contains
  const probe = await page.evaluate(() => {
    const el = document.querySelector(".pane .terminal-mount, .pane canvas, .terminal-mount");
    const pane = document.querySelector(".pane");
    const txt = (document.querySelector(".pane") || document.body).innerText.slice(0, 400);
    const r = el ? el.getBoundingClientRect() : null;
    return {
      found: !!el, tag: el && el.tagName,
      rect: r && { w: Math.round(r.width), h: Math.round(r.height) },
      canvasW: el && el.width, canvasH: el && el.height,
      paneH: pane && Math.round(pane.getBoundingClientRect().height),
      hasAbortAny: typeof AbortSignal.any === "function",
      ua: navigator.userAgent, dpr: devicePixelRatio,
      innerText: txt,
    };
  });
  fs.writeFileSync(`${OUT}/probe.json`, JSON.stringify({ probe, consoleErrors: consoleErrors.slice(0, 25) }, null, 2));
  log("probe", JSON.stringify(probe).slice(0, 300));
  log("console errors:", consoleErrors.length);

  // landscape check on the same engine
  await page.setViewportSize({ width: 915, height: 412 });
  await sleep(2500);
  await page.screenshot({ path: `${OUT}/p09-landscape.png` });

  await context.close();
  await browser.close();
  log("done");
})().catch((e) => { console.error("[pw] FAILED", e); process.exit(1); });
