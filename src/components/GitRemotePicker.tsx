import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown } from "lucide-react";
import { useCurrentServerScope } from "../query/keys";
import { gitQueries } from "../query/git";
import { gitProjectsFor } from "../git/registry";
import { gitErrorMessage } from "../git/errors";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { Button } from "./controls";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover";
import { Command, CommandInput, CommandList, CommandItem } from "./ui/command";
import { GitLoadMore } from "./GitLoadMore";

export function GitRemotePicker({
  repoId,
  value,
  onChange,
  disabled = false,
}: {
  repoId: string;
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          role="combobox"
          aria-label="Remote"
          aria-expanded={open}
          disabled={disabled}
          className="w-full justify-between"
        >
          <span className="truncate">{value || "Choose remote"}</span>
          <ChevronDown size={14} />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-[320px] p-0">
        {open && (
          <RemoteNames
            repoId={repoId}
            onSelect={(name) => {
              onChange(name);
              setOpen(false);
            }}
          />
        )}
      </PopoverContent>
    </Popover>
  );
}
function RemoteNames({
  repoId,
  onSelect,
}: {
  repoId: string;
  onSelect: (name: string) => void;
}) {
  const scope = useCurrentServerScope();
  const [input, setInput] = useState("");
  const [filter, setFilter] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setFilter(input), 200);
    return () => clearTimeout(timer);
  }, [input]);
  const params = { repoId, filter, pageSize: 20 };
  const query = useQuery({
    ...gitQueries.remoteNames(scope, params),
    refetchOnMount: false,
  });
  const waiting = input !== filter;
  const page = !query.isError ? query.data : undefined;
  const pages = useGitPageLoader({
    queryKey: gitQueries.remoteNames(scope, params).queryKey,
    page: page ?? null,
    enabled: !waiting && !query.isFetching && !query.isError,
    prefetch: true,
    entryKey: (entry: { name: string }) => entry.name,
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .remoteNames({ ...params, cursor }),
  });
  return (
    <Command shouldFilter={false}>
      <CommandInput
        aria-label="Search remotes"
        placeholder="Search remotes…"
        value={input}
        onValueChange={setInput}
      />
      <CommandList
        aria-label="Remotes"
        className="max-h-[260px] overflow-y-auto"
      >
        {(waiting || query.isFetching) && (
          <p role="status" className="p-3 text-xs text-muted-foreground">
            Loading remotes…
          </p>
        )}
        {!waiting && !query.isFetching && query.error && (
          <div role="alert" className="p-3 text-xs">
            {gitErrorMessage(query.error)}
            <Button onClick={() => void query.refetch()}>Retry remotes</Button>
          </div>
        )}
        {!waiting && !query.isFetching && page && (
          <>
            {page.entries.map((entry) => (
              <CommandItem
                key={entry.name}
                value={entry.name}
                onSelect={() => onSelect(entry.name)}
              >
                {entry.name}
              </CommandItem>
            ))}
            {!page.entries.length && (
              <p className="p-3 text-xs text-muted-foreground">
                {filter ? "No matching remotes." : "No remotes configured."}
              </p>
            )}
            <GitLoadMore
              cursor={page.nextCursor}
              loading={pages.loading}
              error={pages.error}
              onLoad={() => void pages.load()}
              label="Load more remotes"
              endLabel="All remotes loaded"
            />
          </>
        )}
      </CommandList>
    </Command>
  );
}
