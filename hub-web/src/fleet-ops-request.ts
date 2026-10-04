import { HubRequestError } from "./connection";
import { newOperationId, type FleetAction, type FleetOpsState, type OperationRequest } from "./fleet-ops";

type SendOperation = (request: OperationRequest & { op_id: string }) => Promise<{ outcome?: Readonly<Record<string, unknown>> } | undefined>;

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
    state.failed(rowKey, action, {
      stale: refused?.status === 409 && refused.code === "stale",
      ...(current && typeof current === "object" ? { current: current as Record<string, unknown> } : {}),
      detail: refused?.detail ?? (error instanceof Error ? error.message : undefined),
    });
    return "failed";
  }
}
