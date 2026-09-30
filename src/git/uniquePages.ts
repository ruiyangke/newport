import {
  validateGitPageContinuation,
  type GitPage,
} from "../domain/gitResponses";

// Only locally validated immutable appends may skip the query cache's second
// full-array traversal. Weak keys and weak predecessors retain no page history.
const appendedFrom = new WeakMap<object, WeakSet<object>>();
export function isValidatedPageAppend(previous: unknown, incoming: unknown) {
  return (
    typeof previous === "object" &&
    previous !== null &&
    typeof incoming === "object" &&
    incoming !== null &&
    appendedFrom.get(incoming)?.has(previous) === true
  );
}

/** Entry keys must remain stable for one query. A replaced page rebuilds the
 * index, including refreshes and rollbacks to an earlier query-cache value. */
export function createUniquePageAppender<T, M>(entryKey: (entry: T) => string) {
  let indexed: GitPage<T, M> | null = null;
  let seen = new Set<string>();
  const append = (
    previous: GitPage<T, M>,
    incoming: GitPage<T, M>,
    cursor: string,
  ): GitPage<T, M> => {
    validateGitPageContinuation(previous, incoming, cursor);
    const reusable = indexed === previous;
    const ids = reusable ? seen : new Set<string>();
    const entries = reusable
      ? [...previous.entries]
      : previous.entries.filter((entry) => {
          const id = entryKey(entry);
          if (ids.has(id)) return false;
          ids.add(id);
          return true;
        });
    const added = new Set<string>();
    for (const entry of incoming.entries) {
      const id = entryKey(entry);
      if (ids.has(id) || added.has(id)) continue;
      added.add(id);
      entries.push(entry);
    }
    if (incoming.nextCursor && entries.length === previous.entries.length)
      throw new Error(
        "The next page did not add any results. Refresh the list.",
      );
    // Publish the index only after successful validation. A thrown key decoder
    // or failed append must not poison a retry with partially inserted keys.
    for (const id of added) ids.add(id);
    seen = ids;
    indexed = { ...incoming, entries };
    appendedFrom.set(indexed, new WeakSet([previous]));
    return indexed;
  };
  // Query caches may structurally share this result into another object. The
  // caller supplies the equal value returned by its cache publication.
  append.adopt = (source: GitPage<T, M>, stored: GitPage<T, M> | undefined) => {
    if (indexed === source && stored) indexed = stored;
  };
  append.reset = () => {
    indexed = null;
    seen = new Set<string>();
  };
  return append;
}
