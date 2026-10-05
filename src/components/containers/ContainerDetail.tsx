import { useState } from "react";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import {
  LoaderCircle,
  MoreHorizontal,
  OctagonAlert,
  Pause,
  Play,
  RotateCw,
  Square,
  TerminalSquare,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { cn } from "../../lib/utils";
import { Button } from "../controls";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { useCompose, useRunningJob } from "./compose";
import { DetailTab, DetailTabs, Fact, enter } from "../master/Detail";
import { LogToolbar, LogViewer, type LogLine } from "../logs/LogViewer";
import { InspectPanel } from "./InspectPanel";
import { PortLinks } from "./PortLinks";
import { Confirm, Spark, statsHistory } from "./parts";
import { bytes } from "../../overviewHealth";
import { useWorkspaceState } from "../../state/workspace";
import {
  useContainerActions,
  useContainerInspect,
  useContainerLogs,
  usePendingActions,
  type ContainerLogTarget,
} from "../../hooks/useContainers";
import { queueTerminalCommand } from "../../terminal/pending";
import {
  composeWords,
  displayName,
  duration,
  formatCpu,
  logTone,
  problemReason,
  projectKey,
  shellCommand,
  stateOf,
  type Container,
  type ContainerLogEntry,
  type ContainerVerb,
  type ComposeAction,
} from "../../domain/containers";

const toneText = {
  success: "text-(--success)",
  warning: "text-(--warning)",
  danger: "text-(--danger)",
  muted: "text-muted-foreground",
} as const;
const PROGRESS: Record<ContainerVerb | "remove", string> = {
  start: "Starting…",
  stop: "Stopping…",
  restart: "Restarting…",
  pause: "Pausing…",
  unpause: "Resuming…",
  remove: "Removing…",
};
const DONE: Record<ContainerVerb, string> = {
  start: "Started",
  stop: "Stopped",
  restart: "Restarted",
  pause: "Paused",
  unpause: "Resumed",
};

/** Lines from a log with no levels, toned by the words in them. */
export function toLines(entries: ContainerLogEntry[]): LogLine[] {
  return entries.map((e) => ({
    at: e.at,
    tone: logTone(e.message),
    source: e.container,
    message: e.message,
  }));
}
export const levelFilter = (level: "all" | "warning" | "err") =>
  level === "all"
    ? undefined
    : level === "err"
      ? (line: LogLine) => line.tone === "danger"
      : (line: LogLine) => line.tone !== "muted";

/** Live logs for a container or a project, with the page's log controls. */
export function ContainerLogs({
  id,
  target,
  label,
}: {
  id: string;
  target: ContainerLogTarget;
  label: string;
}) {
  const [level] = useWorkspaceState(id, "containers.logLevel");
  const [live] = useWorkspaceState(id, "containers.live");
  const [find] = useWorkspaceState(id, "containers.logQuery");
  const logs = useContainerLogs(id, target, live);
  return (
    <LogViewer
      lines={toLines(logs.data?.entries ?? [])}
      filter={levelFilter(level)}
      dataset={target.container ?? `p:${target.project}`}
      pending={logs.isPending}
      error={logs.error}
      live={live}
      find={find}
      label={label}
      empty="No log output yet."
      filtered={
        level === "err"
          ? "No error lines in the latest output."
          : "No warnings or errors in the latest output."
      }
    />
  );
}
export function ContainerLogToolbar({ id }: { id: string }) {
  const [level, setLevel] = useWorkspaceState(id, "containers.logLevel");
  const [live, setLive] = useWorkspaceState(id, "containers.live");
  const [find, setFind] = useWorkspaceState(id, "containers.logQuery");
  return (
    <LogToolbar
      live={live}
      onLive={setLive}
      level={level}
      onLevel={setLevel}
      find={find}
      onFind={setFind}
    />
  );
}

function Actions({
  id,
  container: c,
  podman,
}: {
  id: string;
  container: Container;
  podman: boolean;
}) {
  const navigate = useNavigate();
  const action = useContainerActions(id);
  const pending = usePendingActions(id);
  const [confirming, setConfirming] = useState<"stop" | "remove" | null>(null);
  const busy = pending.some(
    (a) =>
      (a.kind === "container" && a.container === c.id) ||
      (a.kind === "remove" && a.containers.includes(c.id)) ||
      (a.kind === "project" && a.project === c.compose?.project),
  );
  const mine =
    action.variables &&
    ((action.variables.kind === "container" &&
      action.variables.container === c.id) ||
      (action.variables.kind === "remove" &&
        action.variables.containers.includes(c.id)));
  const error = mine && action.error ? String(action.error) : null;
  const run = (verb: ContainerVerb) =>
    action.mutate(
      { kind: "container", container: c.id, name: c.name, verb },
      {
        onSuccess: () => {
          setConfirming(null);
          toast(`${DONE[verb]} ${displayName(c)}`);
        },
      },
    );
  const remove = () =>
    action.mutate(
      { kind: "remove", containers: [c.id], names: [c.name] },
      {
        onSuccess: () => {
          setConfirming(null);
          toast(`Removed ${c.name}`);
        },
      },
    );
  const service = c.compose && !c.compose.oneoff ? c.compose.service : null;
  const compose = useCompose(id, c.compose?.project ?? "", !!service);
  const builds = compose.info.data?.services?.find(
    (s) => s.name === service,
  )?.build;
  // Compose's own per-service actions, when the server has the CLI.
  const composeItems: ComposeAction[] =
    service && compose.cli
      ? [
          {
            kind: "up",
            services: [service],
            build: false,
            pull: false,
            forceRecreate: true,
          },
          {
            kind: "up",
            services: [service],
            build: false,
            pull: true,
            forceRecreate: true,
          },
          ...(builds
            ? [
                {
                  kind: "up" as const,
                  services: [service],
                  build: true,
                  pull: false,
                  forceRecreate: true,
                },
              ]
            : []),
        ]
      : [];
  const up = c.state === "running" || c.state === "restarting";
  const paused = c.state === "paused";
  const loading = (verb: ContainerVerb) =>
    busy &&
    action.variables?.kind === "container" &&
    action.variables.verb === verb;
  return (
    <div className="flex flex-col gap-[8px]">
      <div className="flex flex-wrap items-center gap-[6px]">
        {up || paused ? (
          <Button
            variant="default"
            loading={loading("restart")}
            disabled={busy}
            onClick={() => run("restart")}
          >
            <RotateCw size={13} /> Restart
          </Button>
        ) : (
          <Button
            variant="default"
            loading={loading("start")}
            disabled={busy}
            onClick={() => run("start")}
          >
            <Play size={13} /> Start
          </Button>
        )}
        {(up || paused) && (
          <Button
            loading={loading("stop")}
            disabled={busy}
            onClick={() => setConfirming("stop")}
          >
            <Square size={12} /> Stop
          </Button>
        )}
        {paused ? (
          <Button
            loading={loading("unpause")}
            disabled={busy}
            onClick={() => run("unpause")}
          >
            <Play size={13} /> Resume
          </Button>
        ) : (
          <Button
            disabled={c.state !== "running"}
            title={
              c.state === "running"
                ? "Open a shell inside this container in the Terminal"
                : "Start the container to open a shell in it"
            }
            onClick={() => {
              queueTerminalCommand(id, shellCommand(c, podman), {
                run: true,
                title: displayName(c),
              });
              void navigate(`/servers/${encodeURIComponent(id)}/terminal`);
            }}
          >
            <TerminalSquare size={13} /> Shell
          </Button>
        )}
        {!up && !paused && (
          <Button
            variant="ghost"
            disabled={busy || c.state === "removing"}
            onClick={() => setConfirming("remove")}
          >
            <Trash2 size={13} /> Remove
          </Button>
        )}
        {(c.state === "running" || composeItems.length > 0) && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label={`More actions for ${c.name}`}
              >
                <MoreHorizontal size={15} />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {c.state === "running" && (
                <DropdownMenuItem disabled={busy} onSelect={() => run("pause")}>
                  <Pause size={14} /> Pause
                </DropdownMenuItem>
              )}
              {c.state === "running" && composeItems.length > 0 && (
                <DropdownMenuSeparator />
              )}
              {composeItems.map((a) => (
                <DropdownMenuItem
                  key={composeWords(a).label}
                  disabled={busy || compose.busy}
                  onSelect={() => compose.start(a, false)}
                >
                  {composeWords(a).label}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </div>
      {compose.error && (
        <p
          role="alert"
          className="text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
        >
          {compose.error}
        </p>
      )}
      {error && !confirming && (
        <p
          role="alert"
          className="text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
        >
          {error}
        </p>
      )}
      {confirming === "stop" && (
        <Confirm
          title={`Stop ${displayName(c)}?`}
          action="Stop"
          busy={busy}
          error={error}
          onConfirm={() => run("stop")}
          onClose={() => setConfirming(null)}
        >
          <p>
            This stops <strong>{c.name}</strong>. Anything using it stops
            working until it is started again.
            {c.restartPolicy === "always" &&
              " Its restart policy is “always”, so Docker starts it again when the daemon restarts."}
          </p>
        </Confirm>
      )}
      {confirming === "remove" && (
        <Confirm
          title={`Remove ${c.name}?`}
          action="Remove"
          busy={busy}
          error={error}
          onConfirm={remove}
          onClose={() => setConfirming(null)}
        >
          <p>
            The stopped container and its logs are deleted. Its image and
            volumes stay.
            {c.compose &&
              !c.compose.oneoff &&
              " Compose creates it again on the next “docker compose up”."}
          </p>
        </Confirm>
      )}
    </div>
  );
}

export function ContainerDetail({
  id,
  container: c,
  onSelect,
  now,
  podman = false,
}: {
  id: string;
  container: Container;
  onSelect: (key: string) => void;
  now: number;
  /** The engine is Podman: its CLI is `podman`, not `docker`. */
  podman?: boolean;
}) {
  const [tab, setTab] = useWorkspaceState(id, "containers.detailTab");
  const state = stateOf(c);
  const reason = problemReason(c);
  const pending = usePendingActions(id).find(
    (a) =>
      (a.kind === "container" && a.container === c.id) ||
      (a.kind === "remove" && a.containers.includes(c.id)),
  );
  // A Compose command on its project (Recreate, Up, Down…) is in progress.
  const composing = useRunningJob(id, c.compose?.project ?? "");
  const [, setProjectTab] = useWorkspaceState(id, "containers.projectTab");
  const progress = pending
    ? PROGRESS[pending.kind === "container" ? pending.verb : "remove"]
    : composing
      ? `Compose is working on ${c.compose?.project}…`
      : null;
  // The last failed check says why it is unhealthy.
  const inspect = useContainerInspect(id, c.id, c.health === "unhealthy");
  const lastCheck =
    c.health === "unhealthy"
      ? inspect.data?.lastHealth.filter((h) => h.exitCode !== 0).at(-1)
      : undefined;
  const running = c.state === "running";
  const history = statsHistory(id, c);
  const since = running ? c.startedAt : c.finishedAt;
  return (
    <article
      aria-label={c.name}
      className={cn("flex min-w-0 flex-1 flex-col gap-[14px]", enter)}
    >
      <header>
        <h2 className="flex-wrap text-[17px] wrap-anywhere">
          {displayName(c)}
          {c.compose && (
            <button
              type="button"
              onClick={() => onSelect(projectKey(c.compose!.project))}
              className="rounded-[4px] bg-muted px-[5px] text-[10.5px] font-normal hover:text-foreground"
            >
              {c.compose.project}
            </button>
          )}
        </h2>
        <p className="mt-[2px] text-[12px] text-muted-foreground wrap-anywhere">
          {c.compose && !c.compose.oneoff ? `${c.name} · ` : ""}
          <span className="font-mono text-[11.5px]">{c.image}</span>
        </p>
        <p
          role="status"
          className={cn(
            "mt-[8px] flex flex-wrap items-center gap-x-[8px] text-[12.5px] transition-colors motion-reduce:transition-none",
            progress ? "text-muted-foreground" : toneText[state.tone],
          )}
        >
          {progress ? (
            <span className="inline-flex items-center gap-[6px] font-semibold">
              <LoaderCircle
                size={13}
                aria-hidden="true"
                className="animate-spin motion-reduce:animate-none"
              />
              {progress}
            </span>
          ) : (
            <span className="font-semibold">{state.label}</span>
          )}
          {composing && !pending && (
            <button
              type="button"
              onClick={() => {
                setProjectTab("activity");
                onSelect(projectKey(c.compose!.project));
              }}
              className="text-primary underline-offset-2 hover:underline"
            >
              View output
            </button>
          )}
          {since && !progress && (
            <span className="text-muted-foreground">
              {running ? "for" : "since"}{" "}
              {running ? duration(now - since) : `${duration(now - since)} ago`}
              {!running && c.exitCode !== null && ` · exit code ${c.exitCode}`}
            </span>
          )}
        </p>
      </header>
      {reason && (
        <div
          className={cn(
            "flex items-start gap-[8px] rounded-[8px] border px-[12px] py-[8px] text-[12.5px]",
            state.tone === "danger"
              ? "border-(--danger)/30 bg-(--danger)/5"
              : "border-(--warning)/30 bg-(--warning)/5",
          )}
        >
          {state.tone === "danger" ? (
            <OctagonAlert
              size={15}
              aria-hidden="true"
              className="mt-[1px] shrink-0 text-(--danger)"
            />
          ) : (
            <TriangleAlert
              size={15}
              aria-hidden="true"
              className="mt-[1px] shrink-0 text-(--warning)"
            />
          )}
          <span className="min-w-0">
            <strong className="font-semibold">{reason}.</strong>{" "}
            {c.restartCount > 0 &&
              c.state !== "restarting" &&
              `Restarted ${c.restartCount}× so far. `}
            {lastCheck ? (
              <span className="mt-[2px] block font-mono text-[11.5px] wrap-anywhere text-muted-foreground">
                {lastCheck.output.trim() ||
                  `check exited ${lastCheck.exitCode}`}
              </span>
            ) : (
              "Its last lines are in the logs below."
            )}
          </span>
        </div>
      )}
      <Actions id={id} container={c} podman={podman} />
      <dl className="m-0 flex flex-wrap gap-x-[28px] gap-y-[10px]">
        {running && (
          <>
            <Fact
              label="CPU"
              detail={
                <Spark
                  values={history.cpu}
                  max={100}
                  label={`CPU over the last ${history.cpu.length * 5} seconds`}
                />
              }
            >
              {c.cpu !== null ? formatCpu(c.cpu) : "—"}
            </Fact>
            <Fact
              label="Memory"
              detail={
                <Spark
                  values={history.memory}
                  max={c.memoryLimit ?? 0}
                  label={`Memory over the last ${history.memory.length * 5} seconds`}
                />
              }
            >
              {c.memory !== null ? bytes(c.memory) : "—"}
              {c.memoryLimit !== null && (
                <span className="text-muted-foreground">
                  {" "}
                  of {bytes(c.memoryLimit)}
                </span>
              )}
            </Fact>
          </>
        )}
        {c.restartCount > 0 && <Fact label="Restarts">{c.restartCount}</Fact>}
        <Fact label="Restart policy">
          {c.restartPolicy && c.restartPolicy !== "no"
            ? c.restartPolicy
            : "never"}
        </Fact>
        <Fact label="Created">{duration(now - c.created)} ago</Fact>
      </dl>
      <PortLinks serverId={id} container={c} />
      <DetailTabs
        value={tab}
        onValueChange={setTab}
        tabs={[
          { value: "logs", label: "Logs" },
          { value: "inspect", label: "Inspect" },
        ]}
        side={tab === "logs" ? <ContainerLogToolbar id={id} /> : null}
      >
        <DetailTab value="logs">
          <ContainerLogs
            id={id}
            target={{ container: c.id }}
            label={`${c.name} logs`}
          />
        </DetailTab>
        <DetailTab value="inspect">
          <InspectPanel id={id} container={c} />
        </DetailTab>
      </DetailTabs>
    </article>
  );
}
