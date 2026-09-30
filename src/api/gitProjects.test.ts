import { beforeEach, expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { GitProjects } from "./gitProjects";
import { gitPath } from "../domain/git";

const project = {
  id: "project",
  serverId: "server",
  name: "Example",
  path: gitPath("/canonical/root"),
};
const repository = {
  repoId: "handle",
  commonRepoId: "common",
  root: project.path,
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
beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(
    async (
      command: string,
      args: { request?: { method: string }; project?: unknown },
    ) => {
      if (command === "git_connect")
        return {
          serverId: "server",
          info: { capabilities: { methods: ["repo.open", "repo.close"] } },
        };
      if (command === "git_request")
        return args.request?.method === "repo.open"
          ? repository
          : { closed: true };
      if (command === "git_projects_save") return args.project;
      if (command === "git_projects_list") return [project];
    },
  );
});

it("lists, renames and removes local bookmarks without connecting to SSH", async () => {
  const workspace = new GitProjects("server");
  await expect(workspace.list()).resolves.toEqual([project]);
  await expect(workspace.rename(project, " Renamed ")).resolves.toMatchObject({
    name: "Renamed",
  });
  await workspace.remove(project);
  await workspace.dispose();
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "git_projects_list",
    "git_projects_save",
    "git_projects_remove",
  ]);
});

it("verifies a repository before saving its canonical path", async () => {
  const workspace = new GitProjects("server");
  const result = await workspace.add("/shortcut/subdirectory", " Example ");
  expect(result.project.path).toEqual(project.path);
  expect(result.project.name).toBe("Example");
  expect(result.repository.repoId).toBe("handle");
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "git_connect",
    "git_request",
    "git_projects_save",
  ]);
  expect(invoke).toHaveBeenCalledWith("git_request", {
    serverId: "server",
    request: {
      method: "repo.open",
      params: { path: gitPath("/shortcut/subdirectory") },
    },
  });
  await workspace.dispose();
});

it("closes the opened handle when a bookmark cannot be saved", async () => {
  const normal = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) =>
    command === "git_projects_save"
      ? Promise.reject(new Error("Already saved"))
      : normal(command, args),
  );
  const workspace = new GitProjects("server");
  await expect(workspace.add("/repo", "Example")).rejects.toThrow(
    "Already saved",
  );
  expect(invoke).toHaveBeenLastCalledWith("git_request", {
    serverId: "server",
    request: { method: "repo.close", params: { repoId: "handle" } },
  });
  await workspace.dispose();
});

it("rejects wrong-server projects and invalid names before any remote request", async () => {
  const workspace = new GitProjects("other");
  await expect(workspace.open(project)).rejects.toThrow("server");
  await expect(workspace.rename(project, "Rename")).rejects.toThrow("server");
  await expect(workspace.remove(project)).rejects.toThrow("server");
  await expect(workspace.add("/repo", "é".repeat(129))).rejects.toThrow("name");
  expect(invoke).not.toHaveBeenCalled();
  await workspace.dispose();
  await expect(workspace.list()).rejects.toThrow("closed");
});

it("does not save a repository returned after the server workspace was disposed", async () => {
  let finish!: (value: unknown) => void;
  const pending = new Promise((resolve) => {
    finish = resolve;
  });
  const normal = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) =>
    command === "git_request" ? pending : normal(command, args),
  );
  const workspace = new GitProjects("server");
  const adding = workspace.add("/repo", "Example");
  const rejected = expect(adding).rejects.toThrow("closed");
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("git_request", expect.anything()),
  );
  await workspace.dispose();
  finish(repository);
  await rejected;
  expect(
    invoke.mock.calls.some(([command]) => command === "git_projects_save"),
  ).toBe(false);
});

const creation = {
  operationId: "create-id",
  repository: "opaque-repository-identity",
  payloadHash: "hash",
  state: "succeeded" as const,
  seq: 1,
  result: { path: gitPath("/created/canonical") },
  error: null,
};
const initRequest = {
  method: "repo.init" as const,
  params: {
    operationId: creation.operationId,
    path: gitPath("/requested"),
    initialBranch: "main",
  },
};

it.each(["repo.init", "repo.clone"] as const)(
  "saves %s only after confirmed creation, using its canonical byte path",
  async (method) => {
    const workspace = new GitProjects("server");
    const bootstrap = vi
      .spyOn(workspace.mutations, "bootstrap")
      .mockResolvedValue(creation);
    const request =
      method === "repo.init"
        ? initRequest
        : {
            method,
            params: {
              operationId: creation.operationId,
              path: gitPath("/requested"),
              url: "https://example.com/repo.git",
            },
          };
    const result = await workspace.bootstrap(request, " Created ");
    expect(bootstrap).toHaveBeenCalledExactlyOnceWith(request);
    expect(result.selection?.project.name).toBe("Created");
    expect(result.selection?.project.path).toEqual(repository.root);
    expect(invoke).toHaveBeenCalledWith("git_request", {
      serverId: "server",
      request: { method: "repo.open", params: { path: creation.result.path } },
    });
    expect(
      invoke.mock.calls.some(
        ([command]) => command === "git_acknowledge_operation",
      ),
    ).toBe(false);
    await workspace.dispose();
  },
);

