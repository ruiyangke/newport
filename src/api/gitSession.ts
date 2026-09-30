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
  private writeBarrier: Promise<unknown> = Promise.resolve();
  private reads = new Set<Promise<unknown>>();
  private resetting?: Promise<void>;
  private closed = false;
  private disposal?: Promise<void>;

  constructor(private readonly serverId: string) {}

  /** The agent's capabilities, asked for once and again after a failure. */
  private async connect() {
    await this.resetting;
    if (this.closed) throw closedError();
    this.connection ??= desktop("git_connect", { serverId: this.serverId });
    const connection = this.connection;
    connection.catch(() => {
      if (this.connection === connection) this.connection = undefined;
    });
    return connection;
  }

  request(request: GitRequest, signal?: AbortSignal): Promise<unknown> {
    // Capture at enqueue time so caller edits cannot alter a queued request.
    const captured = structuredClone(request);
    const mutation = isGitMutation(captured);
    if (signal && mutation)
      return Promise.reject(
        new Error("Write requests cannot use read cancellation."),
      );
    const barrier = mutation
      ? Promise.all([this.writeBarrier, ...this.reads])
      : this.writeBarrier;
    const result = barrier.then(async () => {
      signal?.throwIfAborted();
      if (this.closed) throw closedError();
      const connection = await this.connect();
      signal?.throwIfAborted();
      const connectionAttempt = this.connection;
      if (this.closed) throw closedError();
      if (!connection.info.capabilities.methods.includes(captured.method)) {
        throw new Error("Update the server agent to use this Git feature.");
      }
      if (
        captured.method === "repo.status_summary" &&
        captured.params.path &&
        !connection.info.capabilities.features?.includes("status_summary.path")
      ) {
        throw new Error(
          "Update the server agent to read project summaries by path.",
        );
      }
      if (
        captured.method === "repo.worktrees" &&
        captured.params.atSnapshot !== undefined &&
        !connection.info.capabilities.features?.includes(
          "worktrees.snapshot_filter",
        )
      ) {
        throw new Error(
          "Update the server agent to search a captured worktree listing.",
        );
      }
      if (
        captured.method === "repo.worktrees" &&
        (captured.params.filter ||
          captured.params.branch ||
          captured.params.name) &&
        !connection.info.capabilities.features?.includes("worktrees.filter")
      ) {
        throw new Error(
          "Update the server agent to search and filter worktrees.",
        );
      }
      // Old agents reject new fields; retain explicitly labelled local filtering.
      if (
        captured.method === "repo.remote_refs" &&
        !connection.info.capabilities.features?.includes("remote_refs.filter")
      ) {
        if (captured.params.filter?.trim())
          throw new Error(
            "Update the server agent to search remote references.",
          );
        delete captured.params.filter;
      }
      if (
        captured.method === "repo.status" &&
        !connection.info.capabilities.features?.includes("status.filter")
      )
        delete captured.params.filter;
      if (
        captured.method === "repo.branches" &&
        (captured.params.filter ||
          (captured.params.branchKind &&
            captured.params.branchKind !== "all")) &&
        !connection.info.capabilities.features?.includes("branches.filter")
      ) {
        throw new Error(
          "Update the server agent to search and filter branches.",
        );
      }
      if (
        (captured.method === "repo.diff_page" ||
          captured.method === "repo.commit_diff_page") &&
        !connection.info.capabilities.features?.includes("diff.tuple_v1")
      )
        delete captured.params.lineEncoding;
      // Older agents reject unknown fields. Keep their full-message history
      // behavior while newer agents transfer just enough for the list.
      if (
        captured.method === "repo.history" &&
        !connection.info.capabilities.features?.includes("history.summary")
      )
        delete captured.params.messageBytes;
      if (
        captured.method === "repo.tags" &&
        !connection.info.capabilities.features?.includes("tags.summary")
      )
        delete captured.params.messageBytes;
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
      let readId: string | undefined;
      const cancelRead = () => {
        if (readId)
          void desktop("git_cancel_read", {
            serverId: this.serverId,
            readId,
          }).catch(() => undefined);
      };
      try {
        if (signal) {
          // Register before dispatch: even an abort during registration has a
          // native token to cancel, without an unbounded early-abort ledger.
          readId = await desktop("git_register_read", {
            serverId: this.serverId,
          });
          signal.addEventListener("abort", cancelRead, { once: true });
          if (signal.aborted) {
            cancelRead();
            signal.throwIfAborted();
          }
          if (this.closed) {
            cancelRead();
            throw closedError();
          }
        }
        const value = await desktop("git_request", {
          serverId: this.serverId,
          request: captured,
          ...(readId ? { readId } : {}),
        });
        signal?.throwIfAborted();
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
        signal?.throwIfAborted();
        if (
          typeof error === "object" &&
          error !== null &&
          "code" in error &&
          terminalCodes.has(String(error.code))
        ) {
          if (connectionAttempt && this.connection === connectionAttempt)
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
      } finally {
        signal?.removeEventListener("abort", cancelRead);
      }
    });
    // Native channels keep each stream sequential. Independent reads can
    // overlap, while writes wait for earlier reads and gate later requests.
    const settled = result.catch(() => undefined);
    if (mutation) this.writeBarrier = settled;
    else {
      this.reads.add(settled);
      void settled.then(() => this.reads.delete(settled));
    }
    return result;
  }

  /**
   * After a transport or protocol failure: let go of this connection so the
   * next request starts on a fresh one. Nothing is lost -- no request depends
   * on anything the old connection held -- and nothing is re-sent.
   */
  forget(): Promise<void> {
    this.connection = undefined;
    if (!this.resetting) {
      const reset = desktop("git_disconnect", {
        serverId: this.serverId,
      }).catch(() => undefined);
      this.resetting = reset;
      void reset.then(() => {
        if (this.resetting === reset) this.resetting = undefined;
      });
    }
    return this.resetting;
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
