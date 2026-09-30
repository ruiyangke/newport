import { desktop } from "./desktop";
import {
  isGitMutation,
  type GitConnection,
  type GitRequest,
} from "../domain/git";

const terminalCodes = new Set([
  "TRANSPORT_ERROR",
  "PROTOCOL_ERROR",
  "CANCELLED",
  "STALE_CONNECTION",
  "SESSION_NOT_FOUND",
  "OUTCOME_UNKNOWN",
]);
function closedError() {
  return new Error("Git session is closed. Reconnect to continue.");
}
export class GitOperationError extends Error {
  constructor(
    readonly operationId: string,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/**
 * Git requests for one server.
 *
 * The native side keeps one connection per server and reconnects it before a
 * request when it has closed or failed; the agent keeps no state a request
 * depends on. So a failure here is reported and nothing more: the next
 * request simply goes out again, over a fresh connection if it has to. Only
 * `dispose` ends this object, for a server that is gone or a deliberate reset.
 */
export class GitSession {
  private connection?: Promise<GitConnection>;
  private tail: Promise<unknown> = Promise.resolve();
  private closed = false;
  private disposal?: Promise<void>;

  constructor(private readonly serverId: string) {}

  /** The agent's capabilities, asked for once and again after a failure. */
  private connect() {
    this.connection ??= desktop("git_connect", { serverId: this.serverId });
    const connection = this.connection;
    connection.catch(() => {
      if (this.connection === connection) this.connection = undefined;
    });
    return connection;
  }

  request(request: GitRequest): Promise<unknown> {
    // Capture at enqueue time so caller edits cannot alter a queued request.
    const captured = structuredClone(request);
    const result = this.tail.then(async () => {
      if (this.closed) throw closedError();
      const connection = await this.connect();
      if (this.closed) throw closedError();
      if (!connection.info.capabilities.methods.includes(captured.method)) {
        throw new Error("Update the server agent to use this Git feature.");
      }
      const operationId = isGitMutation(captured)
        ? captured.params.operationId
        : null;
      if (
        captured.method === "operation.start" &&
        !connection.info.capabilities.actions?.includes(
          captured.params.action.kind,
        )
      ) {
        throw new Error("This server agent does not support that Git action.");
      }
      if (
        captured.method === "operation.start" &&
        ["stash.apply", "stash.pop", "stash.drop"].includes(
          captured.params.action.kind,
        ) &&
        "index" in captured.params.action &&
        captured.params.action.index !== undefined &&
        !connection.info.capabilities.features?.includes("stash.entry_index")
      ) {
        throw new Error(
          "Update the server agent to select an exact stash entry.",
        );
      }
      if (
        captured.method === "operation.start" &&
        "hunks" in captured.params.action &&
        captured.params.action.hunks !== undefined &&
        !connection.info.capabilities.features?.includes("index.hunks")
      ) {
        throw new Error("Update the server agent to stage individual hunks.");
      }
      if (
        captured.method === "operation.start" &&
        "hunks" in captured.params.action &&
        captured.params.action.hunks?.lines !== undefined &&
        !connection.info.capabilities.features?.includes("index.lines")
      ) {
        throw new Error("Update the server agent to stage individual lines.");
      }
      if (
        captured.method === "operation.start" &&
        captured.params.action.kind === "conflict.resolve" &&
        !connection.info.capabilities.actions?.includes("conflict.resolve")
      ) {
        throw new Error(
          "Update the server agent to resolve conflicts by choosing a side.",
        );
      }
      if (
        captured.method === "operation.start" &&
        captured.params.action.kind === "worktree.add" &&
        captured.params.action.newBranch &&
        !connection.info.capabilities.features?.includes("worktree.new_branch")
      ) {
        throw new Error(
          "Update the server agent to create a worktree on a new branch.",
        );
      }
      if (
        captured.method === "operation.start" &&
        captured.params.action.kind === "discard" &&
        captured.params.action.hunks !== undefined &&
        !connection.info.capabilities.features?.includes("discard.hunks")
      ) {
        throw new Error("Update the server agent to discard selected hunks.");
      }
      try {
        const value = await desktop("git_request", {
          serverId: this.serverId,
          request: captured,
        });
        if (this.closed) {
          if (operationId)
            throw new GitOperationError(
              operationId,
              "OUTCOME_UNKNOWN",
              "The operation may have completed. Reconnect and check its outcome before trying again.",
            );
          throw closedError();
        }
        return value;
      } catch (error) {
        if (
          typeof error === "object" &&
          error !== null &&
          "code" in error &&
          terminalCodes.has(String(error.code))
        ) {
          void this.forget();
        }
        if (operationId) {
          const code =
            error && typeof error === "object" && "code" in error
              ? String(error.code)
              : "OUTCOME_UNKNOWN";
          const message =
            error && typeof error === "object" && "message" in error
              ? String(error.message)
              : String(error);
          throw new GitOperationError(
            operationId,
            terminalCodes.has(code) ? "OUTCOME_UNKNOWN" : code,
            message,
          );
        }
        throw error;
      }
    });
    // The agent allows one in-flight request; a failed read must not poison the queue.
    this.tail = result.catch(() => undefined);
    return result;
  }

  /**
   * After a transport or protocol failure: let go of this connection so the
   * next request starts on a fresh one. Nothing is lost -- no request depends
   * on anything the old connection held -- and nothing is re-sent.
   */
  forget(): Promise<void> {
    this.connection = undefined;
    return desktop("git_disconnect", { serverId: this.serverId }).catch(
      () => undefined,
    );
  }

  /** Ends this object: the server is gone, or the user reset the connection. */
  dispose(): Promise<void> {
    this.closed = true;
    // Do not wait for the request queue: disconnect cancels an in-flight read.
    this.disposal ??= desktop("git_disconnect", {
      serverId: this.serverId,
    }).catch(() => undefined);
    return this.disposal;
  }
}
