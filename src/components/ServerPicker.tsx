import { ChevronsUpDown, Plus, Server as ServerIcon } from "lucide-react";
import { useMemo, useState } from "react";
import { displayName, type Server } from "../types";
import { Button } from "./controls";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "./ui/command";

function endpoint(server: Server) {
  const host = server.sshHost.includes(":")
    ? `[${server.sshHost}]`
    : server.sshHost;
  return `${server.sshUser}@${host}${server.sshPort === 22 ? "" : `:${server.sshPort}`}`;
}

export function ServerPicker({
  servers,
  selected,
  loaded,
  blocked,
  onSelect,
  onAdd,
}: {
  servers: Server[];
  selected: string;
  loaded: boolean;
  blocked: boolean;
  onSelect: (id: string) => void;
  onAdd: () => void;
}) {
  const [open, setOpen] = useState(false);
  const current = servers.find((server) => server.id === selected);
  const sorted = useMemo(
    () =>
      [...servers].sort(
        (a, b) =>
          displayName(a).localeCompare(displayName(b), undefined, {
            numeric: true,
            sensitivity: "base",
          }) ||
          endpoint(a).localeCompare(endpoint(b)) ||
          a.id.localeCompare(b.id),
      ),
    [servers],
  );
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          className="server-picker"
          aria-label="Choose server"
          disabled={!loaded}
        >
          <span className="server-avatar">
            <ServerIcon size={20} strokeWidth={1.5} />
          </span>
          <span className="server-picker-label">
            <strong>
              {current ? displayName(current) : "Choose a server"}
            </strong>
            <small>{current ? endpoint(current) : "Your SSH workspace"}</small>
          </span>
          <ChevronsUpDown size={12} aria-hidden="true" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        side="bottom"
        sideOffset={4}
        collisionPadding={12}
        className="server-picker-menu"
        aria-label="Server picker"
      >
        <Command
          defaultValue={selected}
          label="Search servers"
          filter={(_value, search, keywords) => {
            const text = (keywords ?? []).join(" ").toLocaleLowerCase();
            return search
              .trim()
              .toLocaleLowerCase()
              .split(/\s+/)
              .every((word) => text.includes(word))
              ? 1
              : 0;
          }}
        >
          <CommandInput
            placeholder="Find a server…"
            aria-label="Search servers"
          />
          <CommandList label="Saved servers">
            <CommandEmpty>
              {servers.length
                ? "No matching servers."
                : "No saved servers yet."}
            </CommandEmpty>
            <CommandGroup>
              {sorted.map((server) => (
                <CommandItem
                  key={server.id}
                  value={server.id}
                  keywords={[
                    displayName(server),
                    server.sshHost,
                    server.sshUser,
                    String(server.sshPort),
                    endpoint(server),
                  ]}
                  data-checked={server.id === selected}
                  className="server-picker-option"
                  onSelect={() => {
                    onSelect(server.id);
                    setOpen(false);
                  }}
                >
                  <ServerIcon
                    className="server-picker-option-icon"
                    aria-hidden="true"
                  />
                  <span className="server-picker-option-label">
                    <span className="server-picker-option-name">
                      {displayName(server)}
                    </span>
                    <span className="picker-host">{endpoint(server)}</span>
                    {server.id === selected && (
                      <span className="sr-only">Current server</span>
                    )}
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
        <div className="server-picker-footer">
          <Button
            variant="ghost"
            disabled={blocked}
            onClick={() => {
              setOpen(false);
              onAdd();
            }}
          >
            <Plus size={14} aria-hidden="true" /> Add server…
          </Button>
          <span>
            {servers.length} {servers.length === 1 ? "server" : "servers"}
          </span>
        </div>
      </PopoverContent>
    </Popover>
  );
}
