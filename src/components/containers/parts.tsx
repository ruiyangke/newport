import { useId, type ReactNode } from "react";
import { Button } from "../controls";
import { Modal } from "../Editors";
import { EditorBody, EditorFooter } from "../EditorLayout";
import type { Container, ContainersSnapshot } from "../../domain/containers";

/** A question before something that takes a running thing away. */
export function Confirm({
  title,
  action,
  busy,
  error,
  onConfirm,
  onClose,
  destructive = true,
  disabled = false,
  children,
}: {
  title: string;
  action: string;
  busy: boolean;
  error: string | null;
  onConfirm: () => void;
  onClose: () => void;
  destructive?: boolean;
  /** Not yet: something in the dialog still needs doing. */
  disabled?: boolean;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <Modal title={title} busy={busy} onClose={onClose} describedBy={id}>
      <EditorBody>
        <div id={id} className="flex flex-col gap-[10px] text-[12.5px]">
          {children}
        </div>
        {error && (
          <p role="alert" className="text-[12px] text-(--danger) wrap-anywhere">
            {error}
          </p>
        )}
      </EditorBody>
      <EditorFooter>
        <Button disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        <Button
          variant={destructive ? "destructive" : "default"}
          loading={busy}
          disabled={disabled}
          onClick={onConfirm}
        >
          {action}
        </Button>
      </EditorFooter>
    </Modal>
  );
}

/** A small single-series area, for a reading's last few minutes. */
export function Spark({
  values,
  max,
  label,
}: {
  values: number[];
  max: number;
  label: string;
}) {
  if (values.length < 2) return null;
  // A fixed top (100 %, a memory limit), else headroom over the peak so an
  // unbounded reading doesn't look full.
  const top =
    max > 0 ? Math.max(max, ...values) : Math.max(...values, 1e-9) * 1.25;
  const step = 60 / (values.length - 1);
  const y = (v: number) => 17 - (v / top) * 15;
  const line = values
    .map(
      (v, i) => `${i ? "L" : "M"}${(i * step).toFixed(2)},${y(v).toFixed(2)}`,
    )
    .join(" ");
  return (
    <svg
      viewBox="0 0 60 18"
      preserveAspectRatio="none"
      role="img"
      aria-label={label}
      className="mt-[3px] block h-[18px] w-[84px] overflow-visible"
    >
      <path
        d={`${line} L60,18 L0,18 Z`}
        fill="var(--series-1)"
        fillOpacity={0.22}
      />
      <path
        d={line}
        fill="none"
        stroke="var(--series-1)"
        strokeWidth={1.25}
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

// A few minutes of readings per container, kept for this session so the
// sparklines don't start over whenever the page is opened again.
const HISTORY = 60;
const history = new Map<string, { cpu: number[]; memory: number[] }>();
const recorded = new Map<string, number>();

/** Adds a snapshot's readings to each container's recent history. */
export function recordStats(
  serverId: string,
  snapshot: ContainersSnapshot | undefined,
  at: number,
) {
  if (!snapshot || recorded.get(serverId) === at) return;
  recorded.set(serverId, at);
  for (const c of snapshot.containers) {
    if (c.state !== "running" || c.cpu === null) continue;
    const key = `${serverId}:${c.id}`;
    const h = history.get(key) ?? { cpu: [], memory: [] };
    h.cpu = [...h.cpu, c.cpu].slice(-HISTORY);
    h.memory = [...h.memory, c.memory ?? 0].slice(-HISTORY);
    history.set(key, h);
  }
}
export const statsHistory = (serverId: string, c: Container) =>
  history.get(`${serverId}:${c.id}`) ?? { cpu: [], memory: [] };
