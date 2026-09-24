import { Switch } from "radix-ui";
import { desktop } from "../api/desktop";
import { active, type Server, type Snapshot } from "../types";
import { Button } from "./controls";
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
    <div className="integration-panel">
      <div className="integration-heading">
        <h2>Connect your computer and server</h2>
        <p>Enable only for servers you trust.</p>
      </div>
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
        <div className="integration-row" key={feature}>
          <div>
            <label htmlFor={feature}>{title}</label>
            <p id={`${feature}-description`}>{description}</p>
          </div>
          <Switch.Root
            id={feature}
            className="integration-switch"
            checked={checked}
            aria-describedby={`${feature}-description`}
            disabled={busy}
            onCheckedChange={(value) => setFeature(feature, value)}
          >
            <Switch.Thumb className="integration-switch-thumb" />
          </Switch.Root>
        </div>
      ))}
      <section className="integration-setup" aria-label="Agent connection">
        <div className="section-heading">
          <h2>Connection</h2>
          {enabled ? (
            <StatusLabel state={state} />
          ) : (
            <span className="integration-off">Off</span>
          )}
        </div>
        {!enabled ? (
          <p>Enable an integration to connect.</p>
        ) : (
          <>
            {state.errorMessage && (
              <p className="connection-error" role="alert">
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
                <p>
                  Run on the server, or add to <code>~/.bashrc</code> or{" "}
                  <code>~/.zshrc</code>.
                </p>
                <CodeBlock
                  label="Shell configuration"
                  code={'eval "$("$HOME/.local/bin/porthop-agent" env)"'}
                />
                {server.clipboardEnabled &&
                  runtime.clipboardPathNeeded?.[server.id] && (
                    <p className="integration-path-note">
                      This also adds the clipboard commands to your PATH.
                    </p>
                  )}
              </>
            )}
          </>
        )}
        <div className="integration-maintenance">
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
