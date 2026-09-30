// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter, useLocation } from "react-router";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { FilesPanel } from "./FilesPanel";
import { TooltipProvider } from "./ui/tooltip";
import { createQueryClient } from "../query/client";
import { ServerScopeProvider } from "../query/keys";
import { workspaceStore } from "../state/workspace";
import { gitPath } from "../domain/git";
import type { Server } from "../types";

const server: Server = {
  id: "alpha",
  name: "alpha",
  sshUser: "developer",
  sshHost: "host",
  sshPort: 22,
  identityFile: null,
  authMethod: "publicKey",
};
const listing = {
  path: "/home/developer",
  entries: [
    {
      name: "docs",
      path: "/home/developer/docs",
      kind: "directory",
      size: null,
      modified: null,
      permissions: null,
    },
    {
      name: "readme.md",
      path: "/home/developer/readme.md",
      kind: "file",
      size: 12,
      modified: null,
      permissions: null,
    },
  ],
};
const repository = {
  repoId: "handle",
  commonRepoId: "common",
  root: gitPath("/home/developer/docs"),
  bare: false,
  objectFormat: "sha1",
  head: {
    oid: null,
    name: gitPath("refs/heads/main"),
    detached: false,
    unborn: true,
  },
  operationState: "Clean",
  integration: null,
  capabilities: { readOnly: false, workingTree: true },
};
type Args = Record<string, never> & {
  path?: string;
  project?: { path: { display: string }; name: string };
  request?: { method: string };
};
/** The bookmark save, replaced per test to fail or to finish late. */
let save: (args: Args) => Promise<unknown>;
let saved: Args["project"][];

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;
let location = "";
function Probe() {
  location = useLocation().pathname;
  return null;
}
function tree(revision: number) {
  return (
    <MemoryRouter initialEntries={["/servers/alpha/files"]}>
      <Probe />
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <ServerScopeProvider server={server} revision={revision}>
            <FilesPanel server={server} />
          </ServerScopeProvider>
        </TooltipProvider>
      </QueryClientProvider>
    </MemoryRouter>
  );
}
async function render(revision = 0) {
  await act(async () => root.render(tree(revision)));
  // The panel lists its initial folder on mount.
  await act(async () => undefined);
}
function button(label: string) {
  return [...document.querySelectorAll("button")].find(
    (element) => element.getAttribute("aria-label") === label,
  );
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => {
    resolve = yes;
  });
  return { promise, resolve };
}

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  workspaceStore.setState(() => ({}));
  client = createQueryClient();
  location = "";
  saved = [];
  save = async (args) => {
    saved.push(args.project);
    return args.project;
  };
  invoke.mockReset();
  invoke.mockImplementation(async (command: string, args: Args) => {
    switch (command) {
      case "files_list":
        return listing;
      case "files_cancel":
      case "git_disconnect":
        return undefined;
      case "git_connect":
        return {
          serverId: server.id,
          info: { capabilities: { methods: ["repo.open", "repo.close"] } },
        };
      case "git_request":
        return args.request?.method === "repo.open"
          ? repository
          : { closed: true };
      case "git_projects_save":
        return save(args);
      default:
        return undefined;
    }
  });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  client.clear();
});

it("offers the project action on folders only, never on files", async () => {
  await render();
  expect(button("Open docs as project")).toBeDefined();
  // A file cannot be a project; only its download stays.
  expect(button("Open readme.md as project")).toBeUndefined();
  expect(button("Download readme.md")).toBeDefined();
  // The open folder is reachable from the toolbar too.
  expect(button("Open this folder as project")).toBeDefined();
});

it("saves a bookmark for the folder's own path and name, then opens Projects", async () => {
  await render();
  await act(async () => button("Open docs as project")!.click());
  expect(saved).toHaveLength(1);
  expect(saved[0]).toMatchObject({
    serverId: "alpha",
    name: "docs",
    path: gitPath("/home/developer/docs"),
  });
  // The path Files browsed is the path repo.open was asked to resolve.
  expect(invoke).toHaveBeenCalledWith("git_request", {
    serverId: "alpha",
    request: {
      method: "repo.open",
      params: { path: gitPath("/home/developer/docs") },
    },
  });
  expect(location).toBe("/servers/alpha/projects");
});

it("keeps the user in Files and shows the real error when the save fails", async () => {
  save = async () => {
    throw new Error("A project already bookmarks that folder.");
  };
  await render();
  await act(async () => button("Open docs as project")!.click());
  const alert = host.querySelector(".files-project-error");
  expect(alert?.textContent).toContain(
    "A project already bookmarks that folder.",
  );
  expect(alert?.textContent).toContain("No project was saved");
  // Nothing was bookmarked, so Projects must not be opened.
  expect(location).toBe("/servers/alpha/files");
  // The failure clears on demand and the action stays available.
  await act(async () =>
    [...document.querySelectorAll("button")]
      .find((element) => element.textContent === "Dismiss")!
      .click(),
  );
  expect(host.querySelector(".files-project-error")).toBeNull();
  expect(button("Open docs as project")!.disabled).toBe(false);
});

it("discards a save that lands after the connection scope changed", async () => {
  const late = deferred<unknown>();
  save = async (args) => {
    saved.push(args.project);
    return late.promise;
  };
  await render();
  await act(async () => button("Open docs as project")!.click());
  expect(saved).toHaveLength(1);
  // The workspace reconnects while the save is still in flight.
  await render(1);
  await act(async () => {
    late.resolve(listing);
    for (let tick = 0; tick < 8; tick++) await Promise.resolve();
  });
  await act(async () => undefined);
  expect(location).toBe("/servers/alpha/files");
  expect(host.querySelector(".files-project-error")).toBeNull();
});
