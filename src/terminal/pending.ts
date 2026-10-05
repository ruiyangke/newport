import { createStore } from "@tanstack/react-store";

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
 * takes each one once.
 */
export const pendingTerminalCommands = createStore<
  Record<string, PendingCommand>
>({});

export function queueTerminalCommand(
  serverId: string,
  command: string,
  options: { run?: boolean; title?: string } = {},
) {
  pendingTerminalCommands.setState((state) => ({
    ...state,
    [serverId]: { command, run: options.run ?? false, title: options.title },
  }));
}

export function takeTerminalCommand(
  serverId: string,
): PendingCommand | undefined {
  const pending = pendingTerminalCommands.state[serverId];
  if (pending === undefined) return undefined;
  pendingTerminalCommands.setState((state) => {
    const next = { ...state };
    delete next[serverId];
    return next;
  });
  return pending;
}
