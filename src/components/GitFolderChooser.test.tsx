// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { FileEntry, FileListing } from "../domain/files";
import { GitFolderChooser } from "./GitFolderChooser";
import { TooltipProvider } from "./ui/tooltip";

const api = vi.hoisted(() => ({ desktop: vi.fn(), collect: vi.fn() }));
vi.mock("../api/desktop", () => api);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const directory = (path: string): FileEntry => ({
  name: path.split("/").filter(Boolean).at(-1) ?? "/",
  path,
  kind: "directory",
  size: null,
  modified: null,
  permissions: null,
});
const file = (path: string): FileEntry => ({
  ...directory(path),
  kind: "file",
  size: 12,
});
const listings = new Map<string, FileListing>();
const held = new Map<string, ReturnType<typeof deferred<FileListing>>>();

let root: Root;
let host: HTMLDivElement;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  listings.clear();
  held.clear();
  listings.set("/", { path: "/", entries: [directory("/srv")] });
  listings.set("/srv", {
    path: "/srv",
    entries: [
      directory("/srv/projects"),
      directory("/srv/logs"),
      directory("/srv/.cache"),
      file("/srv/notes.txt"),
    ],
  });
  listings.set("/srv/projects", {
    path: "/srv/projects",
    entries: [directory("/srv/projects/newport")],
  });
  listings.set("/srv/projects/newport", {
    path: "/srv/projects/newport",
    entries: [directory("/srv/projects/newport/src")],
  });
  api.desktop.mockReset();
  api.desktop.mockImplementation(
    (command: string, args: { path?: string; operation?: string }) => {
      if (command === "files_cancel") return Promise.resolve();
      if (command !== "files_list")
        return Promise.reject(new Error(`unexpected ${command}`));
      const path = args.path ?? "";
      const waiting = held.get(path);
      if (waiting) return waiting.promise;
      const listing = listings.get(path);
      return listing
        ? Promise.resolve(listing)
        : Promise.reject(new Error(`No such folder: ${path}`));
    },
  );
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

const buttons = () => [...document.querySelectorAll("button")];
function button(label: string) {
  return buttons().find(
    (b) => b.textContent === label || b.getAttribute("aria-label") === label,
  )!;
}
function folder(name: string) {
  return [
    ...document.querySelectorAll<HTMLButtonElement>(".git-folder-name"),
  ].find((b) => b.textContent === name)!;
}
const folderNames = () =>
  [...document.querySelectorAll(".git-folder-name")].map((b) => b.textContent);
function pathField() {
  const label = [...document.querySelectorAll("label")].find(
    (element) => element.textContent === "Server path",
  )!;
  return document.getElementById(label.htmlFor) as HTMLInputElement;
}
async function typePath(value: string) {
  const input = pathField();
  await act(async () => {
    // React tracks the node's own value, so bypass its tracker to type.
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () =>
    input
      .closest("form")!
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })),
  );
}
function render(
  overrides: Partial<Parameters<typeof GitFolderChooser>[0]> = {},
) {
  const onChoose = vi.fn();
  const onCancel = vi.fn();
  return {
    onChoose,
    onCancel,
    node: (
      <TooltipProvider>
        <GitFolderChooser
          serverId="server"
          open
          initialPath="/srv"
          onCancel={onCancel}
          onChoose={onChoose}
          {...overrides}
        />
      </TooltipProvider>
    ),
  };
}

it("lists the directories and never offers a file as a folder", async () => {
  const { node } = render();
  await act(async () => root.render(node));
  expect(api.desktop).toHaveBeenCalledWith("files_list", {
    id: "server",
    operation: expect.any(String),
    path: "/srv",
  });
  expect(folderNames()).toEqual(["projects", "logs", "notes.txt"]);
  // The file is shown, but it is not a control the user can act on.
  expect(folder("notes.txt").tagName).toBe("SPAN");
  expect(folder("projects").tagName).toBe("BUTTON");
  expect(button("Choose notes.txt")).toBeUndefined();
  // Dot folders stay out of the way until they are asked for.
  expect(folderNames()).not.toContain(".cache");
  await act(async () => button("Hidden folders").click());
  expect(folderNames()).toContain(".cache");
});

