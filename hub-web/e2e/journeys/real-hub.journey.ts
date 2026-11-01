import { test, expect, type Request } from "@playwright/test";
import { access, chmod, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { claimReceiptDirectory, showRecordingChapter } from "./receipt-directory.mjs";
import { RECORD } from "./journey";
import {
  saveQaTrace,
  redactQaText,
  redactQaValue,
} from "../../../scripts/visual-qa.mjs";

const control = `http://127.0.0.1:${process.env.REAL_HUB_CONTROL_PORT ?? 29925}`;
async function command(path: string, body = {}) {
  const response = await fetch(`${control}/${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  });
  const data = await response.json();
  if (!response.ok)
    throw new Error(`Disposable fixture ${path}: ${data.error}`);
  return data;
}

// Real wall time is necessary for DPoP and the minute-long outage. The generic
// journey fixture installs a synthetic epoch and checks a removed view; this
// test deliberately owns only pairing, conversation and connection evidence.
test(
  "HUB-J11 real disposable hub recovers and replays across every network failure",
  {
    annotation: {
      type: "journey-part",
      description: "real hub, real operator queue, Linux",
    },
  },
  async ({ page, context }, info) => {
    const directory = resolve(
      process.env.JOURNEY_RECEIPTS ??
        join(process.env.JOURNEY_OUTPUT ?? "e2e/.results/real-hub", "journeys"),
      "HUB-J11",
      "parts",
      `real-hub-run-${info.repeatEachIndex}`,
    );
    claimReceiptDirectory(directory, info.title);
    const secrets: string[] = [];
    const credentialReads: Promise<void>[] = [];
    page.on("response", (response) => {
      if (!new URL(response.url()).pathname.startsWith("/v1/auth/")) return;
      credentialReads.push(
        response
          .json()
          .then((data) => {
            for (const [key, value] of Object.entries(data)) {
              if (
                /credential|ticket|token/i.test(key) &&
                typeof value === "string"
              )
                secrets.push(value);
            }
          })
          .catch(() => undefined),
      );
    });
    const stages: {
      title: string;
      slug: string;
      ms: number;
      screenshot: string;
    }[] = [];
    const errors: string[] = [];
    const network: {
      path: string;
      started: number;
      ms?: number;
      failed?: boolean;
    }[] = [];
    const requests = new Map<Request, (typeof network)[number]>();
    page.on("pageerror", (error) => errors.push(redactQaText(error, secrets)));
    page.on("request", (request) => {
      if (new URL(request.url()).hostname !== "journey-hub.ts.net") return;
      const entry = {
        path: new URL(request.url()).pathname,
        started: performance.now(),
      };
      requests.set(request, entry);
      network.push(entry);
    });
    const completed = (request: Request, failed: boolean) => {
      const entry = requests.get(request);
      if (entry) {
        entry.ms = performance.now() - entry.started;
        entry.failed = failed;
        requests.delete(request);
      }
    };
    page.on("requestfinished", (request) => completed(request, false));
    page.on("requestfailed", (request) => completed(request, true));
    // Observe real daemon state, rather than trusting a possibly stale Live header.
    const welcomes: number[] = [];
    const attachments: unknown[] = [];
    page.on("websocket", (socket) => socket.on("framereceived", ({ payload }) => {
      if (typeof payload !== "string") return;
      try {
        const frame = JSON.parse(payload);
        // The real daemon publishes UTC pass timestamps. Match them against
        // the monotonic epoch of this actual frame, not the fixture clock.
        if (frame.message?.Welcome) welcomes.push(performance.timeOrigin + performance.now());
      } catch { /* Non-JSON frames are pane output. */ }
    }));
    const header = page.locator("#conversation-connection");
    const stage = async (title: string, body: () => Promise<void>) =>
      test.step(title, async () => {
        await showRecordingChapter(page, title, { duration: 1000 });
        const started = performance.now();
        await body();
        const screenshot = `J${String(stages.length + 1).padStart(2, "0")}.png`;
        await page.screenshot({ path: join(directory, screenshot) });
        stages.push({
          title,
          slug: title.toLowerCase().replace(/[^a-z0-9]+/g, "-"),
          ms: performance.now() - started,
          screenshot,
        });
      });
    let navigations = 0;
    let fixture:
      | {
          pairUrl: string;
          hubUrl: string;
          version: string;
          hubPid: number;
          daemonPid: number;
        }
      | undefined;
    let evidence: unknown;
    let passed = false;
    await context.tracing.start({
      screenshots: false,
      snapshots: true,
      sources: true,
    });
    await page.screencast.start({
      path: join(directory, "receipt.webm"),
      size: { width: 1280, height: 800 },
    });
    try {
      fixture = await command("reset");
      const pairing = new URL(fixture!.pairUrl);
      secrets.push(
        fixture!.pairUrl,
        new URLSearchParams(pairing.hash.slice(1)).get("pair") ?? "",
      );
      // cas hub pair returns the origin root; the production app is /commander/.
      pairing.pathname = "/commander/";
      await page.addInitScript(() =>
        localStorage.setItem("cas-commander:dormant", "1"),
      );
      await stage("Pair a browser with the real disposable hub", async () => {
        try {
          await page.goto(pairing.href);
        } catch {
          throw new Error("The real pairing page did not load");
        }
        const dialog = page.locator("#pair-dialog");
        await expect(dialog).toBeVisible();
        await dialog
          .getByRole("textbox", { name: "Your name (shown on the machine)" })
          .fill("Disposable journey");
        await dialog.getByRole("button", { name: "Pair", exact: true }).click();
        await expect(dialog).toBeHidden();
        await page
          .getByRole("navigation", { name: "Choose a supervisor" })
          .getByRole("button", { name: /project on/ }).and(page
          .getByRole("navigation", { name: "Choose a supervisor" }).locator(".conversation-row"))
          .click();
        await expect(header).toContainText("Live");
        await command("queue", {
          message: "Real supervisor queue is connected.",
        });
        await expect(
          page
            .getByRole("log")
            .getByText("Real supervisor queue is connected.", { exact: true }),
        ).toHaveCount(1);
      });
      // Collect the private credential only for redaction; never serialize it.
      secrets.push(
        ...(await page.evaluate(
          () =>
            new Promise<string[]>((resolve, reject) => {
              const open = indexedDB.open("cas-commander-v1");
              open.onerror = () => reject(Error("machine catalog unavailable"));
              open.onsuccess = () => {
                const db = open.result;
                const request = db
                  .transaction("machines")
                  .objectStore("machines")
                  .getAll();
                request.onsuccess = () => {
                  resolve(
                    request.result
                      .flatMap((record) => [
                        record.credential,
                        record.pairingInstall?.candidate?.credential,
                      ])
                      .filter((value) => typeof value === "string"),
                  );
                  db.close();
                };
                request.onerror = () => {
                  db.close();
                  reject(Error("machine catalog unreadable"));
                };
              };
            }),
        )),
      );
      page.on("framenavigated", (frame) => {
        if (frame === page.mainFrame()) navigations++;
      });
      await page.evaluate(() => {
        const states: string[] = [];
        (window as unknown as { realHubStates: string[] }).realHubStates =
          states;
        new MutationObserver(() => {
          const current =
            document.querySelector("#conversation-connection")?.textContent ??
            "";
          if (current && states.at(-1) !== current) states.push(current);
        }).observe(document.body, {
          subtree: true,
          childList: true,
          characterData: true,
        });
      });
      const replay = async (message: string) => {
        await expect(header).toContainText("Live", { timeout: 30_000 });
        await expect(page.getByRole("log")).toContainText(message, {
          timeout: 15_000,
        });
        const more = page
          .getByRole("log")
          .getByRole("button", { name: "Show full update", exact: true });
        if (await more.isVisible()) await more.click();
        await expect(
          page.getByRole("log").getByText(message, { exact: true }),
        ).toHaveCount(1);
        expect(navigations, "recovery must not reload the browser").toBe(0);
        await expect(page.locator("#pair-dialog")).toBeHidden();
      };
      const countAttached = async (afterWelcome: number) => {
        let snapshot: Awaited<ReturnType<typeof command>>;
        await expect.poll(async () => {
          snapshot = await command("attachment");
          return snapshot.loopPid === fixture!.daemonPid && Date.parse(snapshot.last_pass_at ?? "") > afterWelcome;
        }, { message: "first published completed pass after the real Welcome" }).toBe(true);
        // The loop records ws_clients immediately before complete_pass. Do not
        // wait for a later positive count: this first fresh snapshot must count it.
        expect(snapshot.commander_clients).toBeGreaterThanOrEqual(1);
        attachments.push({ welcomeAt: afterWelcome, ...snapshot });
      };
      await stage(
        "Restart the real hub and replay the queued message",
        async () => {
          const before = welcomes.length;
          await command("stop");
          await expect(header).toContainText("Reconnecting");
          await command("queue", { message: "Queued during the hub restart." });
          await command("start");
          await replay("Queued during the hub restart.");
          await expect.poll(() => welcomes.length).toBeGreaterThan(before);
          await countAttached(welcomes.at(-1)!);
        },
      );
      await stage("Replace the daemon without losing the open phone", async () => {
        const before = welcomes.length;
        const stopped = await command("daemon-stop");
        await expect(header).toContainText("Reconnecting");
        // Observe the old daemon's end receipt before starting its replacement.
        // Otherwise a fast restart can hide the ended-to-live catalog gap.
        await expect(page.locator('#message-status[data-session-ended="true"]')).toBeVisible();
        const replacement = await command("daemon-start");
        expect(replacement.daemonPid).not.toBe(stopped.daemonPid);
        fixture!.daemonPid = replacement.daemonPid;
        await expect.poll(() => welcomes.length, { timeout: 30_000 }).toBeGreaterThan(before);
        await expect(header).toContainText("Live");
        await countAttached(welcomes.at(-1)!);
        expect(navigations, "replacement must not reload the phone").toBe(0);
      });
      await stage(
        "Wait through a real sixty-second outage with bounded retries",
        async () => {
          const pausedAt = network.length;
          await command("pause");
          try {
            // SIGSTOP leaves the real TCP listener open but prevents replies.
            // A refused connection would not prove the finite probe deadline.
            await expect
              .poll(
                () =>
                  network
                    .slice(pausedAt)
                    .filter(
                      (request) =>
                        request.path === "/v1/sessions" &&
                        request.failed &&
                        request.ms !== undefined,
                    ).length,
                {
                  timeout: 12_000,
                  message:
                    "a real unanswered catalog request aborts at its deadline",
                },
              )
              .toBeGreaterThan(0);
            expect(
              network
                .slice(pausedAt)
                .filter(
                  (request) =>
                    request.path === "/v1/sessions" && request.ms !== undefined,
                )
                .every((request) => request.ms! <= 5_000),
            ).toBe(true);
          } finally {
            await command("resume");
          }
          await expect(header).toContainText("Live");
          await command("stop");
          await expect(header).toContainText("Reconnecting");
          const started = performance.now();
          const offset = network.length;
          await command("queue", {
            message: "Queued during the minute-long outage.",
          });
          // This is the fault duration, not a delay added to make an assertion pass.
          await new Promise((resolve) => setTimeout(resolve, 60_000));
          expect(performance.now() - started).toBeGreaterThanOrEqual(60_000);
          const during = network.slice(offset);
          expect(
            during.length,
            "backoff bounds actual hub requests during the outage",
          ).toBeLessThanOrEqual(30);
          expect(
            during.length,
            "the app actually attempted recovery",
          ).toBeGreaterThan(0);
          expect(
            during.every(
              (request) => request.ms !== undefined && request.ms <= 15_000,
            ),
            "every outage request settles within its deadline",
          ).toBe(true);
          await command("start");
          await replay("Queued during the minute-long outage.");
        },
      );
      await stage(
        "Go offline and recover without losing the supervisor reply",
        async () => {
          await context.setOffline(true);
          await expect(header).toContainText(/Offline|Reconnecting/);
          await command("queue", {
            message: "Queued while this browser was offline.",
          });
          await context.setOffline(false);
          await replay("Queued while this browser was offline.");
        },
      );
      await stage(
        "Deny Local Network Access and grant it to recover",
        async () => {
          const cdp = await context.newCDPSession(page);
          const { targetInfo } = await cdp.send("Target.getTargetInfo");
          for (const name of ["local-network", "loopback-network"])
            await cdp.send("Browser.setPermission", {
              permission: { name },
              setting: "denied",
              origin: control,
              browserContextId: targetInfo.browserContextId,
            });
          expect(
            await page.evaluate(
              async () =>
                (
                  await navigator.permissions.query({
                    name: "local-network" as PermissionName,
                  })
                ).state,
            ),
          ).toBe("denied");
          await command("stop");
          await command("start");
          await command("queue", {
            message: "Queued while Local Network Access was denied.",
          });
          await expect(
            page
              .getByRole("status")
              .filter({ hasText: /allow local network access for this page/i }),
          ).toBeVisible();
          // Browser.grantPermissions does not overwrite a per-origin
          // Browser.setPermission denial. Change those same real permissions.
          for (const name of ["local-network", "loopback-network"])
            await cdp.send("Browser.setPermission", {
              permission: { name },
              setting: "granted",
              origin: control,
              browserContextId: targetInfo.browserContextId,
            });
          expect(
            await page.evaluate(
              async () =>
                (
                  await navigator.permissions.query({
                    name: "local-network" as PermissionName,
                  })
                ).state,
            ),
          ).toBe("granted");
          await replay("Queued while Local Network Access was denied.");
          await cdp.detach();
        },
      );
      await stage(
        "Verify the real hub authentication and reconnect audit",
        async () => {
          evidence = await command("evidence");
          const audit = (
            evidence as {
              audit: { action: string; outcome: string; machine_id: string }[];
            }
          ).audit;
          expect(
            audit.filter((event) => event.action === "pairing_exchange"),
          ).toHaveLength(1);
          expect(
            audit.filter((event) => event.action === "dpop_auth").length,
          ).toBeGreaterThan(3);
          expect(
            audit.filter((event) => event.action === "websocket_read").length,
          ).toBeGreaterThanOrEqual(5);
          expect(
            new Set(audit.map((event) => event.machine_id)).size,
            "all reconnects preserve the paired hub identity",
          ).toBe(1);
          const logs = evidence as {
            hubLog: string;
            hubTrace: string;
            daemonLog: string;
          };
          expect(logs.hubLog.match(/cas hub serve starting/g)).toHaveLength(4);
          expect(
            logs.hubTrace.match(
              /relayed Commander conversation history response/g,
            )?.length,
          ).toBeGreaterThanOrEqual(5);
          expect(logs.hubTrace).toContain("replies=5");
          expect(logs.daemonLog.length).toBeGreaterThan(0);
          const states = await page.evaluate(
            () =>
              (window as unknown as { realHubStates: string[] }).realHubStates,
          );
          expect(states.some((state) => /needs pairing/i.test(state))).toBe(
            false,
          );
          expect(
            states.filter((state) => /Reconnecting/.test(state)).length,
          ).toBeGreaterThanOrEqual(3);
          expect(errors).toEqual([]);
        },
      );
      passed = true;
    } finally {
      try {
        if (fixture)
          evidence ??= await command("evidence").catch(() => undefined);
        await writeFile(
          join(directory, "hub-evidence.json"),
          JSON.stringify(
            redactQaValue({ evidence, network, attachments }, secrets),
            null,
            2,
          ),
        );
        await writeFile(
          join(directory, "final.aria.yml"),
          await page
            .locator("body")
            .ariaSnapshot()
            .catch(() => "# page unavailable"),
        );
        await writeFile(
          join(directory, "final.aria.json"),
          JSON.stringify(
            await page
              .locator("body")
              .ariaSnapshotJSON()
              .catch(() => null),
            null,
            2,
          ),
        );
        await page.screencast.stop().catch(() => undefined);
        await Promise.allSettled(credentialReads);
        await saveQaTrace(context, join(directory, "trace.zip"), { secrets });
        await writeFile(
          join(directory, "result.json"),
          JSON.stringify(
            {
              id: "HUB-J11",
              title: info.title,
              status: passed ? "PASS" : "FAIL",
              label:
                "real-bundle, real-hub, real-factory-daemon, inert-supervisor-provider",
              project: info.project.name,
              viewport: page.viewportSize(),
              clock: { source: "real wall time" },
              binary: fixture?.version,
              output_dir: directory,
              stages,
              page_errors: errors,
            },
            null,
            2,
          ),
        );
      } finally {
        await context.close().catch(() => undefined);
        await command("cleanup");
      }
    }
  },
);


// cas-7ebc: the catalog's double supplied options the real payload discarded.
// Keep this check on the actual CLI, daemon queue, hub and production browser.
test("HUB-J7 real ask choices survive reload and the phone answer names its question", {
  annotation: { type: "journey-part", description: "real hub, CLI ask, durable linked answer" },
}, async ({ page, context }, info) => {
  const directory = resolve(process.env.JOURNEY_RECEIPTS ??
    join(process.env.JOURNEY_OUTPUT ?? "e2e/.results/real-hub", "journeys"),
    "HUB-J7", "parts", `real-hub-run-${info.repeatEachIndex}`);
  claimReceiptDirectory(directory, info.title);
  const secrets: string[] = [];
  let fixture: { pairUrl: string; version: string } | undefined;
  let tracing = false;
  let passed = false;
  let readback: unknown;
  try {
    fixture = await command("reset", { messageControl: true });
    const pairing = new URL(fixture!.pairUrl);
    secrets.push(fixture!.pairUrl, new URLSearchParams(pairing.hash.slice(1)).get("pair") ?? "");
    pairing.pathname = "/commander/";
    await page.setViewportSize({ width: 390, height: 844 });
    await page.addInitScript(() => localStorage.setItem("cas-commander:dormant", "1"));
    // Pair outside the trace so invitation and installation keys are not captured.
    await page.goto(pairing.href);
    await page.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Disposable choices QA");
    await page.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(page.locator("#pair-dialog")).toBeHidden();
    // Pairing opens the only conversation on a phone. Its list is then
    // hidden; clicking the disappearing row races that automatic navigation.
    await expect(page.getByRole("textbox", { name: "Your message", exact: true })).toBeVisible();
    await expect(page.locator("#conversation-connection")).toContainText("Live");
    await context.tracing.start({ screenshots: false,
      snapshots: { dom: true, aria: true, screen: true }, sources: true });
    tracing = true;
    const ask = await command("ask");
    const question = page.getByRole("group", { name: "Question from journey-supervisor", exact: true });
    await expect(question.getByRole("button", { name: "Keep testing", exact: true })).toBeVisible();
    await expect(question.getByRole("button")).toHaveCount(2);
    await page.screenshot({ path: join(directory, "J01-live-choices.png") });
    await page.reload();
    await expect(question.getByRole("button", { name: "Keep testing", exact: true })).toBeVisible();
    await page.screenshot({ path: join(directory, "J02-history-choices.png") });
    await question.getByRole("button", { name: "Keep testing", exact: true }).click();
    await expect.poll(async () => (await command("answers", { notificationId: ask.message_id })).length).toBe(1);
    readback = await command("answers", { notificationId: ask.message_id });
    expect(readback).toEqual([expect.objectContaining({
      target: "journey-supervisor", operator_verified: 1,
      prompt: `[CAS reply: explicitly acknowledges notification_id=${ask.message_id}]\nKeep testing`,
    })]);
    await expect(question.getByLabel("You replied: Keep testing")).toBeVisible();
    await expect(question.getByRole("button")).toHaveCount(0);
    await page.reload();
    await expect(question.getByLabel("You replied: Keep testing")).toBeVisible();
    await expect(question.getByRole("button")).toHaveCount(0);
    expect((await command("answers", { notificationId: ask.message_id })).length).toBe(1);
    await page.screenshot({ path: join(directory, "J03-linked-answer.png") });
    passed = true;
  } finally {
    try {
      await page.screenshot({ path: join(directory, "final.png") }).catch(() => undefined);
      await writeFile(join(directory, "final.aria.yml"), await page.locator("body").ariaSnapshot().catch(() => "# unavailable"));
      await writeFile(join(directory, "final.aria.json"), JSON.stringify(await page.locator("body").ariaSnapshotJSON().catch(() => null), null, 2));
      await writeFile(join(directory, "result.json"), JSON.stringify({
        id: "HUB-J7", status: passed ? "PASS" : "FAIL", binary: fixture?.version,
        label: "real-bundle, real-hub, real-factory-daemon, inert-supervisor-provider",
        viewport: page.viewportSize(), readback,
      }, null, 2));
      if (tracing) await saveQaTrace(context, join(directory, "trace.zip"), { secrets });
    } finally {
      await context.close().catch(() => undefined);
      await command("cleanup");
    }
  }
});

// The disposable project's config must load when the real upload stores bytes;
// protocol doubles cannot catch a malformed config in real-hub-server.mjs.
test("HUB-J24 real hub composer decodes a local raster preview and stores the picked file", {
  annotation: { type: "journey-part", description: "real hub, parsed fixture config, host file readback" },
}, async ({ page }, info) => {
  try {
    const fixture = await command("reset", { messageControl: true, nativeCommander: true });
    const pairing = new URL(fixture!.pairUrl);
    pairing.pathname = "/commander/";
    await page.setViewportSize({ width: 390, height: 844 });
    await page.addInitScript(() => localStorage.setItem("cas-commander:dormant", "1"));
    const documentResponse = await page.goto(pairing.href);
    expect(documentResponse!.headers()["content-security-policy"]).toContain("default-src 'none'");
    await page.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Disposable upload QA");
    await page.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(page.locator("#pair-dialog")).toBeHidden();
    await expect(page.locator("#conversation-connection")).toContainText("Live");
    if (RECORD) {
      await page.evaluate(() => {
        const violations: string[] = [];
        (window as unknown as { chapterStyleViolations: string[] }).chapterStyleViolations = violations;
        document.addEventListener("securitypolicyviolation", event => {
          if (event.effectiveDirective.startsWith("style-src")) violations.push(event.effectiveDirective);
        });
      });
      await page.screencast.start({ path: info.outputPath("native-chapter.webm"), size: { width: 390, height: 844 } });
      const chapter = showRecordingChapter(page, "M01 Choose a local image", { duration: 5000 });
      await expect(page.getByText("M01 Choose a local image", { exact: true })).toBeVisible();
      await page.screenshot({ path: info.outputPath("native-chapter.png") });
      await chapter;
      expect(await page.evaluate(() => (window as unknown as { chapterStyleViolations: string[] }).chapterStyleViolations)).toEqual([]);
    }
    await expect(page.getByText("M01 Choose a local image", { exact: true })).toHaveCount(0);
    const imageViolations: string[] = [];
    await page.exposeFunction("recordImageViolation", (blocked: string) => imageViolations.push(blocked));
    await page.evaluate(() => document.addEventListener("securitypolicyviolation", event => {
      if (event.effectiveDirective === "img-src")
        (window as unknown as { recordImageViolation: (blocked: string) => void }).recordImageViolation(event.blockedURI);
    }));
    // Untrusted active formats stay named files, never mounted as previews.
    const unsafeChooser = page.waitForEvent("filechooser");
    await page.getByRole("button", { name: "Attach files", exact: true }).click();
    await (await unsafeChooser).setFiles({ name: "untrusted.svg", mimeType: "image/svg+xml", buffer: Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>') });
    await expect(page.locator(".composer-file")).toContainText("untrusted.svg");
    await expect(page.locator(".composer-file img")).toHaveCount(0);
    await page.getByRole("button", { name: "Remove untrusted.svg", exact: true }).click();
    const bytes = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=", "base64");
    const chooser = page.waitForEvent("filechooser");
    await page.getByRole("button", { name: "Attach files", exact: true }).click();
    await (await chooser).setFiles({ name: "fixture.png", mimeType: "image/png", buffer: bytes });
    const thumbnail = page.locator(".composer-file img");
    await expect(thumbnail).toHaveAttribute("src", /^blob:/);
    await expect.poll(() => thumbnail.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(1);
    expect(imageViolations).toEqual([]);
    await page.screenshot({ path: info.outputPath("pending-thumbnail.png") });
    const uploaded = page.waitForResponse(response =>
      response.request().method() === "POST" && new URL(response.url()).pathname.includes("/uploads/"));
    await page.getByRole("button", { name: /^Send to the .+ supervisor$/, exact: true }).click();
    const response = await uploaded;
    expect(response.status(), await response.text()).toBe(200);
    const stored = await response.json();
    expect(await readFile(stored.path)).toEqual(bytes);
    await expect(page.locator(".composer-uploads")).toBeHidden();
  } finally {
    if (RECORD) await page.screencast.stop().catch(() => undefined);
    await command("cleanup");
  }
});

test("HUB-J13 real fixture keeps cleanup state when removal is refused", {
  annotation: { type: "journey-part", description: "real hub, failed removal and explicit recovery" },
}, async () => {
  const fixture = await command("reset", { newSession: true });
  const home = dirname(fixture.project);
  try {
    // A real filesystem refusal, without racing a writer or mocking rm.
    await chmod(home, 0o500);
    await expect(command("cleanup")).rejects.toThrow(/EACCES/);
    await chmod(home, 0o700);
    await command("cleanup");
    await expect(access(home)).rejects.toMatchObject({ code: "ENOENT" });
  } finally {
    await chmod(home, 0o700).catch(error => { if (error.code !== "ENOENT") throw error; });
    await command("cleanup");
  }
});

// Unlike the protocol double, this crosses POST /v1/sessions and the real
// reaper/daemon/PTY bootstrap. Only provider and OS service placement are inert.
test("HUB-J13 real hub starts a supervisor and carries a conversation", {
  annotation: { type: "journey-part", description: "real New session launch" },
}, async ({ page, context }, info) => {
  const directory = resolve(process.env.JOURNEY_RECEIPTS ??
    join(process.env.JOURNEY_OUTPUT ?? "e2e/.results/real-hub", "journeys"),
    "HUB-J13", "parts", `real-hub-run-${info.repeatEachIndex}`);
  claimReceiptDirectory(directory, info.title);
  const fixture = await command("reset", { newSession: true });
  const pairing = new URL(fixture!.pairUrl);
  pairing.pathname = "/commander/";
  const secrets = [fixture.pairUrl, new URLSearchParams(pairing.hash.slice(1)).get("pair") ?? ""];
  const redactions: Promise<void>[] = [];
  page.on("response", (response) => {
    if (!new URL(response.url()).pathname.startsWith("/v1/auth/")) return;
    redactions.push(response.json().then((body) => {
      for (const [key, value] of Object.entries(body)) {
        if (/credential|ticket|token/i.test(key) && typeof value === "string") secrets.push(value);
      }
    }).catch(() => undefined));
  });
  let passed = false;
  let tracing = false;
  await page.addInitScript(() => localStorage.setItem("cas-commander:dormant", "1"));
  try {
    // Pair outside the trace, as with the other real-hub parts.
    await page.goto(pairing.href);
    const dialog = page.locator("#pair-dialog");
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Launch journey");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    await context.tracing.start({ screenshots: false,
      snapshots: { dom: true, aria: true, screen: true }, sources: true });
    tracing = true;
    await page.getByRole("button", { name: "New session", exact: true }).click();
    const sheet = page.getByRole("dialog", { name: "New session" });
    await sheet.getByRole("tab", { name: "Browse" }).click();
    await sheet.getByRole("radio", { name: /^project/ }).check();
    await page.screenshot({ path: join(directory, "J01-choose-project.png") });
    await sheet.getByRole("button", { name: "Start", exact: true }).click();
    await expect(sheet).toBeHidden({ timeout: 30_000 });
    await expect(page.getByRole("log").getByText("Supervisor ready", { exact: true })).toBeVisible({ timeout: 30_000 });
    await page.screenshot({ path: join(directory, "J02-first-message.png") });
    await page.getByRole("textbox", { name: "Your message", exact: true }).fill("ping launch supervisor");
    await page.getByRole("button", { name: "Send to the project supervisor", exact: true }).click();
    await expect(page.getByRole("log").getByText("Launch supervisor answered", { exact: true })).toBeVisible({ timeout: 30_000 });
    await page.screenshot({ path: join(directory, "J03-answer.png") });
    const composer = page.getByRole("textbox", { name: "Your message", exact: true });
    await composer.fill("/exit");
    await composer.press("Enter");
    const confirmation = page.getByRole("alertdialog", { name: /Exit/ });
    await expect(confirmation).toBeVisible();
    await confirmation.getByRole("button", { name: "Exit", exact: true }).click();
    const exitReceipt = page.locator(".supervisor-command-receipt");
    await expect(exitReceipt).toContainText("/exit");
    await expect(exitReceipt).toHaveAttribute("data-phase", "observed", { timeout: 15_000 });
    await expect(page.getByRole("tabpanel", { name: "Conversation" }).getByText("Session ended", { exact: true }))
      .toBeVisible({ timeout: 15_000 });
    await page.screenshot({ path: join(directory, "J04-exit-ended.png") });
    passed = true;
  } finally {
    try {
      await writeFile(join(directory, "hub-evidence.json"), JSON.stringify(await command("evidence"), null, 2));
      await writeFile(join(directory, "final.aria.yml"), await page.locator("body").ariaSnapshot().catch(() => "# unavailable"));
      await writeFile(join(directory, "result.json"), JSON.stringify({
        id: "HUB-J13", status: passed ? "PASS" : "FAIL", binary: fixture.version,
        label: "real-bundle, real-hub, real-reaper, real-factory-daemon, inert-provider-and-placement",
        viewport: page.viewportSize(),
      }, null, 2));
      await Promise.all(redactions);
      if (tracing) await saveQaTrace(context, join(directory, "trace.zip"), { secrets });
    } finally {
      await context.close().catch(() => undefined);
      await command("cleanup");
      await expect(access(dirname(fixture.project))).rejects.toMatchObject({ code: "ENOENT" });
    }
  }
});

// cas-f8ab: process-owned stopped fixtures, the actual hub catalog and embedded
// candidate client. Real wall time proves a two-minute absence of attach traffic.
test("HUB-J14 real hub hides stopped sessions and recovers running sessions", {
  annotation: { type: "journey-part", description: "crash without receipt, ended daemon, machine outage and pending send" },
}, async ({ page, context }, info) => {
  test.setTimeout(240_000);
  const directory = resolve(process.env.JOURNEY_RECEIPTS ?? join(process.env.JOURNEY_OUTPUT ?? "e2e/.results/real-hub", "journeys"), "HUB-J14", "parts", `real-stopped-run-${info.repeatEachIndex}`);
  claimReceiptDirectory(directory, info.title);
  const traffic: { path: string; frame?: unknown; at: number }[] = [];
  const catalogs: unknown[] = [];
  let passed = false;
  let fixture: { pairUrl: string; version: string } | undefined;
  let stopped: { name: string; pid: number; signal: string }[] = [];
  let observationMs = 0;
  let journalSends: unknown[] = [];
  const secrets: string[] = [];
  try {
    fixture = await command("reset", { messageControl: true });
    stopped = await command("stopped-sessions");
    page.on("request", request => {
      const path = new URL(request.url()).pathname;
      if (path.includes("/attach")) traffic.push({ path, at: performance.now() });
    });
    page.on("response", response => {
      if (new URL(response.url()).pathname !== "/v1/sessions") return;
      void response.json().then(body => catalogs.push(body)).catch(() => {});
    });
    page.on("websocket", socket => socket.on("framesent", ({ payload }) => {
      if (typeof payload !== "string") return;
      try { const frame = JSON.parse(payload); if (frame.subscribe) traffic.push({ path: new URL(socket.url()).pathname, frame, at: performance.now() }); } catch { /* Pane bytes. */ }
    }));
    await page.addInitScript(() => localStorage.setItem("cas-commander:dormant", "1"));
    const pairing = new URL(fixture!.pairUrl); pairing.pathname = "/commander/";
    secrets.push(fixture!.pairUrl, new URLSearchParams(pairing.hash.slice(1)).get("pair") ?? "");
    await page.goto(pairing.href);
    await page.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Session lifecycle QA");
    await page.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(page.locator("#pair-dialog")).toBeHidden();
    await context.tracing.start({ screenshots: false, snapshots: { dom: true, aria: true, screen: true }, sources: true });
    const rows = page.locator("#conversation-list .conversation-row");
    await expect(rows).toHaveCount(1);
    await expect(rows).toContainText("journey-supervisor");
    await expect(page.getByRole("button", { name: /Dismiss ended|Clear ended/ })).toHaveCount(0);
    for (const width of [1280, 390]) for (const colorScheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await page.emulateMedia({ colorScheme });
      const back = page.locator("#conversation-back"); if (await back.isVisible()) await back.click();
      await expect(rows).toHaveCount(1);
      await page.screenshot({ path: join(directory, `running-only-${colorScheme}-${width}.png`) });
    }
    const started = performance.now();
    await expect.poll(() => performance.now() - started, { timeout: 125_000, intervals: [1000] }).toBeGreaterThanOrEqual(120_000);
    observationMs = performance.now() - started;
    expect(traffic.filter(entry => /crashed-journey|ended-journey/.test(JSON.stringify(entry)))).toEqual([]);
    // The running session remains through a machine outage, without one retry
    // line per row. Its own fresh Welcome proves automatic recovery.
    await page.setViewportSize({ width: 1280, height: 800 });
    await rows.click();
    await expect(page.locator("#conversation-connection")).toContainText("Live");
    await command("stop");
    await expect(page.locator("#conversation-connection")).toContainText(/Reconnecting|Connecting/);
    await expect(rows).toHaveCount(1);
    await expect(rows).not.toContainText(/retry|Reconnecting|message pending/i);
    await page.screenshot({ path: join(directory, "machine-outage.png") });
    await command("start");
    await expect(page.locator("#conversation-connection")).toContainText("Live");
    // A held message to a recipient now gone becomes Not sent, with its
    // original words accessible in machine Attention. It never auto-replays.
    await command("stop");
    await expect(page.locator("#conversation-connection")).toContainText(/Reconnecting|Connecting/);
    await page.getByRole("textbox", { name: "Your message", exact: true }).fill("Keep this stopped-session instruction");
    await page.getByRole("button", { name: /^Send to the .+ supervisor$/ }).click();
    await expect(page.getByText("Waiting for the connection — auto-sends for up to 2 minutes; after that, choose Retry.", { exact: true })).toBeVisible();
    await command("daemon-stop");
    await command("start");
    await expect(rows).toHaveCount(0);
    await expect(page.locator(".project-card")).toHaveCount(0);
    await expect(page.getByText("Waiting message not sent", { exact: true })).toBeVisible();
    const readSaved = async () => journalSends = await page.evaluate(async () => {
      const db = await new Promise<IDBDatabase>((resolve, reject) => { const r = indexedDB.open("cas-commander-delivery-v1", 1); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
      try { return await new Promise<unknown[]>((resolve, reject) => { const r = db.transaction("sends").objectStore("sends").getAll(); r.onsuccess = () => resolve(r.result.map(row => row.send).filter(Boolean)); r.onerror = () => reject(r.error); }); } finally { db.close(); }
    });
    await expect.poll(readSaved).toEqual(expect.arrayContaining([expect.objectContaining({ text: "Keep this stopped-session instruction", state: "error" })]));
    await page.screenshot({ path: join(directory, "stopped-message.png") });
    await command("daemon-start");
    await expect(rows).toHaveCount(1);
    await expect(rows).toContainText("journey-supervisor");
    passed = true;
  } finally {
    try {
      await writeFile(join(directory, "final.aria.yml"), await page.locator("body").ariaSnapshot().catch(() => "# unavailable"));
      await writeFile(join(directory, "final.aria.json"), JSON.stringify(await page.locator("body").ariaSnapshotJSON().catch(() => null)));
      await writeFile(join(directory, "result.json"), JSON.stringify({ id: "HUB-J14", status: passed ? "PASS" : "FAIL", binary: fixture?.version, stopped, observationMs, traffic, catalogs, journalSends }, null, 2));
      await saveQaTrace(context, join(directory, "trace.zip"), { secrets });
    } finally { await context.close(); await command("cleanup"); }
  }
});
