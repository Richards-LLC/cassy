// The world every journey starts from: two machines a real operator has,
// each running one live supervisor.
import type { Machine } from "./hub-double";

export const ATLAS: Machine = {
  id: "atlas",
  label: "Atlas · Linux",
  sessions: [{ name: "patient-pelican-9", supervisor: "patient-pelican-9", project_dir: "/projects/cas-src", workers: ["young-otter-14"], liveness: "live" }],
};

export const STUDIO: Machine = {
  id: "studio",
  label: "Studio Mac · macOS",
  sessions: [{ name: "calm-otter-4", supervisor: "calm-otter-4", project_dir: "/projects/gabber-studio", workers: ["bright-robin-85"], liveness: "live" }],
};

export const PELICAN = "patient-pelican-9";
export const OTTER = "calm-otter-4";
