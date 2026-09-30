import { desktop } from "./desktop";
import { GitSession } from "./gitSession";
import { GitRepositoryClient } from "./gitRepository";
import { GitMutations } from "./gitMutations";
import {
  gitPath,
  type GitBootstrapRequest,
  type GitPath,
  type GitProject,
} from "../domain/git";
import { decodeGitBytes, type GitOperation } from "../domain/gitResponses";

/** Creation is complete. Recover by adding the existing repository, never by replaying it. */
export class GitProjectCreatedError extends Error {
  constructor(
    readonly operation: GitOperation,
    readonly requestedPath: GitPath,
    cause: unknown,
  ) {
    super(
      "The repository was created, but opening or saving its project failed. Add the existing repository to recover; do not create it again.",
      { cause },
    );
    this.name = "GitProjectCreatedError";
  }
}

function projectName(value: string) {
  const name = value.trim();
  if (
    !name ||
    new TextEncoder().encode(name).length > 256 ||
    /\p{Cc}/u.test(name)
  ) {
    throw new Error(
      "Enter a project name of up to 256 bytes without control characters.",
    );
  }
  return name;
}

/** Own this alongside the server connection scope, and dispose on scope changes. */
export class GitProjects {
  private session?: GitSession;
  private client?: GitRepositoryClient;
  private closed = false;
  readonly mutations: GitMutations;

  constructor(readonly serverId: string) {
    this.mutations = new GitMutations(
      serverId,
      () => this.repositories,
      () => this.current(),
    );
  }

  private current() {
    if (this.closed) throw new Error("This project workspace is closed.");
  }

  get repositories(): GitRepositoryClient {
    this.current();
    if (!this.client) {
      this.session = new GitSession(this.serverId);
      this.client = new GitRepositoryClient(this.session);
    }
    return this.client;
  }

  async list() {
    this.current();
    const projects = await desktop("git_projects_list", {
      serverId: this.serverId,
    });
    this.current();
    return projects;
  }

  async add(path: string, name: string) {
    this.current();
    const label = projectName(name);
    const requestedPath = gitPath(path);
    return this.addPath(requestedPath, label);
  }

  async bootstrap(request: GitBootstrapRequest, name: string) {
    this.current();
    const label = projectName(name);
    const captured = structuredClone(request);
    const operation = await this.mutations.bootstrap(captured);
    if (operation.state !== "succeeded") return { operation, selection: null };
    try {
      this.current();
      // Use the canonical byte path returned by creation, never its display label
      // or the operation's opaque repository identity.
      const path = decodeGitBytes(operation.result?.path);
      const selection = await this.addPath(path, label);
      // The caller consumes the result before explicitly acknowledging its receipt.
      return { operation, selection };
    } catch (cause) {
      throw new GitProjectCreatedError(operation, captured.params.path, cause);
    }
  }

  private async addPath(requestedPath: GitPath, label: string) {
    this.current();
    const client = this.repositories;
    const repository = await client.open(requestedPath);
    try {
      this.current();
      const project = await desktop("git_projects_save", {
        project: {
          id: crypto.randomUUID(),
          serverId: this.serverId,
          name: label,
          // repo.open resolves subdirectories and symlinks to the repository root.
          path: repository.root,
        },
      });
      this.current();
      return { project, repository };
    } catch (error) {
      // Failed/duplicate bookmark saves must not exhaust the agent's handle limit.
      await this.release(client, repository.repoId);
      throw error;
    }
  }

  async open(project: GitProject, signal?: AbortSignal) {
    this.checkProject(project);
    signal?.throwIfAborted();
    const client = signal
      ? this.repositories.withSignal(signal)
      : this.repositories;
    const repository = await client.open(project.path);
    try {
      signal?.throwIfAborted();
      this.current();
      return repository;
    } catch (error) {
      await this.release(client, repository.repoId);
      throw error;
    }
  }

  async rename(project: GitProject, name: string) {
    this.checkProject(project);
    const saved = await desktop("git_projects_save", {
      project: { ...project, name: projectName(name) },
    });
    this.current();
    return saved;
  }

  async remove(project: GitProject) {
    this.checkProject(project);
    await desktop("git_projects_remove", {
      serverId: this.serverId,
      projectId: project.id,
    });
    this.current();
  }

  private checkProject(project: GitProject) {
    this.current();
    if (project.serverId !== this.serverId)
      throw new Error("Select this project's server before opening it.");
  }

  private async release(client: GitRepositoryClient, repoId: string) {
    await client.close(repoId).catch(async () => {
      await this.dispose().catch(() => undefined);
    });
  }

  async dispose() {
    this.closed = true;
    await this.session?.dispose();
  }
}
