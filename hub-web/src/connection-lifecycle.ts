export interface ManagedMachineConnection<Machine extends { id: string }> {
  readonly machine: Machine;
  start(): void;
  stop(): void;
}

export function ensureMachineConnection<Machine extends { id: string }, Connection extends ManagedMachineConnection<Machine>>(
  machine: Machine,
  connections: Map<string, Connection>,
  createConnection: (machine: Machine) => Connection,
): Connection {
  const existing = connections.get(machine.id);
  if (existing) return existing;
  const connection = createConnection(machine);
  connections.set(machine.id, connection);
  connection.start();
  return connection;
}

export function replaceMachineConnection<Machine extends { id: string }, Connection extends ManagedMachineConnection<Machine>>(
  machine: Machine,
  connections: Map<string, Connection>,
  connectionStates: { delete(machineId: string): boolean },
  createConnection: (machine: Machine) => Connection,
): Connection {
  connections.get(machine.id)?.stop();
  connections.delete(machine.id);
  connectionStates.delete(machine.id);
  return ensureMachineConnection(machine, connections, createConnection);
}
