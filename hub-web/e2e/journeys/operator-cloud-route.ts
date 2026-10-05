// Attach the operator inbox protocol double (test/operator-cloud-double.ts)
// to a page at the cloud origin's network boundary. The page's own
// IndexedDB, WebCrypto and fetch are real Chromium; only the cloud's answers
// come from the double. Evidence label: "real-bundle, protocol-double". The
// deployed-cloud journey (cas-9b7d S5) is the acceptance gate.
import type { BrowserContext, Page, Route } from "@playwright/test";
import { OperatorCloudDouble } from "../../test/operator-cloud-double";
import { journeyNow } from "./clock";

export const OPERATOR_CLOUD = "https://petra-stella-cloud.vercel.app";

const PREFLIGHT_HEADERS = {
  "Access-Control-Allow-Methods": "GET, POST, PUT, DELETE, OPTIONS",
  "Access-Control-Allow-Headers": "Authorization, Content-Type, PSC-PoP-Proof",
  "Access-Control-Max-Age": "600",
};

async function answer(double: OperatorCloudDouble, route: Route): Promise<void> {
  const request = route.request();
  const url = new URL(request.url());
  const headers = Object.fromEntries(Object.entries(await request.allHeaders()).map(([key, value]) => [key.toLowerCase(), value]));
  const body = request.postDataBuffer() ?? Buffer.alloc(0);
  const result = await double.handle({
    method: request.method(),
    path: `${url.pathname}${url.search}`,
    headers,
    body: new Uint8Array(body),
  });
  await route.fulfill({
    status: result.status,
    contentType: "application/json",
    headers: {
      ...result.headers,
      ...(request.method() === "OPTIONS" ? PREFLIGHT_HEADERS : {}),
      Date: new Date(double.now()).toUTCString(),
    },
    body: result.status === 204 ? "" : JSON.stringify(result.body),
  });
}

/** Route the cloud's operator API (and its approval page origin) to `double`. */
export async function routeOperatorCloud(target: Page | BrowserContext, double: OperatorCloudDouble): Promise<void> {
  await target.route(`${OPERATOR_CLOUD}/api/operator/**`, (route) => answer(double, route));
}

export function operatorCloudDouble(): OperatorCloudDouble {
  // The double shares the journey clock with the page, so proof times and
  // token expiries agree without relying on the skew re-sign.
  return new OperatorCloudDouble({ baseUrl: OPERATOR_CLOUD, now: journeyNow });
}
