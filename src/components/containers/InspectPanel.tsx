import { useState, type ReactNode } from "react";
import { Copy, Eye, EyeOff } from "lucide-react";
import { toast } from "sonner";
import { Button } from "../controls";
import { Skeleton } from "../ui/skeleton";
import { Scroll } from "../master/Detail";
import { useContainerInspect } from "../../hooks/useContainers";
import { duration, type Container } from "../../domain/containers";

function Section({
  title,
  count,
  action,
  children,
}: {
  title: string;
  count?: number;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section aria-label={title} className="mb-[18px]">
      <header className="mb-[6px] flex items-center gap-[8px]">
        <h3 className="text-[12px] font-semibold">
          {title}
          {count !== undefined && (
            <span className="ml-[6px] font-normal text-muted-foreground tabular-nums">
              {count}
            </span>
          )}
        </h3>
        <span className="ml-auto">{action}</span>
      </header>
      {children}
    </section>
  );
}

const cell = "border-b border-border py-[5px] pr-[12px] align-top";
const mono = "font-mono text-[11.5px] wrap-anywhere";

// Names that usually carry secrets stay hidden even when values are shown.
const SECRET = /(pass|secret|token|key|credential|auth|private|dsn|cookie)/i;

/**
 * What the container was started with: command, environment, mounts,
 * networks, labels, and its health check's recent results. Environment
 * values stay hidden until asked for; secret-looking ones need their own
 * click.
 */
