import { SectionHeader } from "./SectionHeader";
import { Switch } from "./ui/switch";
import { desktop } from "../api/desktop";
import { active, type Server, type Snapshot } from "../types";
import { Button } from "./controls";
import { PageHeader } from "./PageHeader";
import { CodeBlock } from "./CodeBlock";
import { StatusLabel } from "./StatusLabel";

type Props = {
  server: Server;
  runtime: Snapshot["runtime"];
  pending: Set<string>;
  act: (
    key: string,
    task: () => Promise<unknown>,
    message?: string,
  ) => Promise<void>;
};

export function IntegrationPanel({ server, runtime, pending, act }: Props) {
  const key = `integration-${server.id}`;
  const reinstallKey = `reinstall-agent-${server.id}`;
  const busy = pending.has(key) || pending.has(reinstallKey);
  const enabled = server.clipboardEnabled || server.browserEnabled;
  const state = runtime.clipboard[server.id] ?? {
    status: "disconnected" as const,
    errorMessage: null,
    reconnectAttempt: 0,
  };
  const setFeature = (feature: "clipboard" | "browser", checked: boolean) =>
    void act(key, () =>
      desktop("set_integration_enabled", {
        id: server.id,
        feature,
        enabled: checked,
      }),
    );

  return (
    <div data-slot="integration-panel" className="mx-auto w-full max-w-[720px]">
      <PageHeader
        title="Integrations"
        subtitle="Enable only for servers you trust."
      />
      {(
        [
          [
            "clipboard",
            "Clipboard",
            "Share your computer clipboard with this server.",
            !!server.clipboardEnabled,
          ],
          [
            "browser",
            "Browser",
            "Open server links on your computer.",
            !!server.browserEnabled,
          ],
        ] as const
      ).map(([feature, title, description, checked]) => (
        <div
          className="flex items-center justify-between gap-[24px] border-b border-border py-[22px]"
          key={feature}
        >
          <div>
            <label
              className="cursor-pointer text-[14px] font-semibold"
              htmlFor={feature}
            >
              {title}
            </label>
            <p
              className="mt-[6px] text-[13px] leading-[1.6] text-muted-foreground"
              id={`${feature}-description`}
            >
              {description}
            </p>
          </div>
          <Switch
            id={feature}
            checked={checked}
            aria-describedby={`${feature}-description`}
            disabled={busy}
            onCheckedChange={(value) => setFeature(feature, value)}
          />
        </div>
      ))}
      <section className="mt-[30px]" aria-label="Agent connection">
        <SectionHeader>
          <h2 className="text-[14px] font-semibold">Connection</h2>
          {enabled ? (
            <StatusLabel state={state} />
          ) : (
            <span className="text-[12px] text-muted-foreground">Off</span>
          )}
        </SectionHeader>
        {!enabled ? (
          <p className="mt-[6px] text-[13px] leading-[1.6] text-muted-foreground">
            Enable an integration to connect.
          </p>
        ) : (
          <>
            {state.errorMessage && (
              <p
                className="mt-[6px] mb-[12px] text-[13px] leading-[1.6] whitespace-pre-wrap text-(--danger) wrap-anywhere"
                role="alert"
              >
                {state.errorMessage}
              </p>
            )}
            {!active(state.status) && (
              <Button
                loading={pending.has(key)}
                disabled={busy}
                onClick={() =>
                  setFeature(
                    server.clipboardEnabled ? "clipboard" : "browser",
                    true,
                  )
                }
              >
                Retry
              </Button>
            )}
            {state.status === "connected" && (
              <>
                <p className="mt-[6px] text-[13px] leading-[1.6] text-muted-foreground">
                  Open a new shell, or run this in your current Bash/Zsh shell.
                </p>
                <CodeBlock
                  className="mt-[16px]"
                  label="Shell configuration"
                  code={
                    'if [ -x "$HOME/.local/bin/newport-agent" ]; then\n  if _newport_env=$("$HOME/.local/bin/newport-agent" env 2>/dev/null); then\n    eval "$_newport_env"\n  fi\n  unset _newport_env\nfi'
                  }
                />
                {server.clipboardEnabled &&
                  runtime.clipboardPathNeeded?.[server.id] && (
                    <p className="mt-[6px] text-[13px] leading-[1.6] text-muted-foreground">
                      This also adds the clipboard commands to your PATH.
                    </p>
                  )}
              </>
            )}
          </>
        )}
        <div className="mt-[20px]">
          <Button
            loading={pending.has(reinstallKey)}
            disabled={busy}
            onClick={() =>
              void act(
                reinstallKey,
                () => desktop("reinstall_agent", { id: server.id }),
                "Agent reinstalled",
              )
            }
          >
            Reinstall agent
          </Button>
        </div>
      </section>
    </div>
  );
}
