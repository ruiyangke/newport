import { beforeEach, expect, it } from "vitest";
import {
  pendingTerminalCommands,
  queueTerminalCommand,
  retainPendingTerminalCommands,
  takeTerminalCommand,
} from "./pending";

beforeEach(() => pendingTerminalCommands.setState(() => ({})));

it("queues handovers instead of overwriting them", () => {
  queueTerminalCommand("s", "first");
  queueTerminalCommand("s", "second", { run: true, title: "shell" });
  expect(takeTerminalCommand("s")).toMatchObject({ command: "first" });
  expect(takeTerminalCommand("s")).toMatchObject({
    command: "second",
    run: true,
    title: "shell",
  });
  expect(takeTerminalCommand("s")).toBeUndefined();
  expect(pendingTerminalCommands.state).toEqual({});
});

it("prunes handovers for servers that are gone", () => {
  queueTerminalCommand("gone", "sudo reboot");
  queueTerminalCommand("kept", "docker shell");
  retainPendingTerminalCommands(new Set(["kept"]));
  expect(Object.keys(pendingTerminalCommands.state)).toEqual(["kept"]);
  expect(takeTerminalCommand("kept")?.command).toBe("docker shell");
});
