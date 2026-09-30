import type {
  GitCommitDiffPage,
  GitWorkingDiffPage,
  GitDiff,
} from "../domain/gitResponses";

/** Only complete lines enter the editor. A page boundary can split UTF-8 or
 * even a very long line; its tail remains pending until its last piece arrives. */
export function historicalDiff(page: GitCommitDiffPage): GitDiff {
  return createDiffRenderer()(page);
}
export function workingDiff(page: GitWorkingDiffPage): GitDiff {
  return createDiffRenderer()(page);
}
type DiffPage = GitCommitDiffPage | GitWorkingDiffPage;
type Piece = DiffPage["entries"][number]["hunks"][number]["lines"][number];
type Content =
  GitDiff["files"][number]["hunks"][number]["lines"][number]["content"];

/** One renderer per mounted preview. Immutable piece identities survive page
 * appends and React Query structural sharing. Weak keys release old selections;
 * prefix checks prevent a replaced fragment from reusing stale decoded text. */
export function createDiffRenderer(): (page: DiffPage) => GitDiff {
  let snapshot: string | undefined;
  let completeLines = new WeakMap<
    Piece,
    { pieces: Piece[]; content: Content }
  >();
  const decoder = new TextDecoder();
  function content(pieces: Piece[]): Content {
    const last = pieces[pieces.length - 1];
    const cached = completeLines.get(last);
    if (
      cached &&
      cached.pieces.length === pieces.length &&
      cached.pieces.every((piece, index) => piece === pieces[index])
    )
      return cached.content;
    const raw = pieces.map((piece) => atob(piece.contentBytesB64)).join("");
    const value = {
      bytesB64: pieces.length === 1 ? last.contentBytesB64 : btoa(raw),
      display: decoder.decode(Uint8Array.from(raw, (c) => c.charCodeAt(0))),
    };
    completeLines.set(last, { pieces, content: value });
    return value;
  }
  return (page) => {
    if (snapshot !== page.snapshot) {
      // Release decoded text for old selections even while the query cache
      // still retains their source pages.
      completeLines = new WeakMap();
      snapshot = page.snapshot;
    }
    const metadata = page.metadata;
    const working = "sourceSnapshot" in metadata;
    // A partial directory must not look like a one-file mutation target.
    const completeFiles =
      !working || page.entries.length === metadata.totalFiles;
    return {
      snapshot: working ? metadata.sourceSnapshot : page.snapshot,
      comparison: working ? null : metadata,
      readOnly: !working,
      truncated: false,
      files: page.entries.map((file) => ({
        ...file,
        hunks: file.hunks.map((hunk) => {
          const lines: GitDiff["files"][number]["hunks"][number]["lines"] = [];
          let pending: Piece[] = [];
          for (const piece of hunk.lines) {
            pending.push(piece);
            if (!piece.lineComplete) continue;
            lines.push({
              id: working ? piece.id : null,
              origin: piece.origin,
              oldLine: piece.oldLine,
              newLine: piece.newLine,
              content: content(pending),
            });
            pending = [];
          }
          const complete =
            completeFiles &&
            hunk.totalLines === lines.length &&
            pending.length === 0;
          if (!complete) for (const line of lines) line.id = null;
          return { ...hunk, id: working && complete ? hunk.id : null, lines };
        }),
      })),
    };
  };
}
