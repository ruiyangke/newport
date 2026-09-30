import { GitProjects } from "../api/gitProjects";
import type { ServerScope } from "../query/keys";

/*
 * One Git workspace per server connection, outside React.
 *
 * The page and each panel that reads its own branches, stashes or tags share
 * one `GitProjects`, so their requests queue together on the server's one
 * connection (the native side keeps a single connection per server). Keyed by
 * connection here, as terminals are, it is found by anyone holding the server
 * scope, and it outlives navigation the way the rest of the workspace does.
 * Repository ids are stateless tokens, so none of this is needed for them to
 * stay valid -- it is about sharing, not ownership.
 *
 * Like terminals, these are native resources: they stay out of reactive state,
 * and are disposed only when their connection stops being valid or the user
 * deliberately lets go of what they hold.
 */
export const gitResources = new Map<string, GitProjects>();

export function gitProjectsFor(scope: ServerScope) {
  let projects = gitResources.get(scope.connection);
  if (!projects) {
    projects = new GitProjects(scope.id);
    gitResources.set(scope.connection, projects);
  }
  return projects;
}

/**
 * Disposes this connection's workspace and starts a fresh one: an explicit
 * reset after a connection failure. The server's connection is dropped and
 * the next request opens a new one.
 */
export function resetGitProjects(scope: ServerScope) {
  const previous = gitResources.get(scope.connection);
  gitResources.delete(scope.connection);
  void previous?.dispose().catch(() => undefined);
  return gitProjectsFor(scope);
}

export function retainGitProjects(valid: Set<string>) {
  for (const [key, projects] of gitResources) {
    if (!valid.has(key)) {
      void projects.dispose().catch(() => undefined);
      gitResources.delete(key);
    }
  }
}
if (import.meta.hot)
  import.meta.hot.dispose(() => retainGitProjects(new Set()));
