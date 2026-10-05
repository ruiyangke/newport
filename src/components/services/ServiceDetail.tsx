import { lazy, Suspense, useState } from "react";
import { useMutationState } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import {
  LoaderCircle,
  OctagonAlert,
  Play,
  RotateCw,
  Square,
  TerminalSquare,
  TriangleAlert,
} from "lucide-react";
import { cn } from "../../lib/utils";
import { Button } from "../controls";
import { Switch } from "../ui/switch";
import { Modal } from "../Editors";
import { EditorBody, EditorFooter } from "../EditorLayout";
import { LogToolbar, LogView } from "./ServiceLogs";
import { DetailTab, DetailTabs, Fact, enter } from "../master/Detail";
import { Skeleton } from "../ui/skeleton";
import { useWorkspaceState } from "../../state/workspace";
import { timerNote } from "./ServiceList";
import { bytes } from "../../overviewHealth";
import {
  availableVerbs,
  duration,
  failureReason,
  shortName,
  stateOf,
  unitKey,
  type Privilege,
  type ServiceUnit,
  type ServiceVerb,
  type TimerUnit,
} from "../../domain/services";
import {
  useServiceAction,
  useUnitFile,
  type LogTarget,
  type ServiceAction,
} from "../../hooks/useServices";
import { queueTerminalCommand } from "../../terminal/pending";

const UnitFileView = lazy(() => import("../GitBlobEditor"));

const VERB_PAST: Record<ServiceVerb, string> = {
  start: "Started",
  stop: "Stopped",
  restart: "Restarted",
  reload: "Reloaded",
  enable: "Will start at boot:",
  disable: "Won’t start at boot:",
};
// Stopping or disabling can take a live service away; they ask first.
const CONFIRM: Partial<Record<ServiceVerb, string>> = {
  stop: "Stop",
  disable: "Don’t start at boot",
};

const toneText = {
  success: "text-(--success)",
  warning: "text-(--warning)",
  danger: "text-(--danger)",
  muted: "text-muted-foreground",
} as const;

/** Why the controls are off, when this account cannot manage the unit. */
function blockedReason(unit: ServiceUnit, privilege: Privilege) {
  if (unit.scope === "user" || privilege !== "none") return null;
  return "This account can’t manage system services: it isn’t root and has no sudo.";
}

