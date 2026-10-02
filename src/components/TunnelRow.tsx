import {
  ArrowRight,
  ExternalLink,
  MoreHorizontal,
  Pencil,
  Play,
  Square,
  Trash2,
} from "lucide-react";
import { desktop } from "../api/desktop";
import {
  active,
  range,
  type Tunnel,
  type DestinationHealth,
  type ConnectionState,
} from "../types";
import { Button } from "./controls";
import { DestinationStatus } from "./DestinationStatus";
import { StatusLabel } from "./StatusLabel";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "./ui/dropdown-menu";

type Props = {
  tunnel: Tunnel;
  state: ConnectionState;
  health?: DestinationHealth[];
  pending: Set<string>;
  act: (
    key: string,
    task: () => Promise<unknown>,
    message?: string,
  ) => Promise<void>;
  setEditor: (editor: {
    kind: "tunnel";
    value: Tunnel;
    existing: boolean;
  }) => void;
  setDeleting: (target: { kind: "tunnel"; id: string; name: string }) => void;
};

export function TunnelRow({
  tunnel: t,
  state,
  health,
  pending,
  act,
  setEditor,
  setDeleting,
}: Props) {
  return (
    <li
      data-slot="tunnel-row"
      className="flex items-center gap-[16px] border-b border-border px-[16px] py-[14px] last:border-b-0 focus-within:bg-accent max-[850px]:flex-wrap max-[850px]:items-start"
    >
      <div className="min-w-0 flex-1 max-[850px]:basis-full">
        <div className="flex flex-wrap items-center gap-x-[12px] gap-y-[8px]">
          <h3 className="font-semibold" id={`tunnel-${t.id}`}>
            {t.name || `Port ${t.localPort}`}
          </h3>
          <StatusLabel state={state} />
        </div>
        <div className="mt-[8px] flex flex-wrap items-center gap-[12px] text-muted-foreground">
          <div className="flex min-w-0 flex-col gap-[2px]">
            <small className="text-[10px]">On this computer</small>
            <code className="text-[12px] wrap-anywhere">
              127.0.0.1:
              <b className="font-medium text-foreground">
                {range(t.localPort, t.localPortEnd)}
              </b>
            </code>
          </div>
          <ArrowRight size={14} />
          <div className="flex min-w-0 flex-col gap-[2px]">
            <small className="text-[10px]">Remote destination</small>
            <code className="text-[12px] wrap-anywhere">
              {t.remoteHost}:
              <b className="font-medium text-foreground">
                {range(t.remotePort, t.remotePortEnd)}
              </b>
            </code>
          </div>
        </div>
        {(t.autoConnect || t.autoReconnect) && (
          <small className="mt-[7px] block text-[11px]">
            {[
              t.autoConnect && "Connect on launch",
              t.autoReconnect && "Auto reconnect",
            ]
              .filter(Boolean)
              .join(" · ")}
          </small>
        )}
        {state.status === "connected" && <DestinationStatus health={health} />}
        {state.errorMessage && (
          <p className="mt-[8px] text-[11px] leading-[1.6] whitespace-pre-wrap text-(--danger) wrap-anywhere">
            {state.errorMessage}
          </p>
        )}
      </div>
      <div className="flex shrink-0 items-center max-[850px]:ml-auto">
        <Button
          variant="ghost"
          size="icon"
          aria-label={`Open ${t.name || t.localPort} in browser`}
          title="Open in browser (HTTP)"
          disabled={state.status !== "connected"}
          onClick={() =>
            void act(`open-${t.id}`, () => desktop("open_tunnel", { id: t.id }))
          }
        >
          <ExternalLink size={15} />
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Actions for tunnel ${t.name || t.localPort}`}
            >
              <MoreHorizontal size={16} />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              aria-label={`Edit ${t.name || t.localPort}`}
              onSelect={() =>
                setEditor({
                  kind: "tunnel",
                  value: t,
                  existing: true,
                })
              }
            >
              <Pencil size={15} /> Edit tunnel
            </DropdownMenuItem>
            <DropdownMenuItem
              variant="destructive"
              aria-label={`Delete ${t.name || t.localPort}`}
              onSelect={() =>
                setDeleting({
                  kind: "tunnel",
                  id: t.id,
                  name: t.name || `Port ${t.localPort}`,
                })
              }
            >
              <Trash2 size={15} /> Delete tunnel
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          className="ml-[8px] min-w-[78px]"
          aria-describedby={`tunnel-${t.id}`}
          loading={pending.has(t.id)}
          disabled={pending.has("all")}
          onClick={() =>
            void act(t.id, () =>
              desktop("set_tunnel_connected", {
                id: t.id,
                connected: !active(state.status),
              }),
            )
          }
        >
          {active(state.status) ? <Square size={13} /> : <Play size={13} />}
          {active(state.status) ? "Disconnect" : "Connect"}
        </Button>
      </div>
    </li>
  );
}
