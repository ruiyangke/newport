import { useState, type ReactNode } from "react";
import { toast } from "sonner";
import { ChevronDown } from "lucide-react";
import { cn } from "../../lib/utils";
import { Button } from "../controls";
import { Skeleton } from "../ui/skeleton";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { enter } from "../master/Detail";
import { Confirm } from "./parts";
import { bytes } from "../../overviewHealth";
import {
  useContainerActions,
  useDockerDisk,
  usePendingActions,
} from "../../hooks/useContainers";
import {
  duration,
  type DiskTotals,
  type PruneTarget,
} from "../../domain/containers";

const PRUNE: Record<
  PruneTarget,
  { title: string; action: string; body: ReactNode; destructive: boolean }
> = {
  containers: {
    title: "Remove all stopped containers?",
    action: "Remove stopped",
    body: "Every container that isn’t running is deleted, with its logs. Images and volumes stay. Compose recreates its services on the next “up”.",
    destructive: true,
  },
  "dangling-images": {
    title: "Remove dangling images?",
    action: "Remove dangling",
    body: "Untagged image layers left behind by rebuilds and pulls are deleted. Nothing that runs or is tagged is touched.",
    destructive: false,
  },
  "unused-images": {
    title: "Remove every unused image?",
    action: "Remove unused",
    body: "Every image no container uses is deleted, tagged ones included. Starting something from them later means pulling or building again.",
    destructive: true,
  },
  "build-cache": {
    title: "Clear the build cache?",
    action: "Clear cache",
    body: "Cached build layers are deleted. The next builds take longer while the cache fills again.",
    destructive: false,
  },
  volumes: {
    title: "Remove unused anonymous volumes?",
    action: "Remove volumes",
    body: (
      <>
        <strong>The data in them is deleted for good.</strong> Only anonymous
        volumes no container uses are removed; named volumes stay.
      </>
    ),
    destructive: true,
  },
};

function Tile({
  label,
  totals,
  unit,
  children,
}: {
  label: string;
  totals: DiskTotals;
  unit: [string, string];
  children?: ReactNode;
}) {
  return (
    <section
      aria-label={label}
      className="flex min-w-0 flex-col gap-[6px] rounded-[10px] border border-border px-[14px] py-[12px]"
    >
      <h3 className="text-[11.5px] font-medium text-muted-foreground">
        {label}
      </h3>
      <p className="text-[20px] font-semibold tracking-[-0.01em]">
        {bytes(totals.size)}
      </p>
      <p className="text-[11.5px] text-muted-foreground">
        {totals.count} {totals.count === 1 ? unit[0] : unit[1]}
        {totals.reclaimable > 0 && (
          <>
            {" · "}
            <span className="text-foreground">
              {bytes(totals.reclaimable)}
            </span>{" "}
            can be freed
          </>
        )}
      </p>
      <div className="mt-auto flex flex-wrap gap-[6px] pt-[4px]">
        {children}
      </div>
    </section>
  );
}

const th = "pb-[4px] text-left text-[11px] font-normal text-muted-foreground";
const td = "border-t border-border py-[5px] pr-[12px] align-top";

/**
 * What Docker keeps on the server's disk, and what can go: stopped
 * containers, unused images, anonymous volumes and the build cache.
 */
