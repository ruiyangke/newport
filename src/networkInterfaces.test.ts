import { expect, it } from "vitest";
import { interfaceKind, splitInterfaces } from "./networkInterfaces";

it("tells uplinks and VPNs from container, bridge and loopback links", () => {
  for (const name of ["eth0", "ens3", "enp0s31f6", "wlan0", "bond0"])
    expect(interfaceKind(name)).toBe("physical");
  for (const name of ["tailscale0", "wg0", "tun0", "zt5u4y6ab"])
    expect(interfaceKind(name)).toBe("vpn");
  for (const name of [
    "veth1a2b3c",
    "br-0123456789ab",
    "docker0",
    "virbr0",
    "cni0",
    "flannel.1",
    "cali12ab",
  ])
    expect(interfaceKind(name)).toBe("virtual");
  expect(interfaceKind("lo")).toBe("loopback");
});

it("leads with uplinks then VPNs, busiest first, and folds the rest", () => {
  const traffic = (n: { total: number }) => n.total;
  const { primary, folded } = splitInterfaces(
    [
      { name: "lo", total: 900 },
      { name: "wg0", total: 50 },
      { name: "veth9", total: 10 },
      { name: "eth1", total: 5 },
      { name: "docker0", total: 400 },
      { name: "eth0", total: 300 },
    ],
    traffic,
  );
  expect(primary.map((n) => n.name)).toEqual(["eth0", "eth1", "wg0"]);
  expect(folded.map((n) => n.name)).toEqual(["lo", "docker0", "veth9"]);
});
