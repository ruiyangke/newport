import { UpdateSettings } from "./UpdateSettings";
import { useState } from "react";
import {
  queryOptions,
  useQuery,
  useQueryClient,
  useMutation,
} from "@tanstack/react-query";
import { fileSize } from "../domain/files";
import { keys } from "../query/keys";
import { readIPC, refreshQuery } from "../query/client";
import { isTauri } from "@tauri-apps/api/core";
import { desktop } from "../api/desktop";
import { getAppearance, setAppearance, type Appearance } from "../appearance";
import { Button, Checkbox, Select, SelectItem } from "./controls";
import "./settings.css";

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
  const mutation = useMutation({
    mutationFn: () => desktop("clear_metrics_cache"),
    onMutate: () => client.cancelQueries({ queryKey: keys.metricsCache }),
    onSuccess: (value) => client.setQueryData(keys.metricsCache, value),
    onSettled: async () => {
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
      await client.invalidateQueries({ queryKey: keys.metricsCache });
    },
  });
  const error = mutation.error || cache.error;
  return (
    <section aria-labelledby="cache-heading">
      <h2 id="cache-heading">Cache</h2>
      <div className="settings-row settings-cache-row">
        <div>
          <span>Metrics history</span>
          <small id="cache-help">
            Recorded history for all servers. New readings continue.
          </small>
        </div>
        <div className="settings-cache-actions">
          <span className="settings-cache-size" aria-label="Metrics cache size">
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
      </div>
      {!available && <p className="muted">Available in the desktop app.</p>}
      {error ? (
        <div className="settings-error" role="alert">
          <p>{String(error)}</p>
          {cache.error && (
            <Button onClick={() => void cache.refetch()}>
              Retry cache size
            </Button>
          )}
        </div>
      ) : mutation.isSuccess ? (
        <p role="status">Cache cleared.</p>
      ) : cache.data?.samples === 0 ? (
        <p className="muted">No recorded history.</p>
      ) : null}
    </section>
  );
}

export function SettingsPanel() {
  const [appearance, updateAppearance] = useState(getAppearance);
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
  const mutation = useMutation({
    mutationFn: (enabled: boolean) =>
      desktop("set_launch_at_login", { enabled }),
    onMutate: () => client.cancelQueries({ queryKey: keys.startup }),
    onSuccess: (value) => client.setQueryData(keys.startup, value),
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
    <div className="settings-panel" aria-label="App settings">
      <section aria-labelledby="appearance-heading">
        <h2 id="appearance-heading">Appearance</h2>
        <div className="settings-row">
          <label id="theme-label">Theme</label>
          <Select
            aria-labelledby="theme-label"
            value={appearance}
            onValueChange={(value) => {
              try {
                setAppearance(value as Appearance);
                updateAppearance(value as Appearance);
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
        </div>
      </section>
      <section aria-labelledby="startup-heading">
        <h2 id="startup-heading">Startup</h2>
        <label className="settings-row settings-toggle">
          <span>
            Launch at login
            <small id="startup-help">
              {startup && !startup.available
                ? "Available in the installed app."
                : "Start in the background when you sign in."}
            </small>
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
          <p className="muted" role="status">
            {busy ? "Saving…" : "Loading settings…"}
          </p>
        )}
      </section>
      <UpdateSettings />
      <MetricsCacheSettings />
      {error && (
        <div className="settings-error" role="alert">
          <p>{error}</p>
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