function Actions({
  id,
  unit,
  privilege,
}: {
  id: string;
  unit: ServiceUnit;
  privilege: Privilege;
}) {
  const navigate = useNavigate();
  const action = useServiceAction(id);
  const [confirming, setConfirming] = useState<ServiceVerb | null>(null);
  const [handover, setHandover] = useState<string | null>(null);
  const verbs = availableVerbs(unit);
  const blocked = blockedReason(unit, privilege);
  const busyVerb =
    action.isPending && action.variables?.unit === unit.name
      ? action.variables.verb
      : null;
  const run = (verb: ServiceVerb) => {
    setHandover(null);
    const request: ServiceAction = { unit: unit.name, scope: unit.scope, verb };
    action.mutate(request, {
      onSuccess: (result) => {
        setConfirming(null);
        if (result.ok) toast(`${VERB_PAST[verb]} ${shortName(unit.name)}`);
        else setHandover(result.command);
      },
    });
  };
  const ask = (verb: ServiceVerb) =>
    CONFIRM[verb] ? setConfirming(verb) : run(verb);
  const error =
    action.isError && action.variables?.unit === unit.name
      ? String(action.error)
      : null;
  const running = !verbs.start;
  return (
    <div className="flex flex-col gap-[8px]">
      <div className="flex flex-wrap items-center gap-[6px]">
        {running ? (
          <Button
            variant="default"
            loading={busyVerb === "restart"}
            disabled={!!blocked || action.isPending}
            onClick={() => ask("restart")}
          >
            <RotateCw size={13} /> Restart
          </Button>
        ) : (
          <Button
            variant="default"
            loading={busyVerb === "start"}
            disabled={!!blocked || action.isPending}
            onClick={() => ask("start")}
          >
            <Play size={13} /> Start
          </Button>
        )}
        {verbs.stop && (
          <Button
            loading={busyVerb === "stop"}
            disabled={!!blocked || action.isPending}
            onClick={() => ask("stop")}
          >
            <Square size={12} /> Stop
          </Button>
        )}
        {!running && verbs.restart && unit.active === "failed" && (
          <Button
            loading={busyVerb === "restart"}
            disabled={!!blocked || action.isPending}
            onClick={() => ask("restart")}
          >
            <RotateCw size={13} /> Restart
          </Button>
        )}
        {verbs.reload && (
          <Button
            variant="ghost"
            loading={busyVerb === "reload"}
            disabled={!!blocked || action.isPending}
            onClick={() => ask("reload")}
            title="Ask the service to reload its configuration without restarting"
          >
            Reload
          </Button>
        )}
        {(verbs.enable || verbs.disable) && (
          <label className="ml-auto flex items-center gap-[7px] text-[12px] text-muted-foreground">
            <Switch
              aria-label="Start at boot"
              checked={unit.enabled === "enabled"}
              disabled={!!blocked || action.isPending}
              onCheckedChange={(on) => ask(on ? "enable" : "disable")}
            />
            Start at boot
          </label>
        )}
      </div>
      {blocked && (
        <p className="text-[11.5px] text-muted-foreground">{blocked}</p>
      )}
      {handover && (
        <div
          role="alert"
          className="flex flex-wrap items-center gap-[10px] rounded-[8px] border border-(--warning)/35 bg-(--warning)/6 px-[10px] py-[7px] text-[12px]"
        >
          <span className="min-w-0 flex-1">
            This needs your sudo password. Run{" "}
            <code className="text-[11.5px]">{handover}</code> in the Terminal.
          </span>
          <Button
            onClick={() => {
              queueTerminalCommand(id, handover);
              setHandover(null);
              void navigate(`/servers/${encodeURIComponent(id)}/terminal`);
            }}
          >
            <TerminalSquare size={13} /> Open in Terminal
          </Button>
        </div>
      )}
      {error && !confirming && (
        <p
          role="alert"
          className="text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
        >
          {error}
        </p>
      )}
      {confirming && (
        <Modal
          title={`${CONFIRM[confirming]} ${shortName(unit.name)}?`}
          busy={action.isPending}
          onClose={() => setConfirming(null)}
          describedBy="service-action-description"
        >
          <EditorBody>
            <p id="service-action-description">
              {confirming === "stop"
                ? `This stops ${unit.name}${unit.scope === "user" ? " (user service)" : ""}. Anything relying on it stops working until it is started again.`
                : `${unit.name} will no longer start when the server boots. It keeps running until it is stopped.`}
            </p>
            {error && (
              <p
                role="alert"
                className="text-[12px] text-(--danger) wrap-anywhere"
              >
                {error}
              </p>
            )}
          </EditorBody>
          <EditorFooter>
            <Button
              disabled={action.isPending}
              onClick={() => setConfirming(null)}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              loading={action.isPending}
              onClick={() => run(confirming)}
            >
              {CONFIRM[confirming]}
            </Button>
          </EditorFooter>
        </Modal>
      )}
    </div>
  );
}

const PROGRESS: Record<ServiceVerb, string> = {
  start: "Starting…",
  stop: "Stopping…",
  restart: "Restarting…",
  reload: "Reloading…",
  enable: "Enabling at boot…",
  disable: "Disabling at boot…",
};

/** The action running on this unit right now, from the shared mutation cache. */
function usePendingVerb(id: string, unit: ServiceUnit) {
  const pending = useMutationState({
    filters: { mutationKey: ["service-action", id], status: "pending" },
    select: (mutation) => mutation.state.variables as ServiceAction,
  });
  return (
    pending.find((a) => a.unit === unit.name && a.scope === unit.scope)?.verb ??
    null
  );
}

const FILE_SKELETON = [38, 22, 46, 30, 0, 18, 64, 40, 34, 0, 20, 52];

function UnitFile({ id, unit }: { id: string; unit: ServiceUnit | TimerUnit }) {
  const file = useUnitFile(id, unit.name, unit.scope, true);
  const skeleton = (
    <div
      aria-label="Reading unit file"
      className="flex flex-col gap-[8px] p-[12px]"
    >
      {FILE_SKELETON.map((width, i) =>
        width ? (
          <Skeleton
            key={i}
            className="h-[11px] rounded-[3px]"
            style={{ width: `${width}%` }}
          />
        ) : (
          <span key={i} className="h-[11px]" />
        ),
      )}
    </div>
  );
  return (
    <div className="relative min-h-0 flex-1">
      <div className="absolute inset-0 overflow-auto rounded-[8px] border border-border text-[12px]">
        {file.error ? (
          <p role="alert" className="p-[10px] text-(--danger) wrap-anywhere">
            {String(file.error)}
          </p>
        ) : file.data ? (
          <Suspense fallback={skeleton}>
            <div className="animate-in duration-200 fade-in motion-reduce:animate-none">
              <UnitFileView
                text={file.data.text}
                label={`${unit.name} unit file`}
              />
            </div>
          </Suspense>
        ) : (
          skeleton
        )}
      </div>
    </div>
  );
}

