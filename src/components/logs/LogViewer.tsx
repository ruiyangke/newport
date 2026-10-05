import { Fragment, useLayoutEffect, useRef, useState } from "react";
import { ArrowDown } from "lucide-react";
import { cn } from "../../lib/utils";
import { SearchField } from "../SearchField";
import { Skeleton } from "../ui/skeleton";
import { Switch } from "../ui/switch";
import { ToggleGroup, ToggleGroupItem } from "../ui/toggle-group";

/** One line as the viewer shows it, whatever log it came from. */
export interface LogLine {
  at: number | null;
  tone: "danger" | "warning" | "muted";
  /** Who wrote it: a unit's `name[pid]`, a project's service. */
  source?: string | null;
  message: string;
}
export type LogLevel = "all" | "warning" | "err";

const time = (at: number) =>
  new Date(at).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
const day = (at: number) =>
  new Date(at).toLocaleDateString([], {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
// Placeholder widths, so the skeleton reads as log lines rather than bars.
const SKELETON = [72, 54, 88, 40, 66, 80, 48, 92, 60, 36, 74, 58];

function Line({
  line,
  fresh,
  timestamps,
}: {
  line: LogLine;
  fresh: boolean;
  timestamps: boolean;
}) {
  const { tone } = line;
  return (
    <li
      className={cn(
        "grid gap-x-[12px] border-l-2 py-[1px] pl-[8px]",
        timestamps
          ? "grid-cols-[auto_minmax(0,1fr)]"
          : "grid-cols-[minmax(0,1fr)]",
        tone === "danger"
          ? "border-(--danger) bg-(--danger)/6"
          : tone === "warning"
            ? "border-(--warning) bg-(--warning)/6"
            : "border-transparent",
        fresh &&
          "animate-in duration-300 fade-in slide-in-from-bottom-1 motion-reduce:animate-none",
      )}
    >
      {!timestamps ? null : line.at === null ? (
        <span className="text-muted-foreground">—</span>
      ) : (
        <time
          dateTime={new Date(line.at).toISOString()}
          title={new Date(line.at).toLocaleString()}
          className="text-muted-foreground tabular-nums"
        >
          {time(line.at)}
        </time>
      )}
      <span className="wrap-anywhere whitespace-pre-wrap">
        {line.source && (
          <span className="text-muted-foreground">{line.source}: </span>
        )}
        <span
          className={cn(
            tone === "danger" && "text-(--danger)",
            tone === "warning" && "text-(--warning)",
          )}
        >
          {line.message}
        </span>
      </span>
    </li>
  );
}

/** Live, level and search, for whichever log sits below. */
export function LogToolbar({
  live,
  onLive,
  level,
  onLevel,
  find,
  onFind,
}: {
  live: boolean;
  onLive: (live: boolean) => void;
  level: LogLevel;
  onLevel: (level: LogLevel) => void;
  find: string;
  onFind: (find: string) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-[8px]">
      <label className="flex items-center gap-[6px] text-[12px] text-muted-foreground">
        <Switch
          aria-label="Follow new lines"
          checked={live}
          onCheckedChange={onLive}
          className="scale-90"
        />
        Live
      </label>
      <ToggleGroup
        type="single"
        variant="outline"
        spacing={0}
        size="sm"
        aria-label="Log level"
        value={level}
        onValueChange={(value) => value && onLevel(value as LogLevel)}
      >
        <ToggleGroupItem value="all" className="px-[9px] text-[12px]">
          All
        </ToggleGroupItem>
        <ToggleGroupItem value="warning" className="px-[9px] text-[12px]">
          Warnings
        </ToggleGroupItem>
        <ToggleGroupItem value="err" className="px-[9px] text-[12px]">
          Errors
        </ToggleGroupItem>
      </ToggleGroup>
      <SearchField
        className="w-[170px]"
        aria-label="Search logs"
        placeholder="Search these lines"
        value={find}
        onChange={(e) => onFind(e.target.value)}
      />
    </div>
  );
}

/**
 * A log filling the space it is given. It follows the newest line while the
 * reader is at the bottom; scrolled up, new lines wait behind a "new lines"
 * button instead of pulling the view away. Lines already there when a log
 * first arrives appear at once; only later ones fade in.
 */
export function LogViewer({
  lines,
  filter,
  dataset,
  pending,
  placeholder = false,
  error,
  live,
  find,
  label,
  reading = "Reading logs",
  empty,
  filtered,
  timestamps = true,
  idle = "paused",
}: {
  /** Every line that has arrived, oldest first. */
  lines: LogLine[];
  /** A level filter applied here rather than by the server. */
  filter?: (line: LogLine) => boolean;
  /** Identity of the lines: a change means a new log, not more of it. */
  dataset: string;
  pending: boolean;
  /** Showing the previous filter's lines while the new ones load. */
  placeholder?: boolean;
  error: unknown;
  live: boolean;
  find: string;
  label: string;
  reading?: string;
  empty: string;
  /** Shown when lines exist but the level hides them all. */
  filtered?: string;
  /** Off for command output, whose lines carry no time. */
  timestamps?: boolean;
  /** The footer's word when not following: "paused", or "finished". */
  idle?: string;
}) {
  const needle = find.trim().toLowerCase();
  const visible = filter
    ? lines
        .map((line, index) => ({ line, index }))
        .filter((x) => filter(x.line))
    : lines.map((line, index) => ({ line, index }));
  const shown = needle
    ? visible.filter(({ line }) =>
        `${line.source ?? ""} ${line.message}`.toLowerCase().includes(needle),
      )
    : visible;
  const stamped = lines.filter((l) => l.at !== null);
  const spansDays =
    stamped.length > 1 &&
    day(stamped[0].at!) !== day(stamped[stamped.length - 1].at!);

  const ready = !pending && !placeholder;
  const [settled, setSettled] = useState<{
    dataset: string;
    count: number;
  } | null>(null);
  if (ready && settled?.dataset !== dataset)
    setSettled({ dataset, count: lines.length });

  const box = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(true);
  const [unseen, setUnseen] = useState(0);
  // Only lines that arrive count as new: not a looser search, and not the
  // first page of another log.
  const arrived = useRef({ dataset, count: 0 });
  useLayoutEffect(() => {
    const el = box.current;
    if (el && pinned) el.scrollTop = el.scrollHeight;
    if (!ready) return;
    const before = arrived.current;
    arrived.current = { dataset, count: lines.length };
    const added = lines.length - before.count;
    if (!pinned && before.dataset === dataset && added > 0)
      setUnseen((n) => n + added);
  }, [ready, dataset, lines.length, shown.length, pinned]);
  const toLatest = () => {
    const el = box.current;
    if (!el) return;
    setUnseen(0);
    setPinned(true);
    el.scrollTo({
      top: el.scrollHeight,
      behavior: matchMedia("(prefers-reduced-motion: reduce)").matches
        ? "auto"
        : "smooth",
    });
  };

  let previousDay: string | null = null;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {!!error && (
        <p
          role="alert"
          className="mb-[8px] text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
        >
          {String(error)}
        </p>
      )}
      {/* Out of flow, so a long log never sizes the pane: the lines get
          whatever height is left and scroll inside it. */}
      <div className="relative min-h-0 flex-1">
        <div
          ref={box}
          onScroll={(e) => {
            const el = e.currentTarget;
            const atBottom =
              el.scrollHeight - el.scrollTop - el.clientHeight < 24;
            if (atBottom !== pinned) setPinned(atBottom);
            if (atBottom && unseen) setUnseen(0);
          }}
          tabIndex={0}
          role="log"
          aria-label={`${label} output`}
          aria-live={live ? "polite" : "off"}
          aria-busy={pending || placeholder}
          className={cn(
            "absolute inset-0 overflow-auto rounded-[8px] border border-border bg-(--native-toolbar)/60 px-[10px] py-[8px] font-mono text-[11.5px] leading-[1.6] transition-opacity focus-visible:outline-2 focus-visible:outline-ring motion-reduce:transition-none",
            placeholder && "opacity-55",
          )}
        >
          {pending ? (
            <ul
              role="list"
              aria-label={reading}
              className="m-0 flex list-none flex-col gap-[7px] p-0 pt-[3px]"
            >
              {SKELETON.map((width, i) => (
                <li key={i} className="flex gap-[12px]">
                  <Skeleton className="h-[11px] w-[62px] shrink-0 rounded-[3px]" />
                  <Skeleton
                    className="h-[11px] rounded-[3px]"
                    style={{ width: `${width}%` }}
                  />
                </li>
              ))}
            </ul>
          ) : shown.length ? (
            <ol role="list" className="m-0 list-none p-0">
              {shown.map(({ line, index }) => {
                const date =
                  spansDays && line.at !== null ? day(line.at) : null;
                const divider = date !== null && date !== previousDay;
                if (date !== null) previousDay = date;
                return (
                  <Fragment key={index}>
                    {divider && (
                      <li
                        role="separator"
                        aria-label={date}
                        className="sticky -top-[8px] z-[1] -mx-[10px] my-[4px] flex items-center gap-[8px] bg-(--native-toolbar) px-[10px] py-[2px] font-sans text-[10.5px] font-semibold text-muted-foreground uppercase first:mt-0"
                      >
                        {date}
                        <span className="h-px flex-1 bg-border" />
                      </li>
                    )}
                    <Line
                      line={line}
                      fresh={!!settled && index >= settled.count}
                      timestamps={timestamps}
                    />
                  </Fragment>
                );
              })}
            </ol>
          ) : (
            <p className="text-muted-foreground">
              {needle
                ? "No lines match."
                : lines.length && filtered
                  ? filtered
                  : empty}
            </p>
          )}
        </div>
        {unseen > 0 && !pinned && (
          <button
            type="button"
            onClick={toLatest}
            className="absolute bottom-[12px] left-1/2 inline-flex -translate-x-1/2 animate-in items-center gap-[5px] rounded-full border border-border bg-background px-[11px] py-[4px] text-[11.5px] font-medium shadow-md duration-200 fade-in slide-in-from-bottom-2 hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring motion-reduce:animate-none"
          >
            <ArrowDown size={12} aria-hidden="true" />
            {unseen} new {unseen === 1 ? "line" : "lines"}
          </button>
        )}
      </div>
      <p
        role="status"
        className="mt-[6px] text-[11px] text-muted-foreground tabular-nums"
      >
        {pending
          ? `${reading}…`
          : placeholder
            ? "Filtering…"
            : needle || shown.length !== lines.length
              ? `${shown.length} of ${lines.length} lines`
              : `${lines.length} lines`}
        {!pending &&
          (live
            ? pinned
              ? " · following new lines"
              : " · scrolled up; new lines wait below"
            : ` · ${idle}`)}
      </p>
    </div>
  );
}
