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
  blockedReason,
  error,
  onClose,
  onCreate,
}: {
  mode: "init" | "clone";
  serverId: string;
  serverName: string;
  busy: boolean;
  blockedReason?: string;
  error: string;
  onClose: () => void;
  onCreate: (request: GitBootstrapRequest, name: string) => Promise<unknown>;
}) {
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
        <form
          className="git-project-form"
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
              void onCreate(request, name);
            } catch (reason) {
              setLocalError(
                reason instanceof Error ? reason.message : String(reason),
              );
            }
          }}
        >
          <p>On {serverName}. The repository will be saved in Projects.</p>
          <label>
            Project name
            <Input
              autoFocus
              required
              value={name}
              onChange={(event) => setName(event.target.value)}
              disabled={busy}
            />
          </label>
          {mode === "clone" && (
            <label>
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
              <small>
                SSH uses the server’s SSH agent. HTTPS supports public
                repositories.
              </small>
            </label>
          )}
          <label>
            {mode === "init"
              ? "Existing directory on server"
              : "Destination on server"}
            <div className="git-path-field">
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
            </div>
            <small>
              {mode === "init"
                ? "Choose an existing directory without a Git repository. Its files will be kept."
                : "The parent directory must exist. The destination must not exist yet."}
            </small>
          </label>
          <label>
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
          </label>
          {blockedReason && <p role="status">{blockedReason}</p>}
          {(localError || error) && <p role="alert">{localError || error}</p>}
          {busy && (
            <p role="status">
              {mode === "clone"
                ? "Cloning repository… This may take a few minutes."
                : "Creating repository…"}
            </p>
          )}
          <footer>
            <Button disabled={busy} onClick={onClose}>
              Cancel
            </Button>
            <Button
              type="submit"
              loading={busy}
              disabled={busy || !!blockedReason}
            >
              {title}
            </Button>
          </footer>
        </form>
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