function UnitPath({ unit }: { unit: ServiceUnit | TimerUnit }) {
  const path = "fragment" in unit ? unit.fragment : null;
  const dropIns = "dropIns" in unit ? unit.dropIns.length : 0;
  if (!path) return null;
  return (
    <span
      title={path}
      className="min-w-0 truncate text-[11.5px] text-muted-foreground"
    >
      <span className="font-mono">{path}</span>
      {dropIns > 0 && ` + ${dropIns} drop-in${dropIns === 1 ? "" : "s"}`}
    </span>
  );
}

/**
 * Logs and the unit file share the rest of the pane. The tab bar carries the
 * log controls, so the lines get every remaining pixel.
 */
function UnitTabs({
  id,
  unit,
  logs,
  logLabel,
}: {
  id: string;
  unit: ServiceUnit | TimerUnit;
  logs: LogTarget | null;
  logLabel: string;
}) {
  const [tab, setTab] = useWorkspaceState(id, "services.detailTab");
  const active = logs ? tab : "unit";
  return (
    <DetailTabs
      value={active}
      onValueChange={setTab}
      tabs={[
        ...(logs ? [{ value: "logs" as const, label: "Logs" }] : []),
        { value: "unit" as const, label: "Unit file" },
      ]}
      side={
        active === "logs" ? <LogToolbar id={id} /> : <UnitPath unit={unit} />
      }
    >
      {logs && (
        <DetailTab value="logs">
          <LogView id={id} target={logs} label={logLabel} />
        </DetailTab>
      )}
      <DetailTab value="unit">
        <UnitFile id={id} unit={unit} />
      </DetailTab>
    </DetailTabs>
  );
}

export function ServiceDetail({
  id,
  unit,
  privilege,
  timers,
  onSelect,
  now,
}: {
  id: string;
  unit: ServiceUnit;
  privilege: Privilege;
  timers: TimerUnit[];
  onSelect: (key: string) => void;
  now: number;
}) {
  const state = stateOf(unit);
  const reason = failureReason(unit);
  const pending = usePendingVerb(id, unit);
  const running = unit.active === "active" && unit.sub === "running";
  const triggers = timers.filter(
    (t) => t.unit === unit.name && t.scope === unit.scope,
  );
  return (
    <article
      aria-label={unit.name}
      className={cn("flex min-w-0 flex-1 flex-col gap-[14px]", enter)}
    >
      <header>
        <h2 className="flex-wrap text-[17px] wrap-anywhere">
          {unit.name}
          {unit.scope === "user" && (
            <span className="rounded-[4px] bg-muted px-[5px] text-[10.5px]">
              user service
            </span>
          )}
        </h2>
        {unit.description && (
          <p className="mt-[2px] text-[12px] text-muted-foreground">
            {unit.description}
          </p>
        )}
        <p
          role="status"
          className={cn(
            "mt-[8px] flex flex-wrap items-center gap-x-[8px] text-[12.5px] transition-colors motion-reduce:transition-none",
            pending ? "text-muted-foreground" : toneText[state.tone],
          )}
        >
          {pending ? (
            <span className="inline-flex items-center gap-[6px] font-semibold">
              <LoaderCircle
                size={13}
                aria-hidden="true"
                className="animate-spin motion-reduce:animate-none"
              />
              {PROGRESS[pending]}
            </span>
          ) : (
            <span className="font-semibold">{state.label}</span>
          )}
          {unit.since && !pending && (
            <span className="text-muted-foreground">
              {running ? "for" : "since"}{" "}
              {running
                ? duration(now - unit.since)
                : `${duration(now - unit.since)} ago`}{" "}
              <span title={new Date(unit.since).toLocaleString()}>
                (
                {new Date(unit.since).toLocaleString([], {
                  dateStyle: "medium",
                  timeStyle: "short",
                })}
                )
              </span>
            </span>
          )}
        </p>
      </header>
      {reason && (
        <div
          className={cn(
            "flex items-start gap-[8px] rounded-[8px] border px-[12px] py-[8px] text-[12.5px]",
            unit.active === "failed"
              ? "border-(--danger)/30 bg-(--danger)/5"
              : "border-(--warning)/30 bg-(--warning)/5",
          )}
        >
          {unit.active === "failed" ? (
            <OctagonAlert
              size={15}
              aria-hidden="true"
              className="mt-[1px] text-(--danger)"
            />
          ) : (
            <TriangleAlert
              size={15}
              aria-hidden="true"
              className="mt-[1px] text-(--warning)"
            />
          )}
          <span>
            <strong className="font-semibold">{reason}.</strong>{" "}
            {unit.restarts ? `Restarted ${unit.restarts}× so far. ` : ""}
            The error lines are highlighted in the logs below.
          </span>
        </div>
      )}
      <Actions id={id} unit={unit} privilege={privilege} />
      <dl className="m-0 flex flex-wrap gap-x-[28px] gap-y-[10px]">
        {unit.mainPid !== null && (
          <>
            <Fact label="Memory">
              {unit.memory !== null ? bytes(unit.memory) : "—"}
            </Fact>
            <Fact label="CPU">
              {unit.cpu !== null ? `${unit.cpu.toFixed(1)}%` : "—"}
            </Fact>
            <Fact label="Main PID">{unit.mainPid}</Fact>
          </>
        )}
        <Fact label="Runs as">
          {unit.user || (unit.scope === "user" ? "you" : "root")}
        </Fact>
        {!!unit.restarts && <Fact label="Restarts">{unit.restarts}</Fact>}
        <Fact label="At boot">
          {unit.enabled === "enabled"
            ? "Starts"
            : unit.enabled === "disabled"
              ? "Doesn’t start"
              : unit.enabled === "static"
                ? "Started by another unit"
                : unit.enabled === "masked"
                  ? "Masked"
                  : (unit.enabled ?? "—")}
        </Fact>
        {(unit.triggeredBy.length > 0 || triggers.length > 0) && (
          <Fact label="Triggered by">
            {[
              ...new Set([...unit.triggeredBy, ...triggers.map((t) => t.name)]),
            ].join(", ")}
          </Fact>
        )}
      </dl>
      {triggers.length > 0 && (
        <p className="-mt-[4px] text-[12px] text-muted-foreground">
          {triggers.map((t) => (
            <button
              key={unitKey(t)}
              type="button"
              onClick={() => onSelect(unitKey(t))}
              className="mr-[10px] underline-offset-2 hover:text-foreground hover:underline"
            >
              {shortName(t.name)} timer · next {timerNote(t, now)}
            </button>
          ))}
        </p>
      )}
      <UnitTabs
        id={id}
        unit={unit}
        logs={{ unit: unit.name, scope: unit.scope }}
        logLabel={`${unit.name} logs`}
      />
    </article>
  );
}

