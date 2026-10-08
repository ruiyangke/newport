import { createStore } from "@tanstack/react-store";
import { registerRetain } from "../state/scoped";

export interface PendingCommand {
  command: string;
  /** Press Enter too. Off for commands that would prompt for a password. */
  run: boolean;
  /** The new tab's name. */
  title?: string;
}

/**
 * Commands another page asked the Terminal to type, by server id: Services
 * hands over a `sudo systemctl …` it can't run without a password (typed,
 * not run); Containers opens a shell inside a container (run). The Terminal
 * takes each one once, oldest first. A queue, not a slot: handing over a
 * second command before visiting the Terminal must not discard the first.
 */
export const pendingTerminalCommands = createStore<
  Record<string, PendingCommand[]>
>({});

export function queueTerminalCommand(
  serverId: string,
  command: string,
  options: { run?: boolean; title?: string } = {},
) {
  pendingTerminalCommands.setState((state) => ({
    ...state,
    [serverId]: [
      ...(state[serverId] ?? []),
      { command, run: options.run ?? false, title: options.title },
    ],
  }));
}

export function takeTerminalCommand(
  serverId: string,
): PendingCommand | undefined {
  const pending = pendingTerminalCommands.state[serverId]?.[0];
  if (pending === undefined) return undefined;
  pendingTerminalCommands.setState((state) => {
    const queue = state[serverId];
    if (!queue?.length) return state;
    if (queue.length === 1) {
      const next = { ...state };
      delete next[serverId];
      return next;
    }
    return { ...state, [serverId]: queue.slice(1) };
  });
  return pending;
}

/** Drop handovers for servers that are gone; the Terminal drains the rest. */
export function retainPendingTerminalCommands(valid: Set<string>) {
  pendingTerminalCommands.setState((state) => {
    if (Object.keys(state).every((key) => valid.has(key))) return state;
    return Object.fromEntries(
      Object.entries(state).filter(([key]) => valid.has(key)),
    );
  });
}
registerRetain(({ servers }) => retainPendingTerminalCommands(servers));
