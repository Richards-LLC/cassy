import { sheetKeydown } from "./attention-sheet";

/** Phone presentation of the active S5 control. Operation state stays in FleetOpsState. */
export function presentFleetSheet(container: HTMLElement, dismiss: () => void): void {
  const content = container.querySelector<HTMLElement>(".fleet-ops-confirm, .fleet-ops-preview, .fleet-ops-picker, .fleet-ops-panel, .fleet-ops-menu");
  if (!content) return;
  const document = container.ownerDocument;
  const sheet = document.createElement("dialog");
  sheet.className = "fleet-action-sheet";
  sheet.classList.toggle("fleet-action-sheet--picker", content.classList.contains("fleet-ops-picker"));
  // cas-68d0 F01: a panel named by its visible heading lends that heading to
  // the dialog and drops its own name, so the name is announced once.
  const labelledBy = content.getAttribute("aria-labelledby");
  if (labelledBy) { sheet.setAttribute("aria-labelledby", labelledBy); content.removeAttribute("aria-labelledby"); }
  else sheet.setAttribute("aria-label", content.getAttribute("aria-label") ?? "Fleet actions");
  const close = document.createElement("button");
  close.type = "button"; close.className = "fleet-sheet-close";
  // cas-4cf2: a panel may name its own close ("Close write access").
  close.setAttribute("aria-label", content.dataset.closeLabel ?? "Close actions"); close.textContent = "×";
  close.onclick = () => dismiss();
  sheet.oncancel = (event) => { event.preventDefault(); dismiss(); };
  // Native dialog containment includes the browser's document focus stop.
  // Keep keyboard navigation on a control when wrapping the action sheet.
  sheet.onkeydown = (event) => {
    if (event.key === "Tab" && sheetKeydown(event, sheet, document.activeElement, dismiss)) event.preventDefault();
  };
  content.before(sheet);
  sheet.append(close, content);
  sheet.showModal();
}
