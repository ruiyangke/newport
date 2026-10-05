import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CircleCheck, CircleX, LoaderCircle, Square } from "lucide-react";
import { cn } from "../../lib/utils";
import { Button, Input } from "../controls";
import { Skeleton } from "../ui/skeleton";
import { Switch } from "../ui/switch";
import { LogViewer } from "../logs/LogViewer";
import { Confirm } from "./parts";
import { keys, useServerScope } from "../../query/keys";
import { useWorkspaceState } from "../../state/workspace";
import {
  useComposeCancel,
  useComposeJob,
  useComposeJobs,
  useComposeProject,
  useComposeRun,
} from "../../hooks/useContainers";
import {
  composeWords,
  jobDuration,
  logTone,
  projectKey,
  type ComposeAction,
  type ComposeJob,
} from "../../domain/containers";

const FileView = lazy(() => import("../GitBlobEditor"));

/** The Compose command running for a project, if any. */
export function useRunningJob(id: string, project: string) {
  const jobs = useComposeJobs(id).data?.jobs ?? [];
  return jobs.find((j) => j.project === project && j.state === "running");
}

/**
 * Runs Compose commands for a project. Starting one opens the project's
 * Activity tab, where its output streams; only one runs per project.
 */
export function useCompose(id: string, project: string, enabled = true) {
  const info = useComposeProject(id, project, enabled);
  const run = useComposeRun(id);
  const running = useRunningJob(id, project);
  const [, setTab] = useWorkspaceState(id, "containers.projectTab");
  const [, setSelected] = useWorkspaceState(id, "containers.selected");
  const pending = run.isPending && run.variables?.project === project;
  return {
    info,
    cli: info.data?.cli ?? null,
    busy: pending || !!running,
    running,
    error:
      run.isError && run.variables?.project === project
        ? String(run.error)
        : null,
    /** Starts `action`; `show` opens the project's Activity tab. */
    start: (action: ComposeAction, show = true) =>
      run.mutate(
        { project, action },
        {
          onSuccess: () => {
            if (show) {
              setSelected(projectKey(project));
              setTab("activity");
            } else toast(composeWords(action).progress);
          },
        },
      ),
  };
}

/**
 * Announces each Compose command as it finishes, wherever the person is on
 * the page, and refreshes what it changed.
 */
export function useComposeWatcher(id: string) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  const jobs = useComposeJobs(id).data?.jobs;
  const [, setTab] = useWorkspaceState(id, "containers.projectTab");
  const [, setSelected] = useWorkspaceState(id, "containers.selected");
  const seen = useRef<Map<string, ComposeJob["state"]> | null>(null);
  useEffect(() => {
    if (!jobs) return;
    const before = seen.current;
    seen.current = new Map(jobs.map((j) => [j.job, j.state]));
    if (!before) return;
    for (const job of jobs) {
      if (before.get(job.job) !== "running" || job.state === "running")
        continue;
      void client.invalidateQueries({ queryKey: keys.containers(scope) });
      void client.invalidateQueries({
        queryKey: keys.composeProject(scope, job.project),
      });
      void client.invalidateQueries({ queryKey: keys.dockerDisk(scope) });
      const view = {
        label: "View output",
        onClick: () => {
          setSelected(projectKey(job.project));
          setTab("activity");
        },
      };
      if (job.state === "succeeded")
        toast(`${job.project}: done`, { description: job.command });
      else if (job.state === "failed")
        toast.error(`${job.project}: failed (exit ${job.exitCode ?? "?"})`, {
          description: job.command,
          action: view,
        });
      else toast(`${job.project}: cancelled`, { description: job.command });
    }
  }, [jobs, client, scope, setSelected, setTab]);
}

function JobState({ job, now }: { job: ComposeJob; now: number }) {
  if (job.state === "running")
    return (
      <span className="inline-flex items-center gap-[5px] text-muted-foreground">
        <LoaderCircle
          size={12}
          aria-hidden="true"
          className="animate-spin motion-reduce:animate-none"
        />
        Running · {jobDuration(job, now)}
      </span>
    );
  if (job.state === "succeeded")
    return (
      <span className="inline-flex items-center gap-[5px] text-(--success)">
        <CircleCheck size={12} aria-hidden="true" /> Succeeded in{" "}
        {jobDuration(job, now)}
      </span>
    );
  return (
    <span className="inline-flex items-center gap-[5px] text-(--danger)">
      <CircleX size={12} aria-hidden="true" />
      {job.state === "cancelled"
        ? "Cancelled"
        : `Failed (exit ${job.exitCode ?? "?"})`}{" "}
      after {jobDuration(job, now)}
    </span>
  );
}