export function TimerDetail({
  id,
  timer,
  service,
  onSelect,
  now,
}: {
  id: string;
  timer: TimerUnit;
  service: ServiceUnit | undefined;
  onSelect: (key: string) => void;
  now: number;
}) {
  const at = (value: number | null) =>
    value
      ? new Date(value).toLocaleString([], {
          dateStyle: "medium",
          timeStyle: "short",
        })
      : "—";
  return (
    <article
      aria-label={timer.name}
      className={cn("flex min-w-0 flex-1 flex-col gap-[14px]", enter)}
    >
      <header>
        <h2 className="text-[17px] wrap-anywhere">{timer.name}</h2>
        {timer.description && (
          <p className="mt-[2px] text-[12px] text-muted-foreground">
            {timer.description}
          </p>
        )}
      </header>
      <dl className="m-0 flex flex-wrap gap-x-[28px] gap-y-[10px]">
        <Fact label="Next run" detail={timer.next ? at(timer.next) : undefined}>
          {timer.next
            ? timer.next > now
              ? `in ${duration(timer.next - now)}`
              : "Due now"
            : "Not scheduled"}
        </Fact>
        <Fact label="Last run" detail={timer.last ? at(timer.last) : undefined}>
          {timer.last ? `${duration(now - timer.last)} ago` : "Never"}
        </Fact>
        <Fact label="Runs">
          {timer.unit ? (
            service ? (
              <button
                type="button"
                onClick={() => onSelect(unitKey(service))}
                className="underline-offset-2 hover:underline"
              >
                {timer.unit} · {stateOf(service).label.toLowerCase()}
              </button>
            ) : (
              timer.unit
            )
          ) : (
            "—"
          )}
        </Fact>
      </dl>
      <UnitTabs
        id={id}
        unit={timer}
        logs={timer.unit ? { unit: timer.unit, scope: timer.scope } : null}
        logLabel={`Logs of ${timer.unit}`}
      />
    </article>
  );
}
