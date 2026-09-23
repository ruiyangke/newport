import type { DestinationHealth } from "../types";

export function destinationSummary(health: DestinationHealth[]) {
  if (!health.length) return "Checking destination…";
  const reachable = health.filter((h) => h.status === "reachable").length;
  if (reachable === health.length)
    return health.length > 1
      ? "All destinations reachable"
      : "Destination reachable";
  if (reachable > 0)
    return `${reachable}/${health.length} destinations reachable`;
  if (health.some((h) => h.status === "unavailable"))
    return "Destination unavailable";
  if (health.some((h) => h.status === "blocked")) return "Forwarding blocked";
  if (health.some((h) => h.status === "unknown"))
    return "Destination check inconclusive";
  return "Checking destination…";
}
export function DestinationStatus({
  health = [],
}: {
  health?: DestinationHealth[];
}) {
  const issues = health.filter((h) => h.message);
  const healthy =
    health.length > 0 && health.every((h) => h.status === "reachable");
  return (
    <div
      className={`destination-health ${healthy ? "healthy" : issues.length ? "attention" : ""}`}
    >
      <span>{destinationSummary(health)}</span>
      {issues.length > 0 && (
        <details>
          <summary>Destination details</summary>
          {issues.map((h) => (
            <p key={h.remotePort}>
              Port {h.remotePort}: {h.message}
              {h.checkedAt && (
                <small>
                  Checked {new Date(h.checkedAt * 1000).toLocaleTimeString()}
                </small>
              )}
            </p>
          ))}
        </details>
      )}
    </div>
  );
}