it.each(["failed", "outcome_unknown", "running", "needs_resolution"] as const)(
  "does not open or save a project for a %s creation",
  async (state) => {
    const workspace = new GitProjects("server");
    vi.spyOn(workspace.mutations, "bootstrap").mockResolvedValue({
      ...creation,
      state,
    });
    expect(await workspace.bootstrap(initRequest, "Example")).toMatchObject({
      operation: { state },
      selection: null,
    });
    expect(invoke).not.toHaveBeenCalled();
    await workspace.dispose();
  },
);

it("keeps the successful creation outcome when saving fails and releases the handle", async () => {
  const normal = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) =>
    command === "git_projects_save"
      ? Promise.reject(new Error("Disk full"))
      : normal(command, args),
  );
  const workspace = new GitProjects("server");
  const bootstrap = vi
    .spyOn(workspace.mutations, "bootstrap")
    .mockResolvedValue(creation);
  await expect(
    workspace.bootstrap(initRequest, "Example"),
  ).rejects.toMatchObject({
    name: "GitProjectCreatedError",
    operation: creation,
    requestedPath: initRequest.params.path,
    cause: new Error("Disk full"),
  });
  expect(bootstrap).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenLastCalledWith("git_request", {
    serverId: "server",
    request: { method: "repo.close", params: { repoId: "handle" } },
  });
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "git_acknowledge_operation",
    ),
  ).toBe(false);
  await workspace.dispose();
});

it("captures creation inputs and preserves the outcome when disposed during creation", async () => {
  const workspace = new GitProjects("server");
  let finish!: (value: typeof creation) => void;
  const bootstrap = vi
    .spyOn(workspace.mutations, "bootstrap")
    .mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
  const request = structuredClone(initRequest);
  const pending = workspace.bootstrap(request, "Example");
  request.params.path.display = "changed";
  expect(bootstrap.mock.calls[0][0].params.path).toEqual(
    initRequest.params.path,
  );
  await workspace.dispose();
  finish(creation);
  await expect(pending).rejects.toMatchObject({
    name: "GitProjectCreatedError",
    operation: creation,
  });
  expect(invoke).not.toHaveBeenCalled();
});

it("rejects an invalid project name before creating a repository", async () => {
  const workspace = new GitProjects("server");
  const bootstrap = vi.spyOn(workspace.mutations, "bootstrap");
  await expect(workspace.bootstrap(initRequest, " ")).rejects.toThrow("name");
  expect(bootstrap).not.toHaveBeenCalled();
  await workspace.dispose();
});

it("retains confirmed creation when its returned path is malformed, without guessing a path", async () => {
  const workspace = new GitProjects("server");
  vi.spyOn(workspace.mutations, "bootstrap").mockResolvedValue({
    ...creation,
    result: { path: { display: "/repo", bytesB64: "%%%" } },
  });
  await expect(
    workspace.bootstrap(initRequest, "Example"),
  ).rejects.toMatchObject({
    name: "GitProjectCreatedError",
    operation: { state: "succeeded" },
  });
  expect(invoke).not.toHaveBeenCalled();
  await workspace.dispose();
});

it("does not create a bookmark or repeat creation when opening the created repository fails", async () => {
  const normal = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) =>
    command === "git_request"
      ? Promise.reject(new Error("Repository unavailable"))
      : normal(command, args),
  );
  const workspace = new GitProjects("server");
  const bootstrap = vi
    .spyOn(workspace.mutations, "bootstrap")
    .mockResolvedValue(creation);
  await expect(
    workspace.bootstrap(initRequest, "Example"),
  ).rejects.toMatchObject({
    name: "GitProjectCreatedError",
    operation: creation,
  });
  expect(bootstrap).toHaveBeenCalledTimes(1);
  expect(
    invoke.mock.calls.some(([command]) => command === "git_projects_save"),
  ).toBe(false);
  await workspace.dispose();
});

it("forwards cancellation while opening a repository for a summary", async () => {
  const normal = invoke.getMockImplementation()!;
  let release!: (value: typeof repository) => void;
  invoke.mockImplementation((command, args) => {
    if (command === "git_register_read") return Promise.resolve("summary-open");
    if (command === "git_request" && args.request?.method === "repo.open")
      return new Promise((resolve) => {
        release = resolve;
      });
    return normal(command, args);
  });
  const workspace = new GitProjects("server");
  const controller = new AbortController();
  const pending = workspace
    .open(project, controller.signal)
    .catch((error) => error);
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  controller.abort();
  release(repository);
  expect(await pending).toMatchObject({ name: "AbortError" });
  expect(invoke).toHaveBeenCalledWith("git_cancel_read", {
    serverId: "server",
    readId: "summary-open",
  });
  await workspace.dispose();
});