it("navigates into a folder and back up again", async () => {
  const { node } = render();
  await act(async () => root.render(node));
  await act(async () => folder("projects").click());
  expect(folderNames()).toEqual(["newport"]);
  expect(pathField().value).toBe("/srv/projects");
  await act(async () => button("Parent folder").click());
  expect(folderNames()).toEqual(["projects", "logs", "notes.txt"]);
  expect(pathField().value).toBe("/srv");
});

it("navigates to a path that was typed into the field", async () => {
  const { node } = render();
  await act(async () => root.render(node));
  await typePath("/srv/projects/newport");
  expect(api.desktop).toHaveBeenLastCalledWith("files_list", {
    id: "server",
    operation: expect.any(String),
    path: "/srv/projects/newport",
  });
  expect(folderNames()).toEqual(["src"]);
});

it("returns the folder on screen when it is chosen", async () => {
  const { onChoose, node } = render();
  await act(async () => root.render(node));
  await act(async () => folder("projects").click());
  await act(async () => button("Choose this folder").click());
  expect(onChoose).toHaveBeenCalledWith("/srv/projects");
});

it("returns a row's own folder from its choose control", async () => {
  const { onChoose, node } = render();
  await act(async () => root.render(node));
  await act(async () => button("Choose logs").click());
  expect(onChoose).toHaveBeenCalledWith("/srv/logs");
});

it("drops a superseded listing instead of letting it overwrite the newer one", async () => {
  const { node } = render();
  await act(async () => root.render(node));
  const slow = deferred<FileListing>();
  held.set("/srv/logs", slow);
  await act(async () => folder("logs").click());
  const stale = api.desktop.mock.calls.at(-1)![1].operation as string;
  // The user gives up waiting and goes somewhere else.
  await act(async () => folder("projects").click());
  expect(folderNames()).toEqual(["newport"]);
  expect(api.desktop).toHaveBeenCalledWith("files_cancel", {
    operation: stale,
  });
  await act(async () => {
    slow.resolve({ path: "/srv/logs", entries: [directory("/srv/logs/old")] });
    await slow.promise;
  });
  expect(folderNames()).toEqual(["newport"]);
  expect(pathField().value).toBe("/srv/projects");
});

it("shows a failed listing's reason and keeps the folder that was open", async () => {
  const { node } = render();
  await act(async () => root.render(node));
  await typePath("/srv/missing");
  expect(document.body.textContent).toContain(
    "Cannot open /srv/missing: Error: No such folder: /srv/missing",
  );
  // The dialog does not blank out: the last good listing is still there.
  expect(folderNames()).toEqual(["projects", "logs", "notes.txt"]);
  expect(document.querySelector('[role="alert"]')).not.toBeNull();
});

it("offers the repository root and the picked folder as separate answers", async () => {
  const resolveRoot = vi.fn(async () => "/srv/projects/newport");
  const { onChoose, node } = render({
    initialPath: "/srv/projects/newport",
    resolveRoot,
  });
  await act(async () => root.render(node));
  await act(async () => button("Choose src").click());
  expect(resolveRoot).toHaveBeenCalledWith("/srv/projects/newport/src");
  expect(document.body.textContent).toContain(
    "You picked /srv/projects/newport/src; its repository root is /srv/projects/newport.",
  );
  expect(onChoose).not.toHaveBeenCalled();
  await act(async () => button("Use repository root").click());
  expect(onChoose).toHaveBeenCalledWith("/srv/projects/newport");
  await act(async () => button("Use the folder I picked").click());
  expect(onChoose).toHaveBeenLastCalledWith("/srv/projects/newport/src");
});

it("chooses straight through when the folder is already the root", async () => {
  const resolveRoot = vi.fn(async () => "/srv/projects");
  const { onChoose, node } = render({
    initialPath: "/srv/projects",
    resolveRoot,
  });
  await act(async () => root.render(node));
  await act(async () => button("Choose this folder").click());
  expect(onChoose).toHaveBeenCalledWith("/srv/projects");
  expect(document.body.textContent).not.toContain("repository root is");
});

it("renders nothing while it is closed", async () => {
  const { node } = render({ open: false });
  await act(async () => root.render(node));
  expect(document.querySelector(".git-folder-list")).toBeNull();
  expect(api.desktop).not.toHaveBeenCalled();
});
