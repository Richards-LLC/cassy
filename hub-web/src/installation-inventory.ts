import type { AccountEnrollment } from "./installation-access";
import type { StoredMachine } from "./types";

export interface InstallationSummary {
  device_id: string;
  device_label: string;
  operator_label: string;
  controller_origin: string;
  credential_generation: number;
  key_fingerprint: string;
  issued_at: string;
  last_used_at: string;
  revoked_at: string | null;
  account_enrollment: AccountEnrollment;
}
/** Plain words for the Account row (cas-4634: verified by the hub, never a label). */
export function accountEnrollmentLabel(enrollment: AccountEnrollment | undefined): string {
  if (!enrollment || enrollment.state === "unenrolled") return "Not in an operator inbox";
  return "In your operator inbox";
}
interface InstallationClient { request<T>(method: string, path: string): Promise<T> }

/** Relative words lead; the full time uses this browser's locale and zone. */
function installationTime(doc: Document, value: string): HTMLElement {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) {
    const missing = doc.createElement("span"); missing.textContent = "Not recorded"; return missing;
  }
  const time = doc.createElement("time"); time.dateTime = value;
  time.title = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(timestamp);
  // A machine clock can lead this browser's. Activity cannot be in the
  // future; retain the exact timestamp in the title and Technical details.
  const seconds = Math.min(0, (timestamp - Date.now()) / 1_000);
  const unit = Math.abs(seconds) >= 86_400 ? "day" : Math.abs(seconds) >= 3_600 ? "hour" : "minute";
  const size = unit === "day" ? 86_400 : unit === "hour" ? 3_600 : 60;
  time.textContent = Math.abs(seconds) < 60 ? "Just now" : new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(Math.trunc(seconds / size), unit);
  return time;
}

function detailRow(doc: Document, list: HTMLElement, label: string, value: string | Node): HTMLElement {
  const item = doc.createElement("div"); const term = doc.createElement("dt"); const detail = doc.createElement("dd");
  term.textContent = label; detail.append(value); item.append(term, detail); list.append(item);
  return detail;
}

/** Exact-ID consent is kept beside the inventory. Removing a local catalog is separate. */
export async function openInstallationInventory(doc: Document, machine: StoredMachine, client: InstallationClient, onOwnRevoke: () => Promise<void>): Promise<void> {
  const opener = doc.activeElement as HTMLElement | null;
  const dialog = doc.createElement("dialog");
  dialog.className = "installation-inventory";
  dialog.setAttribute("aria-labelledby", "installation-inventory-title");
  const body = doc.createElement("section"); body.className = "installation-inventory-body";
  const title = doc.createElement("h2"); title.id = "installation-inventory-title"; title.textContent = `${machine.label} browser installations`;
  const lead = doc.createElement("p"); lead.textContent = "Re-pairing updates this browser's installation. Revoke only the device you recognize.";
  const status = doc.createElement("p"); status.setAttribute("role", "status"); status.textContent = "Loading installations…";
  const list = doc.createElement("div"); list.className = "installation-inventory-list";
  const actions = doc.createElement("div"); actions.className = "dialog-actions";
  const close = doc.createElement("button"); close.type = "button"; close.textContent = "Close";
  close.onclick = () => dialog.close(); actions.append(close);
  // The actions are the sheet's footer, after the scrolling body, so nothing
  // (a wrapped intro's Revoke) can scroll under Close (cas-205e QA F01).
  body.append(title, lead, status, list); dialog.append(body, actions); doc.body.append(dialog);
  dialog.addEventListener("close", () => { dialog.remove(); if (opener?.isConnected) opener.focus(); }, { once: true });
  dialog.showModal();
  try {
    const devices = await client.request<InstallationSummary[]>("GET", "/v1/auth/devices");
    status.textContent = machine.scopes.includes("hub-admin") ? `${devices.length} installations. ${devices.filter((device) => device.account_enrollment?.state === "enrolled").length} in your operator inbox.` : "This browser's access. Viewing other installations requires hub admin permission.";
    devices.sort((a, b) => Number(b.device_id === machine.deviceId) - Number(a.device_id === machine.deviceId));
    for (const device of devices) {
      const row = doc.createElement("section"); row.className = "installation-inventory-row";
      const label = device.device_label.trim() || "Unnamed browser";
      const heading = doc.createElement("h3"); heading.textContent = `${label} · ${device.device_id === machine.deviceId ? "This browser" : "Another browser"}`;
      const summary = doc.createElement("dl"); summary.className = "pair-details installation-summary";
      detailRow(doc, summary, "Used by", device.operator_label.trim() || "Not recorded");
      const access = detailRow(doc, summary, "Access", device.revoked_at ? "Revoked" : "Active");
      detailRow(doc, summary, "Paired", installationTime(doc, device.issued_at));
      detailRow(doc, summary, "Last used", installationTime(doc, device.last_used_at));
      detailRow(doc, summary, "Account", accountEnrollmentLabel(device.account_enrollment));
      const technical = doc.createElement("details"); technical.className = "installation-technical";
      const disclosure = doc.createElement("summary"); disclosure.textContent = "Technical details";
      const details = doc.createElement("dl"); details.className = "pair-details";
      const id = doc.createElement("code"); id.textContent = device.device_id;
      detailRow(doc, details, "Installation ID", id);
      for (const [name, value] of [["Credential generation", String(device.credential_generation)], ["Paired (UTC)", device.issued_at], ["Last used (UTC)", device.last_used_at], ["Origin", device.controller_origin], ["Signing key", device.key_fingerprint]]) {
        detailRow(doc, details, name!, value!);
      }
      if (device.account_enrollment?.state === "enrolled") detailRow(doc, details, "Account key epoch", device.account_enrollment.epoch);
      technical.append(disclosure, details);
      row.append(heading, summary, technical);
      if (!device.revoked_at) {
        const revoke = doc.createElement("button"); revoke.type = "button"; revoke.textContent = device.device_id === machine.deviceId ? "Revoke this browser's access" : "Revoke this installation";
        revoke.onclick = async () => {
          if (!doc.defaultView?.confirm(`Revoke ${label} (${device.device_id})? Its live connections will close. Replacing a lost browser requires a new pairing invitation.`)) return;
          revoke.disabled = true;
          try {
            await client.request("POST", `/v1/auth/devices/${encodeURIComponent(device.device_id)}/revoke`);
            access.textContent = "Revoked";
            status.textContent = `Revoked ${label}. Its live connections are closing.`;
            if (device.device_id === machine.deviceId) { await onOwnRevoke(); dialog.close(); }
            else { revoke.textContent = "Revoked"; }
          } catch (error) { revoke.disabled = false; status.textContent = error instanceof Error ? error.message : "Revocation failed. Retry when the hub is reachable."; }
        };
        row.append(revoke);
      }
      list.append(row);
    }
  } catch (error) { status.textContent = error instanceof Error ? error.message : "Could not load installations. Retry when the hub is reachable."; }
}
