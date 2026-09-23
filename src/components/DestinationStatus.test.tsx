import { expect, it } from "vitest";
import { destinationSummary } from "./DestinationStatus";
import type { DestinationHealth } from "../types";
const health = (status: DestinationHealth["status"]): DestinationHealth => ({
  status,
  localPort: 3000,
  remotePort: 3000,
  checkedAt: 1,
  message: null,
});
it("distinguishes all, partial, unavailable and inconclusive destination checks", () => {
  expect(destinationSummary([])).toBe("Checking destination…");
  expect(destinationSummary([health("reachable")])).toBe(
    "Destination reachable",
  );
  expect(destinationSummary([health("reachable"), health("unavailable")])).toBe(
    "1/2 destinations reachable",
  );
  expect(destinationSummary([health("unavailable")])).toBe(
    "Destination unavailable",
  );
  expect(destinationSummary([health("blocked")])).toBe("Forwarding blocked");
  expect(destinationSummary([health("unknown")])).toBe(
    "Destination check inconclusive",
  );
});
