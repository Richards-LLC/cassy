import { HubRequestError } from "./connection";
import { newOperationId, type FleetAction, type FleetOpsState, type OperationRequest } from "./fleet-ops";

type SendOperation = (request: OperationRequest & { op_id: string }) => Promise<{ outcome?: Readonly<Record<string, unknown>> } | undefined>;

/** Transport diagnostics belong in logs; the operator needs a cause and next step. */
function transportDetail(error: HubRequestError | undefined): string {
  if (!error) return "the machine could not be reached. Check its connection and try again.";
  if (error.status === 403) return "this pairing does not allow the action. Check its permissions in Paired machines.";
  if (error.status === 401) return "the pairing is no longer accepted. Pair the machine again.";
  if (error.status >= 500) return "the machine returned an error. Try again.";
  return "the machine refused the action. Refresh the fleet and try again.";
}

/** The async operation mutates only the request slot and selection lifetime it started in. */
export async function runFleetOperation(state: FleetOpsState, rowKey: string, action: FleetAction, send: SendOperation, started: () => void): Promise<"succeeded" | "failed" | undefined> {
  const owner = state.started(rowKey, action);
  started();
  try {
    const answer = await send({ op_id: newOperationId(), op: { ...action.request.op }, expected: { ...action.request.expected } });
    if (!state.owns(owner)) return;
    state.succeeded(rowKey, action, Date.now(), answer?.outcome);
    return "succeeded";
  } catch (error) {
    if (!state.owns(owner)) return;
    const refused = error instanceof HubRequestError ? error : undefined;
    const current = refused?.body?.current;
    const detail = refused?.detail?.trim() ? refused.detail : undefined;
    const subject = action.request.op.worker ?? action.request.expected.worker ?? action.request.op.task_id ?? action.request.op.epic_id;
    state.failed(rowKey, action, {
      stale: refused?.status === 409 && refused.code === "stale",
      ...(current && typeof current === "object" ? { current: current as Record<string, unknown> } : {}),
      detail: detail ?? transportDetail(refused),
      ...(!detail && typeof subject === "string" && !action.label.includes(subject) ? { subject } : {}),
    });
    return "failed";
  }
}
