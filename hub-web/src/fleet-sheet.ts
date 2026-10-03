/** Phone presentation of the active S5 control. Operation state stays in FleetOpsState. */
export function presentFleetSheet(container: HTMLElement, dismiss: () => void): void {
  const content = container.querySelector<HTMLElement>(".fleet-ops-confirm, .fleet-ops-preview, .fleet-ops-picker, .fleet-ops-panel, .fleet-ops-menu");
  if (!content) return;
  const document = container.ownerDocument;
  const sheet = document.createElement("dialog");
  sheet.className = "fleet-action-sheet";
  sheet.classList.toggle("fleet-action-sheet--picker", content.classList.contains("fleet-ops-picker"));
  sheet.setAttribute("aria-label", content.getAttribute("aria-label") ?? "Fleet actions");
  const close = document.createElement("button");
  close.type = "button"; close.className = "fleet-sheet-close";
  close.setAttribute("aria-label", "Close actions"); close.textContent = "×";
  close.onclick = dismiss;
  sheet.oncancel = (event) => { event.preventDefault(); dismiss(); };
  content.before(sheet);
  sheet.append(close, content);
  sheet.showModal();
}
