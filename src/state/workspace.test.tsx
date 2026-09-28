// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, expect, it } from "vitest";
import { ServerScopeProvider } from "../query/keys";
import {
  retainWorkspaces,
  setWorkspaceValue,
  useWorkspaceState,
  workspaceStore,
} from "./workspace";
import type { Server } from "../types";

beforeEach(() => workspaceStore.setState(() => ({})));
it("keeps functional updates isolated and prunes obsolete connection scopes", () => {
  setWorkspaceValue("a", "files.page", (value) => value + 1);
  setWorkspaceValue("a", "files.page", (value) => value + 1);
  setWorkspaceValue("b", "files.page", 7);
  expect(workspaceStore.state.a["files.page"]).toBe(2);
  expect(workspaceStore.state.b["files.page"]).toBe(7);
  retainWorkspaces(new Set(["b"]));
  expect(Object.keys(workspaceStore.state)).toEqual(["b"]);
});
it("restores state after unmount and isolates different servers", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const element = document.createElement("div");
  const root = createRoot(element);
  const server = (id: string): Server => ({
    id,
    name: id,
    sshHost: id,
    sshUser: "user",
    sshPort: 22,
    authMethod: "publicKey",
    identityFile: null,
  });
  function Page({ id }: { id: string }) {
    const [value, setValue] = useWorkspaceState(id, "services.query");
    return (
      <button onClick={() => setValue("saved")}>{value || "empty"}</button>
    );
  }
  const render = (id: string) =>
    root.render(
      <ServerScopeProvider server={server(id)}>
        <Page id={id} />
      </ServerScopeProvider>,
    );
  try {
    await act(() => render("a"));
    await act(() => element.querySelector("button")!.click());
    await act(() => root.render(null));
    await act(() => render("b"));
    expect(element.textContent).toBe("empty");
    await act(() => render("a"));
    expect(element.textContent).toBe("saved");
  } finally {
    await act(() => root.unmount());
  }
});

it("does not publish an unchanged value", () => {
  setWorkspaceValue("a", "files.page", 3);
  const previous = workspaceStore.state;
  setWorkspaceValue("a", "files.page", (value) => value);
  expect(workspaceStore.state).toBe(previous);
});
