/**
 * What kind of network interface a name denotes. Container hosts report
 * dozens of virtual links (veth pairs, bridges); people care about the
 * physical uplink and VPNs, so those lead and the rest fold away.
 */
export type InterfaceKind = "physical" | "vpn" | "virtual" | "loopback";

const VIRTUAL =
  /^(veth|br-|docker|virbr|vnet|tap|cni|flannel|cali|cilium|vxlan|weave|kube-|podman|lxc|lxd|lxcbr|ovs-|dummy|ifb|nlmon|gre|sit|ip6tnl)/;
const VPN = /^(tailscale|wg|tun|utun|zt|ppp|ipsec|nebula)/;

export function interfaceKind(name: string): InterfaceKind {
  if (name === "lo" || name.startsWith("lo:")) return "loopback";
  if (VIRTUAL.test(name)) return "virtual";
  if (VPN.test(name)) return "vpn";
  return "physical";
}

/** Whether an interface belongs in the main list rather than folded away. */
export const isPrimary = (name: string) => {
  const kind = interfaceKind(name);
  return kind === "physical" || kind === "vpn";
};

/**
 * Interfaces split for display: primary ones (physical, then VPN) and the
 * folded rest, each busiest first by total traffic.
 */
export function splitInterfaces<T extends { name: string }>(
  interfaces: T[],
  traffic: (item: T) => number,
) {
  const busiest = (a: T, b: T) =>
    traffic(b) - traffic(a) || a.name.localeCompare(b.name);
  const rank = (item: T) => (interfaceKind(item.name) === "physical" ? 0 : 1);
  return {
    primary: interfaces
      .filter((i) => isPrimary(i.name))
      .sort((a, b) => rank(a) - rank(b) || busiest(a, b)),
    folded: interfaces.filter((i) => !isPrimary(i.name)).sort(busiest),
  };
}
