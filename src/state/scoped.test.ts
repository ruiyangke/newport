import { expect, it } from "vitest";
import { registerRetain, retainAll } from "./scoped";

it("fans retention out to every participant with both key spaces", () => {
  const seen: { servers: Set<string>; connections: Set<string> }[] = [];
  const unregisterA = registerRetain((selections) => {
    seen.push(selections);
  });
  const unregisterB = registerRetain((selections) => {
    seen.push(selections);
  });
  const servers = new Set(["server"]);
  const connections = new Set(["connection"]);
  retainAll({ servers, connections });
  expect(seen).toHaveLength(2);
  for (const selections of seen) {
    expect(selections.servers).toBe(servers);
    expect(selections.connections).toBe(connections);
  }
  unregisterA();
  unregisterB();
  retainAll({ servers, connections });
  expect(seen).toHaveLength(2);
});
