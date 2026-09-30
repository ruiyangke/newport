import { GitLogTerminal, type GitLogTerminalHandle } from "./GitLogTerminal";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { Copy, ScrollText, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "./ui/button";

type Entry = {
  command: string;
  durationMs: number;
  exitCode: number | null;
  output: string;
  interrupted: boolean;
};
type LogEvent = { serverId: string; repoId: string | null; entry: Entry };
const LIMIT = 100;
const result = (entry: Entry) =>
  entry.interrupted
    ? "Interrupted"
    : entry.exitCode === 0
      ? "Done"
      : `Exit ${entry.exitCode ?? "unknown"}`;

/** Session-only, bounded command output; argument values are omitted. */
export function GitCommandLog({
  serverId,
  repoId,
  children,
}: {
  serverId: string;
  repoId: string;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [follow, setFollow] = useState(true);
  const [listenerError, setListenerError] = useState(false);
  const panelId = useId();
  const terminal = useRef<GitLogTerminalHandle>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen<LogEvent>("git-command-log", ({ payload }) => {
      if (
        !disposed &&
        payload.serverId === serverId &&
        payload.repoId === repoId
      ) {
        setEntries((current) => [...current, payload.entry].slice(-LIMIT));
      }
    })
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch(() => {
        if (!disposed) setListenerError(true);
      });
    return () => {
      disposed = true;
      stop?.();
    };
  }, [serverId, repoId]);
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  const text = entries
    .map(
      (entry) =>
        `\x1b[0m$ ${entry.command}\n${result(entry)} · ${entry.durationMs} ms${entry.output ? `\n${entry.output}` : ""}\x1b[0m`,
    )
    .join("\n\n");
  return (
    <>
      {open && (
        <section
          id={panelId}
          aria-label="Git command log"
          className="flex h-[220px] max-h-[40vh] min-h-0 flex-none flex-col border-t border-border bg-background"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation();
              close();
            }
          }}
        >
          <header className="flex h-9 flex-none items-center gap-2 border-b border-border px-3">
            <h3 className="text-xs font-medium">Git output</h3>
            <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
              Latest {LIMIT} commands · this session
            </span>
            <Button
              size="xs"
              variant="ghost"
              aria-pressed={follow}
              onClick={() => setFollow(!follow)}
            >
              Auto-scroll {follow ? "on" : "off"}
            </Button>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="Copy Git log"
              title="Copy log"
              disabled={!entries.length}
              onClick={() => {
                void navigator.clipboard
                  .writeText(terminal.current?.text() ?? "")
                  .then(() => toast.success("Git log copied"))
                  .catch(() => toast.error("Could not copy the Git log"));
              }}
            >
              <Copy />
            </Button>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="Clear Git log"
              title="Clear log"
              disabled={!entries.length}
              onClick={() => setEntries([])}
            >
              <Trash2 />
            </Button>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="Close Git log"
              title="Close log"
              onClick={close}
            >
              <X />
            </Button>
          </header>
          <div className="flex min-h-0 flex-1 p-3">
            {entries.length ? (
              <GitLogTerminal ref={terminal} text={text} follow={follow} />
            ) : (
              <p className="font-sans text-xs text-muted-foreground">
                {listenerError
                  ? "Could not subscribe to Git output. Reopen Projects to retry."
                  : "Run a Git action or refresh this repository to see command output. Requires the CLI backend."}
              </p>
            )}
          </div>
          <p className="flex-none px-3 pb-2 text-[10px] text-muted-foreground">
            Output appears when a request finishes. Up to 4 KiB per stream;
            terminal formatting is applied and sensitive values filtered.
          </p>
        </section>
      )}
      <footer className="git-projects-footer flex h-[28px] flex-none items-center justify-between gap-[12px] border-t border-border px-[12px] text-[11px] text-muted-foreground">
        {children}
        <Button
          ref={trigger}
          size="xs"
          variant="ghost"
          className="h-6 flex-none px-1.5 text-[11px]"
          aria-expanded={open}
          aria-controls={panelId}
          onClick={() => setOpen(!open)}
        >
          <ScrollText className="size-3" />
          Log
        </Button>
      </footer>
    </>
  );
}
