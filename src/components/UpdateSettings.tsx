import {
  SettingsSection,
  SettingsHeading,
  SettingsRow,
  SettingsHelp,
  SettingsMessage,
} from "./SettingsLayout";
import { useSharedMutation } from "../hooks/useSharedMutation";
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
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
        `Newport ${data.version} is ready to install. Restart from Settings to update.`,
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
  const check = useSharedMutation({
    mutationKey: ["updates", "check"],
    mutationFn: () => desktop("check_for_updates"),
    onSettled: () => client.invalidateQueries({ queryKey: updateKey }),
  });
  const install = useSharedMutation({
    mutationKey: ["updates", "install"],
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
      ? "Updates are unavailable in this build."
      : phase === "downloading"
        ? data?.total
          ? `Downloading… ${Math.min(100, Math.round((data.downloaded / data.total) * 100))}%`
          : "Downloading…"
        : phase === "checking"
          ? "Checking for updates…"
          : phase === "installing"
            ? "Installing…"
            : ready
              ? `Version ${data?.version} is ready to install.`
              : phase === "current"
                ? "You’re up to date."
                : "Updates download automatically. You choose when to restart.";
  return (
    <SettingsSection aria-labelledby="updates-heading">
      <SettingsHeading id="updates-heading">Updates</SettingsHeading>
      <SettingsRow wrap>
        <div>
          <span>Newport {data?.currentVersion ?? version}</span>
          <SettingsHelp role="status">{message}</SettingsHelp>
        </div>
        <Button
          disabled={!data?.enabled || busy}
          loading={busy}
          onClick={() => (ready ? setConfirm(true) : check.mutate())}
        >
          {ready ? "Restart to update" : "Check for updates"}
        </Button>
      </SettingsRow>
      {confirm && ready && (
        <div className="mt-[16px]">
          <SettingsMessage>
            Restarting disconnects active sessions and cancels transfers.
          </SettingsMessage>
          <div className="mt-[8px] flex justify-end gap-[8px]">
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
        <SettingsMessage
          className="mt-[20px] [&_p]:text-(--danger)"
          role="alert"
        >
          {String(problem)}
        </SettingsMessage>
      )}
    </SettingsSection>
  );
}
