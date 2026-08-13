export interface PairingOperation {
  readonly generation: number;
  readonly signal: AbortSignal;
  readonly controller: AbortController;
}

/** Owns the cancellation boundary for one logical pairing flow. */
export class PairingOperationCoordinator {
  private generation = 0;
  private readonly active = new Set<AbortController>();

  replace(): number {
    this.invalidate();
    return this.generation;
  }

  invalidate(): void {
    this.generation += 1;
    for (const controller of this.active) controller.abort();
    this.active.clear();
  }

  begin(generation = this.generation): PairingOperation {
    const controller = new AbortController();
    const operation = { generation, signal: controller.signal, controller };
    if (generation === this.generation) this.active.add(controller);
    else controller.abort();
    return operation;
  }

  isCurrent(operation: PairingOperation): boolean {
    return operation.generation === this.generation && !operation.signal.aborted;
  }

  finish(operation: PairingOperation): void {
    this.active.delete(operation.controller);
  }
}

/** Await a result and commit it only while its pairing generation is current. */
export async function commitPairingResult<T>(
  coordinator: PairingOperationCoordinator,
  operation: PairingOperation,
  result: Promise<T>,
  commit: (value: T) => Promise<unknown> | unknown,
): Promise<boolean> {
  try {
    const value = await result;
    if (!coordinator.isCurrent(operation)) return false;
    await commit(value);
    return coordinator.isCurrent(operation);
  } finally {
    coordinator.finish(operation);
  }
}
