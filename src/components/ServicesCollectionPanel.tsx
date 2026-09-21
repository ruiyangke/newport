import { useRef } from "react";
import { ServicesPanel as ServicesInventory } from "./ServicesPanel";
import { type Log } from "../domain/cockpit";
import { useCollection } from "../hooks/useCollection";

export function ServicesCollectionPanel({
  id,
  onLogs,
}: {
  id: string;
  onLogs: (log: Log, origin: HTMLElement) => void;
}) {
  const scopeRef = useRef<HTMLDivElement>(null);
  const state = useCollection(id, "services", false, scopeRef);
  return (
    <div ref={scopeRef}>
      <ServicesInventory state={state} onLogs={onLogs} />
    </div>
  );
}
