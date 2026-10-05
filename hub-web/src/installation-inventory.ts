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
  return `Operator inbox (key epoch ${enrollment.epoch})`;
}
interface InstallationClient { request<T>(method: string, path: string): Promise<T> }

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
  body.append(title, lead, status, list, actions); dialog.append(body); doc.body.append(dialog);
  dialog.addEventListener("close", () => { dialog.remove(); if (opener?.isConnected) opener.focus(); }, { once: true });
  dialog.showModal();
  try {
    const devices = await client.request<InstallationSummary[]>("GET", "/v1/auth/devices");
    status.textContent = machine.scopes.includes("hub-admin") ? `${devices.length} installations, ${devices.filter((device) => device.account_enrollment?.state === "enrolled").length} in your operator inbox.` : "This browser's access. Viewing other installations requires hub admin permission.";
    devices.sort((a, b) => Number(b.device_id === machine.deviceId) - Number(a.device_id === machine.deviceId));
    for (const device of devices) {
      const row = doc.createElement("section"); row.className = "installation-inventory-row";
      const heading = doc.createElement("h3"); heading.textContent = `${device.device_label}${device.device_id === machine.deviceId ? " · This browser" : ""}`;
      const id = doc.createElement("code"); id.textContent = device.device_id;
      const details = doc.createElement("dl"); details.className = "pair-details";
      for (const [label, value] of [["Access", device.revoked_at ? "Revoked" : `Generation ${device.credential_generation}`], ["Paired", device.issued_at], ["Last used", device.last_used_at], ["Origin", device.controller_origin], ["Signing key", device.key_fingerprint], ["Account", accountEnrollmentLabel(device.account_enrollment)]]) {
        const item = doc.createElement("div"); const term = doc.createElement("dt"); const detail = doc.createElement("dd");
        term.textContent = label!; detail.textContent = value!; item.append(term, detail); details.append(item);
      }
      row.append(heading, id, details);
      if (!device.revoked_at) {
        const revoke = doc.createElement("button"); revoke.type = "button"; revoke.textContent = device.device_id === machine.deviceId ? "Revoke this browser's access" : "Revoke this installation";
        revoke.onclick = async () => {
          if (!doc.defaultView?.confirm(`Revoke ${device.device_label} (${device.device_id})? Its live connections will close. Replacing a lost browser requires a new pairing invitation.`)) return;
          revoke.disabled = true;
          try {
            await client.request("POST", `/v1/auth/devices/${encodeURIComponent(device.device_id)}/revoke`);
            status.textContent = `Revoked ${device.device_id}. Its live connections are closing.`;
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
