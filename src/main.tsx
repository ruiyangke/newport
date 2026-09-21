import { QueryClientProvider } from "@tanstack/react-query";
import { createQueryClient } from "./query/client";
import { applyAppearance } from "./appearance";
import { MemoryRouter } from "react-router";
import { Toaster } from "./components/ui/sonner";
import React from "react";
import ReactDOM from "react-dom/client";
import { TooltipProvider } from "./components/ui/tooltip";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles.css";
import "./components/dialogs.css";
const queryClient = createQueryClient();
applyAppearance();
window
  .matchMedia("(prefers-color-scheme: dark)")
  .addEventListener("change", () => applyAppearance());
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <TooltipProvider delayDuration={400}>
      <QueryClientProvider client={queryClient}>
        <MemoryRouter>
          <ErrorBoundary scope="app">
            <App />
          </ErrorBoundary>
        </MemoryRouter>
      </QueryClientProvider>
      <Toaster position="bottom-right" closeButton />
    </TooltipProvider>
  </React.StrictMode>,
);
