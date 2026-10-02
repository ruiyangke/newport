import {
  SettingsSection,
  SettingsHeading,
  SettingsRow,
  SettingsHelp,
  SettingsMessage,
} from "./SettingsLayout";
import { useSharedMutation } from "../hooks/useSharedMutation";
import { UpdateSettings } from "./UpdateSettings";
import { useState } from "react";
import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { fileSize } from "../domain/files";
import { keys } from "../query/keys";
import { readIPC, refreshQuery } from "../query/client";
import { isTauri } from "@tauri-apps/api/core";
import { desktop } from "../api/desktop";
import { useAppearance, setAppearance, type Appearance } from "../appearance";
import { Button, Checkbox, Select, SelectItem } from "./controls";

function MetricsCacheSettings() {
  const client = useQueryClient();
  const available = isTauri();
  const options = queryOptions({
    queryKey: keys.metricsCache,
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("get_metrics_cache")),
  });
  const cache = useQuery({
    ...options,
    enabled: available,
    refetchOnMount: "always",
    refetchInterval: 10_000,
  });
  const mutation = useSharedMutation({
    mutationKey: ["settings", "clear-metrics"],
    mutationFn: () => desktop("clear_metrics_cache"),
    onMutate: () => client.cancelQueries({ queryKey: keys.metricsCache }),
    onSuccess: async (value) => {
      await client.cancelQueries({ queryKey: keys.metricsCache });
      client.setQueryData(keys.metricsCache, value);
      // Drop both chart history and cached overview readings. Late IPC replies
      // must not bring samples from before the clear back into the charts.
      const metrics = {
        predicate: (query: { queryKey: readonly unknown[] }) =>
          query.queryKey[0] === "history" ||
          (query.queryKey[0] === "server" &&
            query.queryKey[3] === "collection" &&
            query.queryKey[4] === "overview"),
      };
      await client.cancelQueries(metrics);
      client.removeQueries(metrics);
    },
    onSettled: () => client.invalidateQueries({ queryKey: keys.metricsCache }),
  });
  const error = mutation.error || cache.error;
  return (
    <SettingsSection aria-labelledby="cache-heading">
      <SettingsHeading id="cache-heading">Cache</SettingsHeading>
      <SettingsRow wrap>
        <div>
          <span>Metrics history</span>
          <SettingsHelp id="cache-help">
            Recorded history for all servers. New readings continue.
          </SettingsHelp>
        </div>
        <div className="ml-auto flex items-center gap-[12px]">
          <span
            className="whitespace-nowrap text-muted-foreground tabular-nums"
            aria-label="Metrics cache size"
          >
            {!available
              ? "—"
              : cache.data
                ? fileSize(cache.data.bytes)
                : cache.isPending
                  ? "Calculating…"
                  : "Unavailable"}
          </span>
          <Button
            loading={mutation.isPending}
            disabled={
              !available ||
              !cache.data ||
              (!cache.data.samples && !mutation.error)
            }
            aria-describedby="cache-help"
            onClick={() => mutation.mutate()}
          >
            Clear cache
          </Button>
        </div>
      </SettingsRow>
      {!available && (
        <SettingsMessage className="text-[12px] leading-[1.65] text-muted-foreground">
          Available in the desktop app.
        </SettingsMessage>
      )}
      {error ? (
        <div className="mt-[20px] [&_p]:text-(--danger)" role="alert">
          <SettingsMessage>{String(error)}</SettingsMessage>
          {cache.error && (
            <Button onClick={() => void cache.refetch()}>
              Retry cache size
            </Button>
          )}
        </div>
      ) : mutation.isSuccess ? (
        <SettingsMessage role="status">Cache cleared.</SettingsMessage>
      ) : cache.data?.samples === 0 ? (
        <SettingsMessage className="text-[12px] leading-[1.65] text-muted-foreground">
          No recorded history.
        </SettingsMessage>
      ) : null}
    </SettingsSection>
  );
}

export function SettingsPanel() {
  const appearance = useAppearance();
  const client = useQueryClient();
  const [appearanceError, setError] = useState("");
  const options = queryOptions({
    queryKey: keys.startup,
    queryFn: ({ signal }) =>
      readIPC(signal, () =>
        isTauri()
          ? desktop("get_startup_settings")
          : Promise.resolve({ enabled: false, available: false }),
      ),
  });
  const query = useQuery({ ...options, refetchOnMount: "always" });
  const mutation = useSharedMutation({
    mutationKey: ["settings", "startup"],
    mutationFn: (enabled: boolean) =>
      desktop("set_launch_at_login", { enabled }),
    onMutate: () => client.cancelQueries({ queryKey: keys.startup }),
    onSuccess: async (value) => {
      await client.cancelQueries({ queryKey: keys.startup });
      client.setQueryData(keys.startup, value);
    },
    onError: () =>
      refreshQuery(client, options).then(
        () => {},
        () => {},
      ),
  });
  const startup = query.data;
  const busy = mutation.isPending;
  const error =
    appearanceError ||
    (mutation.error
      ? String(mutation.error)
      : query.error
        ? String(query.error)
        : "");
  const changeStartup = (enabled: boolean) => {
    setError("");
    mutation.mutate(enabled);
  };
  return (
    <div
      data-slot="settings-panel"
      className="mx-auto max-w-[720px]"
      aria-label="App settings"
    >
      <SettingsSection aria-labelledby="appearance-heading">
        <SettingsHeading id="appearance-heading">Appearance</SettingsHeading>
        <SettingsRow>
          <label id="theme-label">Theme</label>
          <Select
            className="w-[160px]"
            aria-labelledby="theme-label"
            value={appearance}
            onValueChange={(value) => {
              try {
                setAppearance(value as Appearance);
                setError("");
              } catch {
                setError(
                  "Could not save your appearance preference. Try again.",
                );
              }
            }}
          >
            <SelectItem value="system">Follow system</SelectItem>
            <SelectItem value="light">Light</SelectItem>
            <SelectItem value="dark">Dark</SelectItem>
          </Select>
        </SettingsRow>
      </SettingsSection>
      <SettingsSection aria-labelledby="startup-heading">
        <SettingsHeading id="startup-heading">Startup</SettingsHeading>
        <label className="flex items-center justify-between gap-[24px] text-[12px]">
          <span>
            Launch at login
            <SettingsHelp id="startup-help">
              {startup && !startup.available
                ? "Available in the installed app."
                : "Start in the background when you sign in."}
            </SettingsHelp>
          </span>
          <Checkbox
            aria-describedby="startup-help"
            checked={startup?.enabled ?? false}
            disabled={!startup?.available || busy}
            aria-busy={busy || undefined}
            onCheckedChange={(checked) => void changeStartup(checked === true)}
          />
        </label>
        {((!startup && !error) || busy) && (
          <SettingsMessage
            className="text-[12px] leading-[1.65] text-muted-foreground"
            role="status"
          >
            {busy ? "Saving…" : "Loading settings…"}
          </SettingsMessage>
        )}
      </SettingsSection>
      <UpdateSettings />
      <MetricsCacheSettings />
      {error && (
        <div className="mt-[20px] [&_p]:text-(--danger)" role="alert">
          <SettingsMessage>{error}</SettingsMessage>
          {!startup && (
            <Button
              onClick={() => void refreshQuery(client, options).catch(() => {})}
            >
              Retry
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
