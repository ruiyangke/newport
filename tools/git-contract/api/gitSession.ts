// Transport-free boundary for the protocol oracle. No desktop adapter.
import type { GitRequest } from "../domain/git";
export interface GitSession {
  request(request: GitRequest, signal?: AbortSignal): Promise<unknown>;
  forget(): Promise<void>;
}
export class GitOperationError extends Error {
  constructor(readonly operationId: string, readonly code: string, message: string) {
    super(message);
  }
}
