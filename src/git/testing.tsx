import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { GitProjects } from "../api/gitProjects";
import type { GitRepositoryClient } from "../api/gitRepository";
import { createQueryClient } from "../query/client";
import { ServerScopeProvider, serverScope } from "../query/keys";
import type { Server } from "../types";
import { gitResources } from "./registry";
import { TooltipProvider } from "../components/ui/tooltip";

/*
 * Test support for components that read Git through Query.
 *
 * Those components no longer take their reads from a `client` prop: they ask
 * the registry for the session belonging to the enclosing server. A test that
 * already has a fake client seeds the registry with it, so the fake answers
 * exactly the calls it answered before and every assertion keeps its meaning.
 */
export const testGitServer = {
  id: "git-test-server",
  name: "Git test server",
  sshHost: "git.test",
  sshUser: "dev",
  sshPort: 22,
  authMethod: "agent",
  identityFile: "",
} as unknown as Server;

export function seedGitClient(
  client: Partial<GitRepositoryClient>,
  server: Server = testGitServer,
) {
  const scope = serverScope(server);
  gitResources.set(scope.connection, {
    repositories: client as GitRepositoryClient,
  } as unknown as GitProjects);
  return scope;
}

/** A fresh cache per test, so one test's reads are never another's answers. */
export function createTestQueryClient() {
  return createQueryClient();
}

export function GitTestProviders({
  queryClient,
  server = testGitServer,
  children,
}: {
  queryClient: QueryClient;
  server?: Server;
  children: ReactNode;
}) {
  return (
    <QueryClientProvider client={queryClient}>
      {/* As the app's root has it: icon buttons carry their name in a tooltip. */}
      <TooltipProvider>
        <ServerScopeProvider server={server}>{children}</ServerScopeProvider>
      </TooltipProvider>
    </QueryClientProvider>
  );
}
