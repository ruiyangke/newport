import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { isTauri } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { desktop } from "../api/desktop";
import { Button } from "./controls";
import { version } from "../../package.json";

const updateKey = ["app-update"] as const;
function useUpdateStatus() {
  return useQuery({
    queryKey: updateKey,
    queryFn: () => desktop("update_status"),
    enabled: isTauri(),
    refetchInterval: 3_000,
    refetchIntervalInBackground: true,
    retry: false,
  });
}
export function UpdateNotice() {
  const { data } = useUpdateStatus();
  const notified = useRef<string | null>(null);
  useEffect(() => {
    if (data?.phase === "ready" && data.version !== notified.current) {
      notified.current = data.version;
      toast.info(
        `Porthop ${data.version} is ready. Restart from Settings to update.`,
        {
          id: "app-update",
        },
      );
    }
  }, [data]);
  return null;
}
export function UpdateSettings() {
  const { data, error } = useUpdateStatus();
  const client = useQueryClient();
  const [confirm, setConfirm] = useState(false);
  const check = useMutation({
    mutationFn: () => desktop("check_for_updates"),
    onSettled: () => client.invalidateQueries({ queryKey: updateKey }),
  });
  const install = useMutation({
    mutationFn: () => desktop("install_update"),
    onSettled: () => client.invalidateQueries({ queryKey: updateKey }),
  });
  const phase = data?.phase;
  const busy =
    check.isPending ||
    install.isPending ||
    ["checking", "downloading", "installing"].includes(phase ?? "");
  const ready = phase === "ready";
  const problem = install.error || check.error || data?.error || error;
  const message =
    !isTauri() || (data && !data.enabled)
      ? "Available in the installed app."
      : phase === "downloading"
        ? data?.total
          ? `Downloading… ${Math.min(100, Math.round((data.downloaded / data.total) * 100))}%`
          : "Downloading…"
        : phase === "checking"
          ? "Checking for updates…"
          : phase === "installing"
            ? "Installing…"
            : ready
              ? `Version ${data?.version} is ready.`
              : phase === "current"
                ? "You’re up to date."
                : "Updates download automatically. You choose when to restart.";
  return (
    <section aria-labelledby="updates-heading">
      <h2 id="updates-heading">Updates</h2>
      <div className="settings-row settings-cache-row">
        <div>
          <span>Porthop {data?.currentVersion ?? version}</span>
          <small role="status">{message}</small>
        </div>
        <Button
          disabled={!data?.enabled || busy}
          loading={busy}
          onClick={() => (ready ? setConfirm(true) : check.mutate())}
        >
          {ready ? "Restart to update" : "Check for updates"}
        </Button>
      </div>
      {confirm && ready && (
        <div className="update-confirmation">
          <p>Restarting disconnects active sessions and cancels transfers.</p>
          <div className="update-actions">
            <Button
              variant="ghost"
              onClick={() => setConfirm(false)}
              disabled={install.isPending}
            >
              Later
            </Button>
            <Button
              variant="default"
              onClick={() => install.mutate()}
              disabled={install.isPending}
            >
              Restart now
            </Button>
          </div>
        </div>
      )}
      {problem && (
        <p className="settings-error" role="alert">
          {String(problem)}
        </p>
      )}
    </section>
  );
}
