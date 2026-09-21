import { queryOptions } from "@tanstack/react-query";
import { desktop } from "../api/desktop";
import { keys, type ServerScope } from "./keys";
import { readIPC } from "./client";

export function fileListingOptions(scope: ServerScope, path: string) {
  return queryOptions({
    queryKey: keys.files(scope, path),
    queryFn: async ({ signal }) => {
      const operation = crypto.randomUUID();
      const cancel = () => {
        void desktop("files_cancel", { operation }).catch(() => {});
      };
      signal.addEventListener("abort", cancel, { once: true });
      try {
        return await readIPC(signal, () =>
          desktop("files_list", { id: scope.id, operation, path }),
        );
      } finally {
        signal.removeEventListener("abort", cancel);
      }
    },
    staleTime: 0,
  });
}
