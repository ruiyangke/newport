import { lazy, memo, Suspense } from "react";
import { type Server } from "../types";
import { OverviewPanel } from "./OverviewPanel";
import { ServicesPage } from "./services/ServicesPage";
import { ContainersPage } from "./containers/ContainersPage";

const TerminalPanel = lazy(() => import("./TerminalPanel"));

export const Cockpit = memo(function Cockpit({
  server,
  tab = "overview",
}: {
  server: Server;
  tab?: string;
}) {
  // Overview scrolls with the workspace; the others fill the window.
  return (
    <div
      className={
        tab === "overview"
          ? "cockpit"
          : "cockpit flex min-h-0 min-w-0 flex-1 flex-col p-0"
      }
    >
      {tab === "overview" ? (
        <OverviewPanel id={server.id} />
      ) : tab === "services" ? (
        <ServicesPage id={server.id} />
      ) : tab === "containers" ? (
        <ContainersPage id={server.id} />
      ) : (
        <Suspense
          fallback={
            <p className="text-[12px] leading-[1.65] text-muted-foreground">
              Loading terminal…
            </p>
          }
        >
          <TerminalPanel key={server.id} server={server} />
        </Suspense>
      )}
    </div>
  );
});
