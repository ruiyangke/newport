// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, expect, it } from "vitest";
import { ServerScopeProvider } from "../query/keys";
import {
  retainWorkspaces,
  setWorkspaceValue,
  patchWorkspaceValue,
  useWorkspaceState,
  workspaceStore,
} from "./workspace";
import type { Server } from "../types";

beforeEach(() => workspaceStore.setState(() => ({})));
it("keeps functional updates isolated and prunes obsolete server scopes", () => {
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

it("keeps UI state across connection revisions and restarts", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const element = document.createElement("div");
  const root = createRoot(element);
  const server: Server = {
    id: "a",
    name: "a",
    sshHost: "a",
    sshUser: "user",
    sshPort: 22,
    authMethod: "publicKey",
    identityFile: null,
  };
  function Page() {
    const [value, setValue] = useWorkspaceState("a", "services.query");
    return (
      <button onClick={() => setValue("saved")}>{value || "empty"}</button>
    );
  }
  const render = (revision: number, instanceId: string) =>
    root.render(
      <ServerScopeProvider
        server={server}
        revision={revision}
        instanceId={instanceId}
      >
        <Page />
      </ServerScopeProvider>,
    );
  try {
    await act(() => render(0, "i"));
    await act(() => element.querySelector("button")!.click());
    expect(element.textContent).toBe("saved");
    // A credential revision bump or backend restart changes the connection
    // but must not wipe UI choices.
    await act(() => render(1, "i"));
    expect(element.textContent).toBe("saved");
    await act(() => render(1, "ii"));
    expect(element.textContent).toBe("saved");
  } finally {
    await act(() => root.unmount());
  }
});

it("gives every scope its own Set instances", () => {
  setWorkspaceValue("a", "files.page", 1);
  setWorkspaceValue("b", "files.page", 2);
  const a = workspaceStore.state.a["containers.collapsed"];
  const b = workspaceStore.state.b["containers.collapsed"];
  expect(a).not.toBe(b);
  (a as Set<string>).add("x");
  expect(b.has("x")).toBe(false);
});

it("applies a patch in one update", () => {
  let notifications = 0;
  const subscription = workspaceStore.subscribe(() => {
    notifications += 1;
  });
  try {
    patchWorkspaceValue("a", { "files.path": "/x", "files.input": "/x" });
    expect(notifications).toBe(1);
    expect(workspaceStore.state.a["files.path"]).toBe("/x");
    expect(workspaceStore.state.a["files.input"]).toBe("/x");
  } finally {
    subscription.unsubscribe();
  }
});

it("does not publish an unchanged value", () => {
  setWorkspaceValue("a", "files.page", 3);
  const previous = workspaceStore.state;
  setWorkspaceValue("a", "files.page", (value) => value);
  expect(workspaceStore.state).toBe(previous);
});
