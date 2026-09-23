import { describe, it, expect } from "vitest";
import { forwardsForPort, forwardingHost } from "./forwarding";
import { newTunnel } from "./types";

describe("forward matching", () => {
  it("matches saved forwards and range offsets without confusing servers", () => {
    const tunnel = {
      ...newTunnel("server", 9000),
      remotePort: 5174,
      remotePortEnd: 5176,
      localPortEnd: 9002,
    };
    const port = {
      port: 5175,
      address: "0.0.0.0",
      processName: "MainThread",
      pid: 1,
    };
    expect(forwardsForPort([tunnel], "server", port)).toEqual([tunnel]);
    expect(tunnel.localPort + port.port - tunnel.remotePort).toBe(9001);
    expect(forwardsForPort([tunnel], "other", port)).toEqual([]);
    expect(
      forwardsForPort([tunnel], "server", { ...port, port: 5177 }),
    ).toEqual([]);
    expect(
      forwardsForPort([tunnel], "server", { ...port, address: "::" }),
    ).toEqual([]);
  });
  it("normalizes wildcard and loopback addresses consistently", () => {
    expect(forwardingHost("LOCALHOST.")).toBe("127.0.0.1");
    expect(forwardingHost("0:0:0:0:0:0:0:0")).toBe("::1");
    expect(forwardingHost("*")).toBe("127.0.0.1");
  });
});
