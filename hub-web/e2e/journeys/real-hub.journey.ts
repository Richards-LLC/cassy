import { test, expect, type Request } from "@playwright/test";
import { writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { claimReceiptDirectory } from "./receipt-directory.mjs";
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
    const header = page.locator("#conversation-connection");
    const stage = async (title: string, body: () => Promise<void>) =>
      test.step(title, async () => {
        await page.screencast.showChapter(title, { duration: 1000 });
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
          .getByRole("button", { name: /project on/ })
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
      await stage(
        "Restart the real hub and replay the queued message",
        async () => {
          await command("stop");
          await expect(header).toContainText("Reconnecting");
          await command("queue", { message: "Queued during the hub restart." });
          await command("start");
          await replay("Queued during the hub restart.");
        },
      );
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
            page.getByText(/Allow Local network access for this page/),
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
            redactQaValue({ evidence, network }, secrets),
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
