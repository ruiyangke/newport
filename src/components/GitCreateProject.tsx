import {
  GitForm,
  GitField,
  GitFieldHint,
  GitPathField,
  GitError,
  GitFormActions,
} from "./GitForm";
import { useState } from "react";
import type { GitBootstrapRequest } from "../domain/git";
import { gitPath } from "../domain/git";
import { Button, Input } from "./controls";
import { GitFolderChooser } from "./GitFolderChooser";
import { Modal } from "./Editors";

export function GitCreateProject({
  mode,
  serverId,
  serverName,
  busy,
  blockedReason: externalBlockedReason,
  pendingOperationIds = [],
  error,
  onClose,
  onCreate,
}: {
  mode: "init" | "clone";
  serverId: string;
  serverName: string;
  busy: boolean;
  blockedReason?: string;
  pendingOperationIds?: string[];
  error: string;
  onClose: () => void;
  onCreate: (request: GitBootstrapRequest, name: string) => Promise<unknown>;
}) {
  const [submittedId, setSubmittedId] = useState<string | null>(null);
  const blockedReason =
    externalBlockedReason ??
    (submittedId && pendingOperationIds.includes(submittedId)
      ? "Check the saved operation outcome before repeating this creation."
      : undefined);
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [browsing, setBrowsing] = useState(false);
  const [url, setUrl] = useState("");
  const [branch, setBranch] = useState(mode === "init" ? "main" : "");
  const [localError, setLocalError] = useState("");
  const title = mode === "init" ? "Create repository" : "Clone repository";
  return (
    <>
      <Modal title={title} busy={busy} onClose={onClose}>
        <GitForm
          dialog
          onSubmit={(event) => {
            event.preventDefault();
            if (busy || blockedReason) return;
            setLocalError("");
            try {
              if (!path.startsWith("/"))
                throw new Error("Enter an absolute path on the server.");
              const params = {
                operationId: crypto.randomUUID(),
                path: gitPath(path),
              };
              const request: GitBootstrapRequest =
                mode === "init"
                  ? {
                      method: "repo.init",
                      params: { ...params, initialBranch: branch.trim() },
                    }
                  : {
                      method: "repo.clone",
                      params: {
                        ...params,
                        url: url.trim(),
                        ...(branch.trim() ? { branch: branch.trim() } : {}),
                      },
                    };
              setSubmittedId(params.operationId);
              void onCreate(request, name);
            } catch (reason) {
              setLocalError(
                reason instanceof Error ? reason.message : String(reason),
              );
            }
          }}
        >
          <p>On {serverName}. The repository will be saved in Projects.</p>
          <GitField>
            Project name
            <Input
              autoFocus
              required
              value={name}
              onChange={(event) => setName(event.target.value)}
              disabled={busy}
            />
          </GitField>
          {mode === "clone" && (
            <GitField>
              Repository URL
              <Input
                required
                placeholder="git@github.com:owner/repository.git"
                value={url}
                onChange={(event) => setUrl(event.target.value)}
                disabled={busy}
                autoCapitalize="none"
                spellCheck={false}
              />
              <GitFieldHint>
                SSH uses the server’s SSH agent. HTTPS supports public
                repositories.
              </GitFieldHint>
            </GitField>
          )}
          <GitField>
            {mode === "init"
              ? "Existing directory on server"
              : "Destination on server"}
            <GitPathField>
              <Input
                required
                placeholder="/home/user/projects/app"
                value={path}
                onChange={(event) => setPath(event.target.value)}
                disabled={busy}
                autoCapitalize="none"
                spellCheck={false}
              />
              <Button
                type="button"
                disabled={busy}
                onClick={() => setBrowsing(true)}
              >
                Browse…
              </Button>
            </GitPathField>
            <GitFieldHint>
              {mode === "init"
                ? "Choose an existing directory without a Git repository. Its files will be kept."
                : "The parent directory must exist. The destination must not exist yet."}
            </GitFieldHint>
          </GitField>
          <GitField>
            {mode === "init" ? "Initial branch" : "Branch (optional)"}
            <Input
              required={mode === "init"}
              value={branch}
              placeholder={
                mode === "clone" ? "Remote default branch" : undefined
              }
              onChange={(event) => setBranch(event.target.value)}
              disabled={busy}
              autoCapitalize="none"
              spellCheck={false}
            />
          </GitField>
          {blockedReason && <p role="status">{blockedReason}</p>}
          {(localError || error) && (
            <GitError role="alert">{localError || error}</GitError>
          )}
          {busy && (
            <p role="status">
              {mode === "clone"
                ? "Cloning repository… This may take a few minutes."
                : "Creating repository…"}
            </p>
          )}
          <GitFormActions>
            <Button disabled={busy} onClick={onClose}>
              Cancel
            </Button>
            <Button
              type="submit"
              variant="default"
              loading={busy}
              disabled={busy || !!blockedReason}
            >
              {title}
            </Button>
          </GitFormActions>
        </GitForm>
      </Modal>
      <GitFolderChooser
        serverId={serverId}
        open={browsing}
        initialPath={path || undefined}
        title={
          mode === "init"
            ? "Choose the directory to initialize"
            : "Choose where to clone"
        }
        busy={busy}
        onCancel={() => setBrowsing(false)}
        onChoose={(chosen) => {
          setPath(chosen);
          setBrowsing(false);
        }}
      />
    </>
  );
}