export function DiskUsage({ id, now }: { id: string; now: number }) {
  const disk = useDockerDisk(id);
  const action = useContainerActions(id);
  const pending = usePendingActions(id);
  const [confirming, setConfirming] = useState<PruneTarget | null>(null);
  const running = (what: PruneTarget) =>
    pending.some((a) => a.kind === "prune" && a.what === what);
  const error =
    action.variables?.kind === "prune" && action.error
      ? String(action.error)
      : null;
  const prune = (what: PruneTarget) =>
    action.mutate(
      { kind: "prune", what },
      {
        onSuccess: (result) => {
          setConfirming(null);
          const r = result as { removed: number; reclaimed: number } | null;
          toast(
            r && r.reclaimed > 0
              ? `Freed ${bytes(r.reclaimed)}`
              : "Nothing to remove",
          );
        },
      },
    );
  const button = (what: PruneTarget, label: string) => (
    <Button
      variant="outline"
      className="h-[26px] px-[8px] text-[12px]"
      loading={running(what)}
      disabled={!!pending.length}
      onClick={() => setConfirming(what)}
    >
      {label}
    </Button>
  );
  const data = disk.data;
  const total = data
    ? Object.values(data.totals).reduce((sum, t) => sum + t.size, 0)
    : 0;
  const reclaimable = data
    ? Object.values(data.totals).reduce((sum, t) => sum + t.reclaimable, 0)
    : 0;
  const images = [...(data?.images ?? [])].sort((a, b) => b.size - a.size);
  const volumes = [...(data?.volumes ?? [])].sort(
    (a, b) => (b.size ?? -1) - (a.size ?? -1),
  );
  const dangling = images.filter((i) => i.dangling).length;
  return (
    <article
      aria-label="Disk usage"
      className={cn("flex min-w-0 flex-col gap-[16px]", enter)}
    >
      <header>
        <h2 className="text-[17px]">Disk usage</h2>
        <p role="status" className="mt-[2px] text-[12px] text-muted-foreground">
          {data
            ? `Docker uses ${bytes(total)} on this server${reclaimable ? ` · ${bytes(reclaimable)} can be freed` : ""}`
            : disk.error
              ? "Disk usage unavailable"
              : "Reading disk usage…"}
        </p>
      </header>
      {disk.error && (
        <p role="alert" className="text-[12px] text-(--danger) wrap-anywhere">
          {String(disk.error)}
        </p>
      )}
      {!data && !disk.error && (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-[10px]">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-[112px] rounded-[10px]" />
          ))}
        </div>
      )}
      {data && (
        <>
          <div className="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-[10px]">
            <Tile
              label="Images"
              totals={data.totals.images}
              unit={["image", "images"]}
            >
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="outline"
                    className="h-[26px] px-[8px] text-[12px]"
                    disabled={!!pending.length}
                    loading={
                      running("dangling-images") || running("unused-images")
                    }
                  >
                    Clean up <ChevronDown size={12} />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start">
                  <DropdownMenuItem
                    onSelect={() => setConfirming("dangling-images")}
                  >
                    Remove dangling images
                    <span className="ml-auto pl-[12px] text-muted-foreground tabular-nums">
                      {dangling}
                    </span>
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onSelect={() => setConfirming("unused-images")}
                  >
                    Remove all unused images
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </Tile>
            <Tile
              label="Containers"
              totals={data.totals.containers}
              unit={["container", "containers"]}
            >
              {button("containers", "Remove stopped")}
            </Tile>
            <Tile
              label="Volumes"
              totals={data.totals.volumes}
              unit={["volume", "volumes"]}
            >
              {button("volumes", "Remove unused")}
            </Tile>
            <Tile
              label="Build cache"
              totals={data.totals.buildCache}
              unit={["entry", "entries"]}
            >
              {button("build-cache", "Clear")}
            </Tile>
          </div>
          {error && !confirming && (
            <p
              role="alert"
              className="text-[12px] text-(--danger) wrap-anywhere"
            >
              {error}
            </p>
          )}
          <section aria-label="Images">
            <h3 className="mb-[6px] text-[12.5px] font-semibold">
              Images
              <span className="ml-[6px] font-normal text-muted-foreground tabular-nums">
                {images.length}
              </span>
            </h3>
            <table className="w-full border-collapse text-[12px]">
              <thead>
                <tr>
                  <th className={th}>Image</th>
                  <th className={cn(th, "text-right")}>Size</th>
                  <th className={cn(th, "pl-[14px]")}>Created</th>
                  <th className={th}>Used by</th>
                </tr>
              </thead>
              <tbody>
                {images.map((image) => (
                  <tr key={image.id}>
                    <td
                      className={cn(
                        td,
                        "font-mono text-[11.5px] wrap-anywhere",
                      )}
                    >
                      {image.tags.length ? (
                        image.tags.join(", ")
                      ) : (
                        <span className="text-muted-foreground">
                          untagged{" "}
                          {image.id.replace(/^sha256:/, "").slice(0, 12)}
                        </span>
                      )}
                    </td>
                    <td className={cn(td, "text-right tabular-nums")}>
                      {bytes(image.size)}
                    </td>
                    <td
                      className={cn(
                        td,
                        "pl-[14px] whitespace-nowrap text-muted-foreground",
                      )}
                    >
                      {duration(now - image.created)} ago
                    </td>
                    <td className={cn(td, "whitespace-nowrap")}>
                      {image.containers > 0 ? (
                        `${image.containers} ${image.containers === 1 ? "container" : "containers"}`
                      ) : (
                        <span className="text-muted-foreground">
                          {image.dangling ? "dangling" : "unused"}
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
          {volumes.length > 0 && (
            <section aria-label="Volumes">
              <h3 className="mb-[6px] text-[12.5px] font-semibold">
                Volumes
                <span className="ml-[6px] font-normal text-muted-foreground tabular-nums">
                  {volumes.length}
                </span>
              </h3>
              <table className="w-full border-collapse text-[12px]">
                <thead>
                  <tr>
                    <th className={th}>Volume</th>
                    <th className={cn(th, "text-right")}>Size</th>
                    <th className={cn(th, "pl-[14px]")}>Used by</th>
                  </tr>
                </thead>
                <tbody>
                  {volumes.map((v) => (
                    <tr key={v.name}>
                      <td className={cn(td, "wrap-anywhere")}>
                        <span className="font-mono text-[11.5px]">
                          {v.anonymous ? `${v.name.slice(0, 12)}…` : v.name}
                        </span>
                        {v.compose && (
                          <span className="ml-[6px] rounded-[4px] bg-muted px-[4px] text-[10.5px] text-muted-foreground">
                            {v.compose}
                          </span>
                        )}
                        {v.anonymous && (
                          <span className="ml-[6px] text-[11px] text-muted-foreground">
                            anonymous
                          </span>
                        )}
                      </td>
                      <td className={cn(td, "text-right tabular-nums")}>
                        {v.size !== null ? bytes(v.size) : "—"}
                      </td>
                      <td className={cn(td, "pl-[14px] whitespace-nowrap")}>
                        {v.refCount ? (
                          `${v.refCount} ${v.refCount === 1 ? "container" : "containers"}`
                        ) : (
                          <span className="text-muted-foreground">unused</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
          )}
        </>
      )}
      {confirming && (
        <Confirm
          title={PRUNE[confirming].title}
          action={PRUNE[confirming].action}
          destructive={PRUNE[confirming].destructive}
          busy={running(confirming)}
          error={error}
          onConfirm={() => prune(confirming)}
          onClose={() => setConfirming(null)}
        >
          <p>{PRUNE[confirming].body}</p>
        </Confirm>
      )}
    </article>
  );
}
