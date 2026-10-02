import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, Copy, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "./controls";

/** macOS keeps its native traffic lights; Windows shares the app toolbar. */
export function WindowControls() {
  const visible = isTauri() && navigator.platform.startsWith("Win");
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!visible) return;
    const window = getCurrentWindow();
    let disposed = false;
    let revision = 0;
    const update = async () => {
      const current = ++revision;
      const value = await window.isMaximized();
      if (!disposed && current === revision) setMaximized(value);
    };
    void update().catch(console.error);
    const stop = window
      .onResized(() => void update().catch(console.error))
      .catch((error) => {
        console.error(error);
        return undefined;
      });
    return () => {
      disposed = true;
      void stop.then((unlisten) => unlisten?.()).catch(console.error);
    };
  }, [visible]);
  if (!visible) return null;
  const run = (action: () => Promise<void>) => {
    void action().catch(() =>
      toast.error("Could not change the window. Try again."),
    );
  };
  return (
    <div
      data-slot="window-controls"
      className="ml-[6px] flex shrink-0 items-center gap-[2px] border-l border-border pl-[8px]"
      role="group"
      aria-label="Window controls"
    >
      <Button
        variant="ghost"
        size="icon"
        className="size-[32px] rounded-[5px]"
        aria-label="Minimize"
        title="Minimize"
        onClick={() => run(() => getCurrentWindow().minimize())}
      >
        <Minus size={14} />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-[32px] rounded-[5px]"
        aria-label={maximized ? "Restore" : "Maximize"}
        title={maximized ? "Restore" : "Maximize"}
        onClick={() => run(() => getCurrentWindow().toggleMaximize())}
      >
        {maximized ? <Copy size={13} /> : <Square size={13} />}
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-[32px] rounded-[5px] hover:bg-destructive hover:text-white"
        aria-label="Close window"
        title="Close window"
        onClick={() => run(() => getCurrentWindow().close())}
      >
        <X size={16} />
      </Button>
    </div>
  );
}
