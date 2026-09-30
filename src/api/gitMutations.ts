import { desktop } from "./desktop";
import type { GitRepositoryClient } from "./gitRepository";
import type { GitBootstrapRequest, GitWriteRequest } from "../domain/git";

type Client = Pick<GitRepositoryClient, "start" | "operation" | "bootstrap">;

/** The native boundary persists intent; this layer sequences user actions and recovery. */
export class GitMutations {
  private busy = false;

  constructor(
    readonly serverId: string,
    private readonly getClient: () => Client,
    private readonly ensureCurrent: () => void,
  ) {}

  async receipts() {
    this.ensureCurrent();
    const receipts = await desktop("git_pending_operations", {
      serverId: this.serverId,
    });
    this.ensureCurrent();
    return receipts;
  }

  start(params: GitWriteRequest["params"]) {
    const captured = structuredClone(params);
    return this.dispatch((client) => client.start(captured));
  }

  bootstrap(request: GitBootstrapRequest) {
    const captured = structuredClone(request);
    return this.dispatch((client) => client.bootstrap(captured));
  }

  private dispatch(action: (client: Client) => ReturnType<Client["start"]>) {
    return this.exclusive(async () => {
      const receipts = await this.receipts();
      if (
        receipts.some(
          (receipt) =>
            receipt.state === "pending" || receipt.state === "outcome_unknown",
        )
      ) {
        throw new Error(
          "Check the earlier Git operation’s outcome before making more changes on this server.",
        );
      }
      this.ensureCurrent();
      // The native boundary records intent before dispatch. Never replay implicitly.
      return action(this.getClient());
    });
  }

  check(operationId: string) {
    return this.exclusive(async () => {
      this.ensureCurrent();
      const result = await this.getClient().operation(operationId);
      this.ensureCurrent();
      return result;
    });
  }

  acknowledge(operationId: string) {
    return this.exclusive(async () => {
      this.ensureCurrent();
      // Native storage checks the latest receipt; stale UI state cannot dismiss uncertainty.
      await desktop("git_acknowledge_operation", {
        serverId: this.serverId,
        operationId,
      });
      this.ensureCurrent();
    });
  }

  private async exclusive<T>(action: () => Promise<T>): Promise<T> {
    this.ensureCurrent();
    if (this.busy) throw new Error("Another Git operation is still running.");
    this.busy = true;
    try {
      return await action();
    } finally {
      this.busy = false;
    }
  }
}
