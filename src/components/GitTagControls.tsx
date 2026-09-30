import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { gitPath, type GitWriteAction } from "../domain/git";
import {
  appendGitPage,
  type GitRepository,
  type GitTags,
} from "../domain/gitResponses";
import { refreshQuery } from "../query/client";
import { gitKeys, gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input } from "./controls";
import { Modal } from "./Editors";
import { Pagination, PaginationContent, PaginationItem } from "./ui/pagination";
import { Textarea } from "./ui/textarea";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

/** Helper copy in the tags dialog: small and muted, as the design has it. */
const note = "text-[12px] text-muted-foreground";

type Tag = GitTags["entries"][number];
type Editor = { kind: "create" } | { kind: "delete" | "push"; tag: Tag };
/**
 * Where the reader is in the paged tag list. Each page is its own query, keyed
 * by its cursor; this only remembers which cursors were visited. It belongs to
 * one reading of the first page (`basis`, that read's time), so a fresh first
 * page -- the dialog reopening, the repository changing, a reload -- starts
 * the list over, as it always has.
 */
type Trail = {
  basis: number;
  cursors: string[];
  index: number;
  pending: boolean;
  error: string;
  selected: string | null;
};
const startTrail = (basis: number): Trail => ({
  basis,
  cursors: [],
  index: 0,
  pending: false,
  error: "",
  selected: null,
});
function usable(tag: Tag) {
  try {
    return (
      !!tag.oid &&
      !tag.symbolicTarget &&
      gitPath(tag.name.display).bytesB64 === tag.name.bytesB64
    );
  } catch {
    return false;
  }
}
export function GitTagControls({
  repository,
  snapshot,
  busy,
  blockedReason,
  open: controlledOpen,
  onOpenChange,
  hideTrigger = false,
  error,
  onAction,
}: {
  /** Still passed by the page; reads go through the session's registry. */
  repository: GitRepository;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
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
  return (
    <>
      {!hideTrigger && (
        <Button
          disabled={busy}
          onClick={() => {
            setOpen(true);
          }}
        >
          Tags
        </Button>
      )}
      {/* Mounted per disclosure, whether opened by the trigger or the actions
          menu: every opening starts from the list and reads it afresh. */}
      {open && (
        <TagsDialog
          repository={repository}
          snapshot={snapshot}
          busy={busy}
          blockedReason={blockedReason}
          error={error}
          onAction={onAction}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

function TagsDialog({
  repository,
  snapshot,
  busy,
  blockedReason,
  error,
  onAction,
  onClose,
}: {
  repository: GitRepository;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
  onClose: () => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const repoId = repository.repoId;
  const [editor, setEditor] = useState<Editor | null>(null);
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [annotation, setAnnotation] = useState("");
  const [annotated, setAnnotated] = useState(false);
  const [remoteName, setRemoteName] = useState("");
  const first = useQuery({
    ...gitQueries.tags(scope, repoId),
    refetchOnMount: "always",
  });
  // The page re-reads status after anything that may have moved a tag; a new
  // snapshot re-reads the list, which also starts it over at the first page.
  const seenSnapshot = useRef(snapshot);
  useEffect(() => {
    if (seenSnapshot.current === snapshot) return;
    seenSnapshot.current = snapshot;
    void queryClient.invalidateQueries({
      queryKey: gitKeys.tags(scope, repoId),
    });
  }, [queryClient, scope, repoId, snapshot]);
  const [storedTrail, setTrail] = useState(() => startTrail(0));
  const trail =
    storedTrail.basis === first.dataUpdatedAt
      ? storedTrail
      : startTrail(first.dataUpdatedAt);
  const cursor = trail.index === 0 ? undefined : trail.cursors[trail.index - 1];
  // Later pages are only ever read by "Next tags", which checks that each one
  // continues the listing; showing one must not re-read it on its own.
  const later = useQuery({
    ...gitQueries.tags(scope, repoId, cursor),
    enabled: false,
  });
  // A listing being (re)read is not shown, as before: nothing is offered from
  // it until the read that replaces it has landed.
  const reading = first.isFetching || first.isError;
  const page = reading
    ? undefined
    : trail.index === 0
      ? first.data
      : later.data;
  const loading = first.isFetching || trail.pending;
  const pushing = editor?.kind === "push";
  // Read afresh each time the push form opens, as before; never otherwise.
  const remotesQuery = useQuery({
    ...gitQueries.remotes(scope, repoId),
    enabled: pushing,
    staleTime: 0,
  });
  const remotes =
    pushing && !remotesQuery.isFetching && !remotesQuery.isError
      ? (remotesQuery.data ?? null)
      : null;
  const readError =
    trail.error ||
    (!first.isFetching && first.error ? String(first.error) : "") ||
    (pushing && !remotesQuery.isFetching && remotesQuery.error
      ? String(remotesQuery.error)
      : "");
  const selected = trail.selected;
  const setSelected = (value: string | null) =>
    setTrail({ ...trail, selected: value });
  const selectedTag = page?.entries.find(
    (tag) => tag.name.bytesB64 === selected,
  );
  // The remote the form offers: the one chosen, while it still exists, else
  // origin, else the first -- what each fresh read used to select.
  const chosenRemote = remotes?.entries.some(
    (remote) => remote.name === remoteName,
  )
    ? remoteName
    : (remotes?.entries.find((remote) => remote.name === "origin")?.name ??
      remotes?.entries[0]?.name ??
      "");
  const remote = remotes?.entries.find(
    (remote) => remote.name === chosenRemote,
  );
  const disabled = busy || loading || !!blockedReason || !snapshot;
  async function submit(action: GitWriteAction) {
    if (disabled) return;
    if (await onAction(action)) {
      setEditor(null);
      onClose();
    }
  }
  function reload() {
    setTrail(startTrail(first.dataUpdatedAt));
    void first.refetch();
    if (pushing) void remotesQuery.refetch();
  }
  async function nextPage() {
    if (!page?.nextCursor || loading || busy) return;
    const visited = trail.cursors[trail.index];
    if (
      visited !== undefined &&
      queryClient.getQueryData(gitKeys.tags(scope, repoId, visited))
    ) {
      setTrail({ ...trail, index: trail.index + 1, selected: null });
      return;
    }
    const nextCursor = page.nextCursor;
    const requested: Trail = { ...trail, pending: true, error: "" };
    setTrail(requested);
    let outcome: Partial<Trail>;
    try {
      const next = await refreshQuery(
        queryClient,
        gitQueries.tags(scope, repoId, nextCursor),
      );
      appendGitPage(page, next, nextCursor);
      outcome = {
        cursors: [...trail.cursors.slice(0, trail.index), nextCursor],
        index: trail.index + 1,
        selected: null,
      };
    } catch (reason) {
      outcome = { error: String(reason) };
    }
    // A page that lands after the list started over belongs to a listing that
    // is no longer shown. It stays cached under its own cursor, unused.
    setTrail((current) =>
      current === requested
        ? { ...current, ...outcome, pending: false }
        : current,
    );
  }
  return (
    <Modal
      title={
        editor?.kind === "create"
          ? "Create tag"
          : editor?.kind === "delete"
            ? "Delete local tag"
            : editor?.kind === "push"
              ? "Push tag"
              : "Tags"
      }
      busy={busy}
      onClose={onClose}
      className="git-tag-dialog w-[min(640px,calc(100vw-32px))]!"
    >
      <div className="git-tag-body min-h-0 overflow-y-auto px-[24px] pt-[16px] pb-[24px] [overflow-wrap:anywhere]">
        {blockedReason && (
          <p role="status" className={note}>
            {blockedReason}
          </p>
        )}
        {(readError || error) && (
          <div role="alert">
            <p className={note}>{readError || error}</p>
            {readError && (
              <Button disabled={busy || loading} onClick={reload}>
                Reload tags
              </Button>
            )}
          </div>
        )}
        {editor ? (
          <form
            className="git-project-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (disabled) return;
              if (
                editor.kind === "create" &&
                name.trim() &&
                /^[0-9a-f]{40}$/i.test(target.trim()) &&
                (!annotated || annotation.trim())
              )
                void submit({
                  kind: "tag.create",
                  name: name.trim(),
                  targetOid: target.trim().toLowerCase(),
                  ...(annotated ? { annotation: { message: annotation } } : {}),
                });
              else if (editor.kind === "delete" && usable(editor.tag))
                void submit({
                  kind: "tag.delete",
                  name: editor.tag.name.display,
                  expectedOid: editor.tag.oid!.hex,
                });
              else if (editor.kind === "push" && remote && usable(editor.tag))
                void submit({
                  kind: "tag.push",
                  name: editor.tag.name.display,
                  expectedOid: editor.tag.oid!.hex,
                  remote: remote.name,
                  expectedToken: remote.token,
                });
            }}
          >
            {editor.kind === "create" ? (
              <>
                <label>
                  Tag name
                  <Input
                    autoFocus
                    required
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    disabled={busy}
                    placeholder="v1.0.0"
                  />
                </label>
                <label>
                  Target object ID
                  <Input
                    aria-label="Target object ID"
                    aria-describedby="git-tag-target-hint"
                    required
                    value={target}
                    onChange={(event) => setTarget(event.target.value)}
                    disabled={busy}
                    pattern="[0-9a-fA-F]{40}"
                    spellCheck={false}
                  />
                  <small id="git-tag-target-hint">
                    Defaults to the current commit. Enter a full 40-character
                    Git object ID.
                  </small>
                </label>
                <label>
                  Tag type
                  <Select
                    value={annotated ? "annotated" : "lightweight"}
                    onValueChange={(value) =>
                      setAnnotated(value === "annotated")
                    }
                    disabled={busy}
                  >
                    <SelectTrigger aria-label="Tag type">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="lightweight">Lightweight</SelectItem>
                      <SelectItem value="annotated">Annotated</SelectItem>
                    </SelectContent>
                  </Select>
                </label>
                {annotated && (
                  <label>
                    Tag message
                    <Textarea
                      aria-label="Tag message"
                      aria-describedby="git-tag-message-hint"
                      required
                      value={annotation}
                      onChange={(event) => setAnnotation(event.target.value)}
                      disabled={busy}
                    />
                    <small id="git-tag-message-hint">
                      Uses the Git identity configured on the server.
                    </small>
                  </label>
                )}
                <p className={note}>
                  Creates a local tag. Push it separately to share it with a
                  remote.
                </p>
              </>
            ) : (
              <>
                <p className={note}>
                  <strong>{editor.tag.name.display}</strong> ·{" "}
                  {editor.tag.oid?.hex.slice(0, 12)}
                </p>
                {editor.kind === "delete" ? (
                  <p className={note}>
                    Remove this tag from the server repository? Commits and tags
                    on other remotes will remain.
                  </p>
                ) : (
                  <>
                    <p className={note}>
                      Push this exact tag to the selected remote. An existing
                      remote tag will not be overwritten.
                    </p>
                    {!remotes && !readError && (
                      <p role="status" className={note}>
                        Loading remotes…
                      </p>
                    )}
                    {remotes && remotes.entries.length === 0 && (
                      <p className={note}>
                        No remotes configured. Add one in Remotes first.
                      </p>
                    )}
                    {!!remotes?.entries.length && (
                      <label>
                        Remote
                        <Select
                          value={chosenRemote}
                          onValueChange={setRemoteName}
                          disabled={busy}
                        >
                          <SelectTrigger aria-label="Remote">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {remotes.entries.map((item) => (
                              <SelectItem key={item.name} value={item.name}>
                                {item.name}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </label>
                    )}
                    {remote && (
                      <p className={`git-tag-address ${note}`}>
                        {remote.pushUrl ??
                          remote.url ??
                          "Remote URL unavailable"}
                      </p>
                    )}
                  </>
                )}
              </>
            )}
            <footer>
              <Button disabled={busy} onClick={() => setEditor(null)}>
                Back
              </Button>
              <Button
                type="submit"
                loading={busy}
                variant={editor.kind === "delete" ? "destructive" : undefined}
                disabled={
                  disabled ||
                  (editor.kind === "push" && !remote) ||
                  (editor.kind === "create" &&
                    (!name.trim() ||
                      !/^[0-9a-f]{40}$/i.test(target.trim()) ||
                      (annotated && !annotation.trim())))
                }
              >
                {editor.kind === "create"
                  ? "Create tag"
                  : editor.kind === "delete"
                    ? "Delete local tag"
                    : "Push tag"}
              </Button>
            </footer>
          </form>
        ) : (
          <>
            <div className="git-tag-toolbar mx-0 my-[12px] flex items-center gap-[8px]">
              <p className={`flex-1 ${note}`}>
                {repository.head.unborn
                  ? "No commits yet. Create a commit before tagging it."
                  : "Tags mark specific points in your repository."}
              </p>
              <Button
                disabled={disabled}
                onClick={() => {
                  setName("");
                  setTarget(repository.head.oid?.hex ?? "");
                  setAnnotation("");
                  setAnnotated(false);
                  setEditor({ kind: "create" });
                }}
              >
                New tag
              </Button>
            </div>
            {loading && (
              <p role="status" className={note}>
                Loading tags…
              </p>
            )}
            {page?.entries.length === 0 && (
              <p className={note}>No tags in this repository.</p>
            )}
            <ul className="git-tag-list max-h-[240px] overflow-y-auto">
              {page?.entries.map((tag) => (
                <li key={tag.name.bytesB64} className="border-b border-border">
                  <Button
                    variant="ghost"
                    className="h-auto! w-full flex-col items-start px-[8px]! py-[10px] text-left whitespace-normal aria-pressed:bg-accent aria-pressed:text-accent-foreground dark:aria-pressed:bg-[color-mix(in_srgb,var(--foreground)_10%,var(--accent))]"
                    aria-pressed={selected === tag.name.bytesB64}
                    onClick={() => setSelected(tag.name.bytesB64)}
                  >
                    <strong>{tag.name.display}</strong>
                    <small className="text-[11px]! text-muted-foreground!">
                      {tag.annotated ? "Annotated" : "Lightweight"} ·{" "}
                      {tag.oid?.hex.slice(0, 8) ?? "Object unavailable"}
                    </small>
                  </Button>
                </li>
              ))}
            </ul>
            {selectedTag && (
              <div className="git-tag-details pt-[16px]">
                <strong>{selectedTag.name.display}</strong>
                <p className={`mt-[6px]! ${note}`}>
                  Object: {selectedTag.oid?.hex ?? "Unavailable"}
                </p>
                {selectedTag.peeledOid && (
                  <p className={`mt-[6px]! ${note}`}>
                    Target: {selectedTag.peeledOid.hex} (
                    {selectedTag.peeledType})
                  </p>
                )}
                {selectedTag.tagger && (
                  <p className={`mt-[6px]! ${note}`}>
                    {selectedTag.tagger.name} &lt;{selectedTag.tagger.email}
                    &gt;
                  </p>
                )}
                {selectedTag.message && (
                  <pre className="mt-[12px] max-h-[160px] overflow-y-auto [font:inherit] whitespace-pre-wrap">
                    {selectedTag.message.display}
                  </pre>
                )}
                {selectedTag.messageTruncated && (
                  <p className={`mt-[6px]! ${note}`}>
                    The annotation is truncated.
                  </p>
                )}
                {selectedTag.detailsOmitted && (
                  <p className={`mt-[6px]! ${note}`}>
                    Some tag details could not be loaded.
                  </p>
                )}
                {!usable(selectedTag) && (
                  <p className={`mt-[6px]! ${note}`}>
                    This tag cannot be changed because its name or direct object
                    ID is unavailable.
                  </p>
                )}
                <div className="git-tag-actions mx-0 my-[12px] flex items-center gap-[8px]">
                  <Button
                    disabled={disabled || !usable(selectedTag)}
                    onClick={() => {
                      setRemoteName("");
                      setEditor({ kind: "push", tag: selectedTag });
                    }}
                  >
                    Push…
                  </Button>
                  <Button
                    disabled={disabled || !usable(selectedTag)}
                    onClick={() =>
                      setEditor({ kind: "delete", tag: selectedTag })
                    }
                  >
                    Delete local tag…
                  </Button>
                </div>
              </div>
            )}
            {page && (trail.index > 0 || page.nextCursor) && (
              <Pagination
                aria-label="Tag pages"
                className="git-tag-pagination mx-0 my-[12px] flex items-center justify-between gap-[8px]"
              >
                <PaginationContent className="w-full justify-between gap-[8px]">
                  <PaginationItem>
                    <Button
                      disabled={busy || loading || trail.index === 0}
                      onClick={() =>
                        setTrail({
                          ...trail,
                          index: trail.index - 1,
                          selected: null,
                        })
                      }
                    >
                      Previous tags
                    </Button>
                  </PaginationItem>
                  <PaginationItem>
                    <span aria-current="page">Page {trail.index + 1}</span>
                  </PaginationItem>
                  <PaginationItem>
                    <Button
                      disabled={busy || loading || !page.nextCursor}
                      onClick={() => void nextPage()}
                    >
                      Next tags
                    </Button>
                  </PaginationItem>
                </PaginationContent>
              </Pagination>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
