// Commander preview: the real production app (src/main.ts) against fixture
// machines served in-page by the journey HubDouble. Preview only.
import "./env";
import { PreviewPage } from "./browser-page";
import { HubDouble, type HistoryPage, type Machine } from "../e2e/journeys/hub-double";

const params = new URLSearchParams(location.search);
const now = Date.now();
const at = (minutesAgo: number) => new Date(now - minutesAgo * 60_000).toISOString();

const CAS = "patient-pelican-9";
const VIOLET = "brisk-lynx-12";
const STUDIO_S = "calm-otter-4";
const CLOUD = "quiet-otter-31";

const ATLAS: Machine = {
  id: "atlas", label: "Atlas · Linux",
  sessions: [
    { name: CAS, supervisor: CAS, project_dir: "/projects/cas-src", workers: ["young-otter-14", "ready-owl-88", "quiet-hawk-74"], liveness: "live", last_activity_at: at(1), last_activity: "Running the inbox journeys, 41 of 82" },
    { name: VIOLET, supervisor: VIOLET, project_dir: "/projects/violet", workers: ["calm-heron-20"], liveness: "live", last_activity_at: at(4), last_activity: "Rotated the dev token" },
  ],
};
const STUDIO: Machine = {
  id: "studio", label: "Studio Mac · macOS",
  sessions: [{ name: STUDIO_S, supervisor: STUDIO_S, project_dir: "/projects/gabber-studio", workers: ["bright-robin-85"], liveness: "live", last_activity_at: at(12), last_activity: "Exported the voice set" }],
};
const WORKSTATION: Machine = {
  id: "workstation", label: "Workstation · Linux",
  sessions: [{ name: CLOUD, supervisor: CLOUD, project_dir: "/projects/petra-stella-cloud", workers: ["bold-wren-44"], liveness: "live", last_activity_at: at(26), last_activity: "Built the preview deploy" }],
};

let id = 100;
const msg = (session: string, text: string, minutesAgo: number) => ({ notification_id: ++id, target: session, text, state: "acknowledged", stamped: true, device_id: "journey-device", session, at: at(minutesAgo) });
const reply = (session: string, message: string, minutesAgo: number, replyTo: number | null, extra: Record<string, unknown> = {}) => ({ notification_id: ++id, reply_to: replyTo, message, summary: "", device_id: "journey-device", attachments: [], session, at: at(minutesAgo), ...extra });

function thread(session: string, turns: Array<["me" | "sup", string, number]>): HistoryPage[] {
  const messages: Array<Record<string, unknown>> = [];
  const replies: Array<Record<string, unknown>> = [];
  let last: number | null = null;
  for (const [who, text, ago] of turns) {
    if (who === "me") { const m = msg(session, text, ago); messages.push(m); last = m.notification_id; }
    else { replies.push(reply(session, text, ago, last)); last = null; }
  }
  return [{ messages, replies, has_earlier: false }];
}

const history: Record<string, HistoryPage[]> = {
  [CAS]: thread(CAS, [
    ["sup", "Morning. The Commander epic has **4 lanes** in review:\n\n- inbox crypto (S0–S2 accepted)\n- atomic cross-tab sends\n- fleet failure note\n- Terminal view removal", 95],
    ["me", "Good. Prioritise the cross-tab sends — I lost a message yesterday.", 92],
    ["sup", "On it. ready-owl-88 has the corrective; QA round 2 is bound to the new tip.", 90],
    ["me", "What's left before 3.47?", 31],
    ["sup", "Two things:\n\n1. The inbox journeys (41 of 82 green so far)\n2. Your call on whether to ship before they finish.\n\nEverything else is merged and the gate is green.", 29],
  ]),
  [VIOLET]: thread(VIOLET, [
    ["sup", "Rotated the dev token and updated the staging secrets. Nothing else is pending.", 40],
    ["me", "Thanks. Keep staging warm for tomorrow's demo.", 38],
    ["sup", "Will do. I'll check the preview deploy every 30 minutes.", 37],
  ]),
  [STUDIO_S]: thread(STUDIO_S, [
    ["me", "Can you export the new voice set for the gabber build?", 70],
    ["sup", "Exported 24 voices to `dist/voices/`. The two Welsh voices need a second pass on pronunciation.", 64],
  ]),
  [CLOUD]: thread(CLOUD, [
    ["sup", "The preview deploy is built: `petra-stella-cloud-git-inbox.vercel.app`.", 27],
  ]),
};

const page = new PreviewPage();
page.installTransport();
const hub = new HubDouble(page as never, {
  machines: [ATLAS, STUDIO, WORKSTATION],
  paired: ["atlas", "studio", "workstation"],
  multiplex: true,
  history,
});
await hub.install();
// The exact production bundle (hub-web/dist/app.js), served beside this shim.
await import(/* @vite-ignore */ new URL("../app.js", import.meta.url).href);

async function seeded(): Promise<boolean> {
  if (!(await indexedDB.databases()).some((db) => db.name === "cas-commander-v1")) return false;
  return new Promise((ok) => {
    const req = indexedDB.open("cas-commander-v1");
    req.onsuccess = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains("machines")) { db.close(); ok(false); return; }
      const count = db.transaction("machines").objectStore("machines").count();
      count.onsuccess = () => { db.close(); ok(count.result >= 3); };
      count.onerror = () => { db.close(); ok(false); };
    };
    req.onerror = () => ok(false);
  });
}

if (!(await seeded())) {
  await hub.seedPaired();
  location.reload();
} else {
  // Live moments: a question arrives, and the supervisors answer what you send.
  const whenAttached = (session: string) => hub.waitFor(() => hub.attaches.includes(session));
  void whenAttached(CAS).then(() => setTimeout(() => {
    hub.supervisorSays(CAS, "The inbox journeys are 41 of 82 and green so far.\n\n**Ship 3.47.0 now, or hold for the full run?**", { kind: "ask", options: ["Ship now", "Hold for the full run"] });
  }, params.get("ask") === "0" ? 1e9 : 1200));
  let answered = 0;
  setInterval(() => {
    while (answered < hub.sends.length) {
      const sent = hub.sends[answered++];
      try {
        const queued = hub.deliverLatest(sent.session);
        setTimeout(() => {
          const text = sent.in_reply_to ? `Understood — "${sent.text}". I'll take it from here and report back.` : `Got it. I'll look into "${sent.text.slice(0, 60)}" and report back here.`;
          try { hub.answerQueued(sent.session, queued, text); } catch { /* socket gone */ }
        }, 1400);
      } catch { /* socket gone */ }
    }
  }, 300);
}