/**
 * The project's Compose commands: the latest one's output streaming in,
 * with a way to stop it, and the earlier ones a click away.
 */
export function ComposeActivity({
  id,
  project,
  now,
}: {
  id: string;
  project: string;
  now: number;
}) {
  const jobs = (useComposeJobs(id).data?.jobs ?? []).filter(
    (j) => j.project === project,
  );
  const [chosen, setChosen] = useState<string | null>(null);
  const current = jobs.find((j) => j.job === chosen) ?? jobs[0] ?? null;
  const job = useComposeJob(id, current?.job ?? null);
  const cancel = useComposeCancel(id);
  // The agent stops the process group before the job reports "cancelled".
  const [stopping, setStopping] = useState<string | null>(null);
  const live = job.data ?? current;
  if (!current)
    return (
      <p className="py-[8px] text-[12px] text-muted-foreground">
        Compose commands you run here — Up, Pull, Rebuild, Down, Scale — show
        their output in this tab.
      </p>
    );
  const lines = (job.data?.output ?? "")
    .split("\n")
    .filter((line, i, all) => line || i < all.length - 1)
    .map((message) => ({ at: null, tone: logTone(message), message }));
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-[8px]">
      <div className="flex flex-wrap items-center gap-x-[12px] gap-y-[6px] text-[12px]">
        <code className="min-w-0 flex-1 text-[11.5px] wrap-anywhere">
          {live!.command}
        </code>
        <JobState job={live!} now={now} />
        {live!.state === "running" && (
          <Button
            className="h-[26px] px-[8px] text-[12px]"
            loading={cancel.isPending || stopping === live!.job}
            onClick={() => {
              setStopping(live!.job);
              cancel.mutate(live!.job, { onError: () => setStopping(null) });
            }}
          >
            <Square size={11} />
            {stopping === live!.job ? "Cancelling…" : "Cancel"}
          </Button>
        )}
      </div>
      <LogViewer
        lines={lines}
        dataset={current.job}
        pending={job.isPending}
        error={job.error}
        live={live!.state === "running"}
        find=""
        label={`${project} activity`}
        reading="Starting"
        empty={live!.state === "running" ? "Waiting for output…" : "No output."}
        timestamps={false}
        idle="finished"
      />
      {jobs.length > 1 && (
        <details className="text-[12px]">
          <summary className="cursor-default text-muted-foreground">
            Earlier commands ({jobs.length - 1})
          </summary>
          <ul role="list" className="m-0 mt-[4px] list-none p-0">
            {jobs.map((j) => (
              <li key={j.job}>
                <button
                  type="button"
                  aria-current={j.job === current.job || undefined}
                  onClick={() => setChosen(j.job)}
                  className={cn(
                    "flex w-full items-center gap-[10px] rounded-[5px] px-[6px] py-[3px] text-left hover:bg-accent/60",
                    j.job === current.job && "bg-(--sidebar-accent)",
                  )}
                >
                  <code className="min-w-0 flex-1 truncate text-[11.5px]">
                    {j.command}
                  </code>
                  <JobState job={j} now={now} />
                </button>
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

/** The project's compose files as they are on the server, read-only. */
export function ComposeFiles({ id, project }: { id: string; project: string }) {
  const info = useComposeProject(id, project);
  const [chosen, setChosen] = useState(0);
  if (info.error)
    return (
      <p role="alert" className="text-[12px] text-(--danger) wrap-anywhere">
        {String(info.error)}
      </p>
    );
  if (!info.data)
    return (
      <div
        aria-label="Reading compose files"
        className="flex flex-col gap-[8px]"
      >
        {[38, 22, 46, 30, 54, 26].map((w, i) => (
          <Skeleton
            key={i}
            className="h-[11px] rounded-[3px]"
            style={{ width: `${w}%` }}
          />
        ))}
      </div>
    );
  const { files, configError } = info.data;
  const file = files[Math.min(chosen, files.length - 1)];
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-[8px]">
      {configError && (
        <div
          role="alert"
          className="rounded-[8px] border border-(--danger)/30 bg-(--danger)/5 px-[12px] py-[8px] text-[12px]"
        >
          <strong className="font-semibold">
            Compose can’t read this project’s configuration.
          </strong>
          <pre className="mt-[4px] font-mono text-[11.5px] whitespace-pre-wrap wrap-anywhere text-muted-foreground">
            {configError}
          </pre>
        </div>
      )}
      {files.length > 1 && (
        <div
          role="tablist"
          aria-label="Compose files"
          className="flex flex-wrap gap-[6px]"
        >
          {files.map((f, i) => (
            <button
              key={f.path}
              type="button"
              role="tab"
              aria-selected={i === chosen}
              onClick={() => setChosen(i)}
              className={cn(
                "rounded-[6px] px-[8px] py-[2px] font-mono text-[11.5px]",
                i === chosen
                  ? "bg-(--sidebar-accent)"
                  : "text-muted-foreground hover:bg-accent/60",
              )}
            >
              {f.path.split("/").pop()}
            </button>
          ))}
        </div>
      )}
      {file ? (
        <>
          <p className="font-mono text-[11.5px] text-muted-foreground wrap-anywhere">
            {file.path}
          </p>
          {file.error && (
            <p role="alert" className="text-[12px] text-(--danger)">
              {file.error}
            </p>
          )}
          {file.text !== null && (
            <div className="relative min-h-0 flex-1">
              <div className="absolute inset-0 overflow-auto rounded-[8px] border border-border text-[12px]">
                <Suspense fallback={null}>
                  <FileView text={file.text} label={`${file.path}`} />
                </Suspense>
              </div>
            </div>
          )}
        </>
      ) : (
        <p className="text-[12px] text-muted-foreground">
          No compose file is recorded for this project.
        </p>
      )}
    </div>
  );
}

/**
 * Down, with the volumes question asked plainly: deleting them needs the
 * project's name typed, since the data can't come back.
 */
export function DownDialog({
  project,
  busy,
  error,
  onConfirm,
  onClose,
}: {
  project: string;
  busy: boolean;
  error: string | null;
  onConfirm: (options: { volumes: boolean; removeOrphans: boolean }) => void;
  onClose: () => void;
}) {
  const [volumes, setVolumes] = useState(false);
  const [orphans, setOrphans] = useState(false);
  const [typed, setTyped] = useState("");
  const blocked = volumes && typed !== project;
  return (
    <Confirm
      title={`Take ${project} down?`}
      action={volumes ? "Down & delete volumes" : "Down"}
      busy={busy}
      error={error}
      disabled={blocked}
      onConfirm={() =>
        !blocked && onConfirm({ volumes, removeOrphans: orphans })
      }
      onClose={onClose}
    >
      <p>
        The project’s containers and networks are removed. Images stay, and{" "}
        <strong>Up</strong> creates everything again from the compose files.
      </p>
      <label className="flex items-center gap-[8px]">
        <Switch checked={orphans} onCheckedChange={setOrphans} />
        Also remove containers for services no longer in the files
      </label>
      <label className="flex items-center gap-[8px]">
        <Switch
          checked={volumes}
          onCheckedChange={setVolumes}
          aria-describedby="down-volumes-warning"
        />
        Delete the project’s volumes
      </label>
      {volumes && (
        <div
          id="down-volumes-warning"
          className="flex flex-col gap-[6px] rounded-[8px] border border-(--danger)/30 bg-(--danger)/5 px-[10px] py-[8px]"
        >
          <p>
            <strong>
              Databases and other data in them are deleted for good.
            </strong>{" "}
            Type <code>{project}</code> to confirm.
          </p>
          <Input
            aria-label="Project name"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            autoComplete="off"
            spellCheck={false}
          />
        </div>
      )}
    </Confirm>
  );
}

export function ScaleDialog({
  service,
  current,
  busy,
  error,
  onConfirm,
  onClose,
}: {
  service: string;
  current: number;
  busy: boolean;
  error: string | null;
  onConfirm: (replicas: number) => void;
  onClose: () => void;
}) {
  const [value, setValue] = useState(String(current || 1));
  const replicas = Number(value);
  const valid = Number.isInteger(replicas) && replicas >= 0 && replicas <= 20;
  return (
    <Confirm
      title={`Scale ${service}`}
      action={valid ? `Run ${replicas}` : "Scale"}
      destructive={false}
      busy={busy}
      error={error ?? (valid ? null : "Choose 0 to 20 containers.")}
      onConfirm={() => valid && onConfirm(replicas)}
      onClose={onClose}
    >
      <p>
        How many <strong>{service}</strong> containers should run? It runs{" "}
        {current} now. Services that publish a fixed host port can only run one.
      </p>
      <Input
        type="number"
        min={0}
        max={20}
        aria-label="Containers"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        className="w-[96px]"
      />
    </Confirm>
  );
}
