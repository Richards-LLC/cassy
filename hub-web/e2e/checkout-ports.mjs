// Default web-server ports for this checkout (cas-00ad).
//
// Several factory worktrees run Playwright on one machine. With fixed ports
// and a reused server, a run attached to whichever checkout's server was
// already listening and reported that build's result. Each checkout now gets
// its own default pair, derived from its absolute path, in 20000–32767 (below
// Linux's ephemeral range, 32768–60999, so an outgoing connection cannot hold
// it), and playwright.config.ts never reuses a running server: a port that is
// already taken fails the run loudly instead of testing someone else's build.
//
// Plain JavaScript on purpose (cas-6942): scripts/test-journeys.sh runs this
// with whatever Node the CI runner has, and `--experimental-strip-types`
// does not exist before Node 22.6. Types live in checkout-ports.d.mts.
//
// CLI: `node e2e/checkout-ports.mjs [hub-web dir]` prints "<fixtures> <journeys>".
import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";

const PORT_FLOOR = 20_000;
const PORT_CEILING = 32_767;

/**
 * The fixture and journey ports for the checkout at `dir` (the hub-web directory).
 * @param {string} dir
 * @returns {{ fixtures: number, journeys: number }}
 */
export function checkoutPorts(dir) {
  const digest = createHash("sha256").update(realpathSync(dir)).digest();
  const pairs = Math.floor((PORT_CEILING - PORT_FLOOR + 1) / 2);
  const fixtures = PORT_FLOOR + (digest.readUInt32BE(0) % pairs) * 2;
  return { fixtures, journeys: fixtures + 1 };
}

const invokedDirectly =
  process.argv[1] !== undefined &&
  import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href;
if (invokedDirectly) {
  const { fixtures, journeys } = checkoutPorts(process.argv[2] ?? ".");
  console.log(fixtures, journeys);
}