export function InspectPanel({
  id,
  container,
}: {
  id: string;
  container: Container;
}) {
  const inspect = useContainerInspect(id, container.id);
  const [reveal, setReveal] = useState(false);
  const [shown, setShown] = useState<ReadonlySet<string>>(new Set());
  const copy = (text: string, what: string) =>
    void navigator.clipboard
      .writeText(text)
      .then(() => toast(`Copied ${what}`))
      .catch(() => toast.error("Couldn't copy"));
  if (inspect.error)
    return (
      <p role="alert" className="text-[12px] text-(--danger) wrap-anywhere">
        {String(inspect.error)}
      </p>
    );
  const data = inspect.data;
  if (!data)
    return (
      <div
        aria-label="Reading container details"
        className="flex flex-col gap-[10px] pt-[4px]"
      >
        {[40, 70, 55, 85, 30, 60].map((w, i) => (
          <Skeleton
            key={i}
            className="h-[11px] rounded-[3px]"
            style={{ width: `${w}%` }}
          />
        ))}
      </div>
    );
  const command = [...(data.entrypoint ?? []), ...(data.cmd ?? [])];
  const labels = Object.entries(data.labels).sort(([a], [b]) =>
    a.localeCompare(b),
  );
  return (
    <Scroll>
      <div className="animate-in pr-[4px] duration-200 fade-in motion-reduce:animate-none">
        <Section
          title="Command"
          action={
            command.length > 0 && (
              <Button
                variant="ghost"
                className="h-[24px] px-[6px] text-[11.5px]"
                onClick={() => copy(command.join(" "), "command")}
              >
                <Copy size={12} /> Copy
              </Button>
            )
          }
        >
          <p className={`${mono} rounded-[6px] bg-muted px-[8px] py-[6px]`}>
            {command.length ? command.join(" ") : "—"}
          </p>
          <p className="mt-[6px] text-[11.5px] text-muted-foreground">
            {[
              data.workingDir && `in ${data.workingDir}`,
              `as ${data.user || "root"}`,
              data.hostname && `hostname ${data.hostname}`,
              data.restartPolicy && `restart: ${data.restartPolicy}`,
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </Section>
        <Section
          title="Environment"
          count={data.env.length}
          action={
            data.env.length > 0 && (
              <Button
                variant="ghost"
                className="h-[24px] px-[6px] text-[11.5px]"
                aria-pressed={reveal}
                onClick={() => {
                  setReveal(!reveal);
                  setShown(new Set());
                }}
              >
                {reveal ? <EyeOff size={12} /> : <Eye size={12} />}
                {reveal ? "Hide values" : "Show values"}
              </Button>
            )
          }
        >
          {data.env.length ? (
            <table className="w-full border-collapse text-[12px]">
              <tbody>
                {data.env.map(({ name, value }) => {
                  const secret = SECRET.test(name);
                  const visible = (reveal && !secret) || shown.has(name);
                  return (
                    <tr key={name}>
                      <th
                        scope="row"
                        className={`${cell} ${mono} w-[38%] text-left font-normal text-muted-foreground`}
                      >
                        {name}
                      </th>
                      <td className={`${cell} ${mono}`}>
                        {visible ? (
                          value || (
                            <span className="text-muted-foreground">
                              (empty)
                            </span>
                          )
                        ) : (
                          <button
                            type="button"
                            aria-label={`Show ${name}`}
                            onClick={() =>
                              setShown((prev) => new Set(prev).add(name))
                            }
                            className="rounded-[4px] text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                          >
                            ••••••{secret && reveal ? " (secret)" : ""}
                          </button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          ) : (
            <p className="text-[12px] text-muted-foreground">None set.</p>
          )}
        </Section>
        <Section title="Mounts" count={data.mounts.length}>
          {data.mounts.length ? (
            <ul role="list" className="m-0 list-none p-0 text-[12px]">
              {data.mounts.map((m) => (
                <li
                  key={m.destination}
                  className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-[12px] border-b border-border py-[5px]"
                >
                  <span className={mono}>
                    {m.destination}
                    <span className="text-muted-foreground">
                      {" ← "}
                      {m.type === "volume"
                        ? `volume ${m.name ?? m.source ?? ""}`
                        : (m.source ?? m.type)}
                    </span>
                  </span>
                  <span className="text-[11px] text-muted-foreground">
                    {m.rw ? "read-write" : "read-only"}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-[12px] text-muted-foreground">None.</p>
          )}
        </Section>
        <Section title="Networks" count={data.networks.length}>
          <ul role="list" className="m-0 list-none p-0 text-[12px]">
            {data.networks.map((n) => (
              <li
                key={n.name}
                className="flex flex-wrap gap-x-[10px] border-b border-border py-[5px]"
              >
                <span className="font-medium">{n.name}</span>
                {n.ipAddress && <span className={mono}>{n.ipAddress}</span>}
                {n.aliases.length > 0 && (
                  <span className="text-muted-foreground">
                    reachable as {n.aliases.join(", ")}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </Section>
        {data.healthcheck && (
          <Section title="Health check">
            <p className={`${mono} rounded-[6px] bg-muted px-[8px] py-[6px]`}>
              {data.healthcheck.test
                .filter((t) => t !== "CMD-SHELL" && t !== "CMD")
                .join(" ")}
            </p>
            {data.healthcheck.interval && (
              <p className="mt-[4px] text-[11.5px] text-muted-foreground">
                Every {duration(data.healthcheck.interval)}
              </p>
            )}
            {data.lastHealth.length > 0 && (
              <ol
                role="list"
                className="m-0 mt-[6px] list-none p-0 text-[12px]"
              >
                {data.lastHealth.map((h) => (
                  <li
                    key={h.at}
                    className="grid grid-cols-[auto_auto_minmax(0,1fr)] gap-x-[10px] border-b border-border py-[4px]"
                  >
                    <time className="text-muted-foreground tabular-nums">
                      {new Date(h.at).toLocaleTimeString()}
                    </time>
                    <span
                      className={
                        h.exitCode === 0
                          ? "text-(--success)"
                          : "text-(--danger)"
                      }
                    >
                      {h.exitCode === 0 ? "passed" : `failed (${h.exitCode})`}
                    </span>
                    <span className={`${mono} text-muted-foreground`}>
                      {h.output.trim() || "—"}
                    </span>
                  </li>
                ))}
              </ol>
            )}
          </Section>
        )}
        <Section title="Labels" count={labels.length}>
          <details className="text-[12px]">
            <summary className="cursor-default text-muted-foreground">
              Show labels
            </summary>
            <table className="mt-[4px] w-full border-collapse">
              <tbody>
                {labels.map(([k, v]) => (
                  <tr key={k}>
                    <th
                      scope="row"
                      className={`${cell} ${mono} w-[45%] text-left font-normal text-muted-foreground`}
                    >
                      {k}
                    </th>
                    <td className={`${cell} ${mono}`}>{v}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
        </Section>
      </div>
    </Scroll>
  );
}
