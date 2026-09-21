// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { afterEach, beforeEach, expect, it } from "vitest";
import { useWorkspaceNavigation } from "./useWorkspaceNavigation";

let root: Root;
let host: HTMLDivElement;
let state: ReturnType<typeof useWorkspaceNavigation>;
let navigate: ReturnType<typeof useNavigate>;
let pathname: string;
let servers: { id: string }[];
let ready: boolean;
function Probe() {
  state = useWorkspaceNavigation(servers, ready);
  navigate = useNavigate();
  pathname = useLocation().pathname;
  return null;
}
async function render(entry = "/") {
  await act(async () => {
    root.render(
      <StrictMode>
        <MemoryRouter initialEntries={[entry]}>
          <Probe />
        </MemoryRouter>
      </StrictMode>,
    );
  });
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  servers = [{ id: "a" }, { id: "b" }];
  ready = true;
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

it("selects the first server and supports Back/Forward without changing the browser URL", async () => {
  const browserUrl = window.location.href;
  await render();
  expect(pathname).toBe("/servers/a/overview");
  await act(async () => state.selectView("commands"));
  await act(async () => state.selectServer("b"));
  expect(pathname).toBe("/servers/b/commands");
  await act(async () => navigate(-1));
  expect([state.selected, state.view]).toEqual(["a", "commands"]);
  await act(async () => navigate(-1));
  expect(state.view).toBe("overview");
  await act(async () => navigate(1));
  expect(state.view).toBe("commands");
  expect(window.location.href).toBe(browserUrl);
});

it("does not add duplicate history entries when reselecting the active view", async () => {
  await render();
  await act(async () => state.selectView("connections"));
  await act(async () => state.selectView("connections"));
  await act(async () => state.selectView("invalid"));
  await act(async () => navigate(-1));
  expect(state.view).toBe("overview");
});

it("waits for profiles before resolving an initial route", async () => {
  servers = [];
  ready = false;
  await render("/servers/b/containers");
  expect(pathname).toBe("/servers/b/containers");
  servers = [{ id: "a" }, { id: "b" }];
  ready = true;
  await render();
  expect([state.selected, state.view]).toEqual(["b", "containers"]);
});

it("repairs deleted servers and handles an empty profile", async () => {
  await render("/servers/b/services");
  servers = [{ id: "a" }];
  await render();
  expect(pathname).toBe("/servers/a/services");
  servers = [];
  await render();
  expect(pathname).toBe("/");
  expect(state.selected).toBe("");
  await act(async () => state.selectView("commands"));
  expect(pathname).toBe("/");
  servers = [{ id: "new" }];
  await render();
  expect(pathname).toBe("/servers/new/overview");
});

it.each(["/unknown", "/servers/missing/invalid", "/servers/a"])(
  "repairs invalid or incomplete route %s",
  async (entry) => {
    await render(entry);
    expect(pathname).toBe("/servers/a/overview");
  },
);

it("keeps Settings available without a server and preserves navigation history", async () => {
  servers = [];
  await render("/settings");
  expect(state.settings).toBe(true);
  expect(pathname).toBe("/settings");
  servers = [{ id: "a" }];
  await render();
  await act(async () => state.selectServer("a"));
  expect(state.settings).toBe(false);
  await act(async () => state.openSettings());
  expect(pathname).toBe("/settings");
  await act(async () => navigate(-1));
  expect(pathname).toBe("/servers/a/overview");
});

it("keeps the selected server and view while visiting Settings", async () => {
  await render("/servers/b/services");
  await act(async () => state.openSettings());
  expect(state.selected).toBe("b");
  expect(state.view).toBe("services");
  await act(async () => state.selectView("connections"));
  expect(pathname).toBe("/servers/b/connections");
});
