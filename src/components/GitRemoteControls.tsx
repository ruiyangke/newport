import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { MoreHorizontal } from "lucide-react";
import { gitPath, type GitWriteAction } from "../domain/git";
import type { GitRemotes, GitRepository } from "../domain/gitResponses";
import { gitKeys } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input } from "./controls";
import { Modal } from "./Editors";
import { GitRemoteRefs } from "./GitRemoteRefs";
import { GitRemotePicker } from "./GitRemotePicker";
import { useGitRemoteSelection } from "../hooks/useGitRemoteSelection";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "./ui/dropdown-menu";

type Remote = GitRemotes["entries"][number];
type Editor =
  | { kind: "add" }
  | { kind: "rename" | "url" | "remove" | "refs"; remote: Remote };
export function GitRemoteControls({
  repository,
  projectName,
  snapshot,
  busy,
  blockedReason,
  recoveryAvailable,
  open: controlledOpen,
  onOpenChange,
  hideTrigger = false,
  error,
  onAction,
}: {
  /** Still passed by the page; reads go through the session's registry. */
  repository: GitRepository;
  projectName: string;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
  recoveryAvailable: boolean;
  /** Controlled disclosure, so a shared actions menu can open this dialog. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  hideTrigger?: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  const [selfOpen, setSelfOpen] = useState(false);
  const open = controlledOpen ?? selfOpen;
  const setOpen = onOpenChange ?? setSelfOpen;
  // The chosen remote outlives the dialog, so reopening it returns to the
  // remote the user was looking at, while that remote still exists.
  const [selected, setSelected] = useState("");
  return (
    <>
      {!hideTrigger && (
        <Button
          disabled={busy}
          onClick={() => {
            setOpen(true);
          }}
        >
          Remotes
        </Button>
      )}
      {/* Mounted per disclosure, whether opened by the trigger or the actions
          menu: every opening starts from the list and reads it afresh. */}
      {open && (
        <RemotesDialog
          repository={repository}
          projectName={projectName}
          snapshot={snapshot}
          busy={busy}
          blockedReason={blockedReason}
          recoveryAvailable={recoveryAvailable}
          error={error}
          onAction={onAction}
          selected={selected}
          onSelect={setSelected}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

function RemotesDialog({
  repository,
  projectName,
  snapshot,
  busy,
  blockedReason,
  recoveryAvailable,
  error,
  onAction,
  selected: storedSelection,
  onSelect: setSelected,
  onClose,
}: {
  repository: GitRepository;
  projectName: string;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
  recoveryAvailable: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
  selected: string;
  onSelect: (name: string) => void;
  onClose: () => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const repoId = repository.repoId;
  const current = repository.head.name?.display.replace(/^refs\/heads\//, "");
  const [branch, setBranch] = useState(current ?? "");
  const [editor, setEditor] = useState<Editor | null>(null);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const selection = useGitRemoteSelection(repoId, storedSelection, setSelected);
  const { selected, remote, loading, error: readError } = selection;
  const seenSnapshot = useRef(snapshot);
  useEffect(() => {
    if (seenSnapshot.current === snapshot) return;
    seenSnapshot.current = snapshot;
    void queryClient.invalidateQueries({
      queryKey: [...gitKeys.repo(scope, repoId), "remote-names"],
    });
    void queryClient.invalidateQueries({
      queryKey: gitKeys.remote(scope, repoId, selected),
    });
  }, [queryClient, scope, repoId, snapshot, selected]);
  const disabled = busy || loading || !!blockedReason;
  const attached =
    !!current &&
    !repository.head.detached &&
    !repository.head.unborn &&
    !!repository.head.oid &&
    !!repository.head.name &&
    gitPath(repository.head.name.display).bytesB64 ===
      repository.head.name.bytesB64;
  async function submit(action: GitWriteAction) {
    if (disabled) return;
    if (await onAction(action)) {
      onClose();
      setEditor(null);
    }
  }
  return (
    <Modal
      title={
        editor?.kind === "add"
          ? "Add remote"
          : editor?.kind === "rename"
            ? "Rename remote"
            : editor?.kind === "url"
              ? "Change remote URL"
              : editor?.kind === "remove"
                ? "Remove remote"
                : editor?.kind === "refs"
                  ? "Remote branches and tags"
                  : "Remotes"
      }
      busy={busy}
      onClose={onClose}
      className="git-remote-dialog"
    >
      <div className="git-remote-body grid gap-[12px] px-[24px] pt-0 pb-[24px] [&_p]:text-[12px] [&_p]:wrap-anywhere [&_p]:text-muted-foreground">
        <p className="git-remote-context m-0!">
          {projectName}
          {current ? ` · ${current}` : " · Detached HEAD"}
        </p>
        {error && <p role="alert">{error}</p>}
        {blockedReason && (
          <div>
            <p>{blockedReason}</p>
            {recoveryAvailable && (
              <Button onClick={onClose}>View saved outcomes</Button>
            )}
          </div>
        )}
        {editor?.kind === "refs" ? (
          <GitRemoteRefs
            repoId={repository.repoId}
            remote={editor.remote}
            snapshot={snapshot}
            disabled={disabled}
            busy={busy}
            source={
              attached && current && repository.head.oid
                ? { name: current, oid: repository.head.oid.hex }
                : undefined
            }
            onAction={onAction}
            onBack={() => setEditor(null)}
          />
        ) : editor ? (
          <form
            className="git-project-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (disabled) return;
              if (editor.kind === "add" && name.trim() && url.trim())
                void submit({
                  kind: "remote.add",
                  name: name.trim(),
                  url: url.trim(),
                });
              else if (editor.kind === "rename" && name.trim())
                void submit({
                  kind: "remote.rename",
                  name: editor.remote.name,
                  newName: name.trim(),
                  expectedToken: editor.remote.token,
                });
              else if (editor.kind === "url" && url.trim())
                void submit({
                  kind: "remote.set_url",
                  name: editor.remote.name,
                  url: url.trim(),
                  expectedToken: editor.remote.token,
                });
              else if (editor.kind === "remove")
                void submit({
                  kind: "remote.remove",
                  name: editor.remote.name,
                  expectedToken: editor.remote.token,
                });
            }}
          >
            {editor.kind === "remove" ? (
              <p>
                Remove <strong>{editor.remote.name}</strong> from this
                repository’s configuration? Its remote-tracking references will
                also be removed. The remote repository itself is kept.
              </p>
            ) : (
              <>
                {(editor.kind === "add" || editor.kind === "rename") && (
                  <label>
                    Remote name
                    <Input
                      autoFocus
                      value={name}
                      onChange={(event) => setName(event.target.value)}
                      required
                      disabled={busy}
                    />
                  </label>
                )}
                {(editor.kind === "add" || editor.kind === "url") && (
                  <label>
                    Remote URL
                    <Input
                      value={url}
                      onChange={(event) => setUrl(event.target.value)}
                      placeholder="git@host:team/repository.git"
                      required
                      disabled={busy}
                    />
                  </label>
                )}
              </>
            )}
            <footer>
              <Button disabled={busy} onClick={() => setEditor(null)}>
                Back
              </Button>
              <Button type="submit" disabled={disabled}>
                {editor.kind === "remove" ? "Remove remote" : "Save remote"}
              </Button>
            </footer>
          </form>
        ) : (
          <>
            {loading && <p role="status">Loading remotes…</p>}
            {readError && (
              <p role="alert">
                {readError}{" "}
                <Button disabled={busy || loading} onClick={selection.refresh}>
                  Retry remotes
                </Button>
              </p>
            )}
            {!loading && !readError && (
              <>
                {selection.empty ? (
                  <p>
                    No remotes configured. Add one to fetch and publish
                    branches.
                  </p>
                ) : (
                  <>
                    <label className="git-remote-selection grid gap-[6px] text-[12px]">
                      Remote
                      <GitRemotePicker
                        repoId={repoId}
                        value={selected}
                        onChange={setSelected}
                        disabled={busy}
                      />
                    </label>
                    {remote && (
                      <>
                        <div className="git-remote-address flex items-center gap-[12px]">
                          <div className="min-w-0 flex-1">
                            <p>{remote.url ?? "Fetch URL unavailable"}</p>
                            {remote.pushUrl && (
                              <p>Push URL: {remote.pushUrl}</p>
                            )}
                          </div>
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <Button
                                size="icon"
                                variant="ghost"
                                aria-label={`Actions for remote ${remote.name}`}
                                disabled={busy || loading}
                              >
                                <MoreHorizontal size={16} />
                              </Button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="end">
                              <DropdownMenuItem
                                onSelect={() =>
                                  setEditor({ kind: "refs", remote })
                                }
                              >
                                Remote branches and tags…
                              </DropdownMenuItem>
                              <DropdownMenuItem
                                disabled={disabled}
                                onSelect={() => {
                                  setName(remote.name);
                                  setEditor({ kind: "rename", remote });
                                }}
                              >
                                Rename…
                              </DropdownMenuItem>
                              <DropdownMenuItem
                                disabled={disabled}
                                onSelect={() => {
                                  setUrl("");
                                  setEditor({ kind: "url", remote });
                                }}
                              >
                                Change URL…
                              </DropdownMenuItem>
                              <DropdownMenuItem
                                disabled={disabled}
                                onSelect={() =>
                                  setEditor({ kind: "remove", remote })
                                }
                              >
                                Remove…
                              </DropdownMenuItem>
                            </DropdownMenuContent>
                          </DropdownMenu>
                        </div>
                        <div className="git-remote-fetch flex items-center gap-[12px]">
                          <p className="min-w-0 flex-1">
                            Update remote-tracking branches without changing
                            your working files.
                          </p>
                          <Button
                            disabled={disabled}
                            onClick={() =>
                              void submit({
                                kind: "fetch",
                                remote: remote.name,
                                expectedToken: remote.token,
                                prune: false,
                              })
                            }
                          >
                            Fetch {remote.name}
                          </Button>
                        </div>
                        <div className="git-remote-transfer grid gap-[10px] border-y border-border px-0 py-[16px]">
                          <label className="grid gap-[6px] text-[12px]">
                            Remote branch
                            <Input
                              value={branch}
                              onChange={(event) =>
                                setBranch(event.target.value)
                              }
                              disabled={busy || !attached}
                            />
                          </label>
                          {attached ? (
                            <p>
                              Pull {remote.name}/{branch || "…"} into {current},
                              or push {current} to {remote.name}/{branch || "…"}
                              .
                            </p>
                          ) : (
                            <p>
                              Switch to a local branch with a commit before
                              pulling or pushing.
                            </p>
                          )}
                          <div className="git-remote-actions flex flex-wrap gap-[8px]">
                            <Button
                              disabled={disabled || !attached || !branch.trim()}
                              onClick={() =>
                                void submit({
                                  kind: "pull.fast_forward",
                                  remote: remote.name,
                                  expectedToken: remote.token,
                                  remoteBranch: branch.trim(),
                                })
                              }
                            >
                              Pull fast-forward
                            </Button>
                            <Button
                              disabled={disabled || !attached || !branch.trim()}
                              onClick={() => {
                                if (current && repository.head.oid)
                                  void submit({
                                    kind: "push",
                                    remote: remote.name,
                                    expectedToken: remote.token,
                                    branch: current,
                                    expectedOid: repository.head.oid.hex,
                                    destinationBranch: branch.trim(),
                                  });
                              }}
                            >
                              Push branch
                            </Button>
                          </div>
                          <p>
                            Pull stops if branches have diverged. Push does not
                            overwrite remote history.
                          </p>
                        </div>
                      </>
                    )}
                  </>
                )}
                <div className="git-remote-footer flex items-center gap-[12px]">
                  <Button
                    disabled={disabled}
                    onClick={() => {
                      setName("");
                      setUrl("");
                      setEditor({ kind: "add" });
                    }}
                  >
                    Add remote
                  </Button>
                  <p className="flex-1">
                    SSH uses the server’s SSH agent. HTTPS uses the repository’s
                    configured credentials on the server.
                  </p>
                </div>
              </>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
