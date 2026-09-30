import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { gitErrorMessage } from "../git/errors";

/** Load only the selected configuration; list names to recover a missing selection. */
export function useGitRemoteSelection(
  repoId: string,
  value: string,
  onChange: (name: string) => void,
  enabled = true,
) {
  const scope = useCurrentServerScope();
  const selected = value || "origin";
  const detail = useQuery({
    ...gitQueries.remote(scope, repoId, selected),
    enabled,
    refetchOnMount: "always",
    staleTime: 0,
  });
  const missing =
    detail.error &&
    typeof detail.error === "object" &&
    "code" in detail.error &&
    detail.error.code === "REMOTE_NOT_FOUND";
  const names = useQuery({
    ...gitQueries.remoteNames(scope, { repoId, filter: "", pageSize: 20 }),
    enabled: enabled && !!missing,
    refetchOnMount: "always",
    staleTime: 0,
  });
  const fallback =
    names.data?.entries.find(
      (entry) => entry.name === "origin" && selected !== "origin",
    )?.name ?? names.data?.entries[0]?.name;
  useEffect(() => {
    if (
      enabled &&
      missing &&
      !names.isFetching &&
      !names.isError &&
      fallback &&
      fallback !== selected
    )
      onChange(fallback);
  }, [
    enabled,
    missing,
    names.isFetching,
    names.isError,
    fallback,
    selected,
    onChange,
  ]);
  const empty =
    !!missing &&
    !names.isFetching &&
    !names.isError &&
    names.data?.metadata.totalEntries === 0;
  const loading =
    enabled &&
    (detail.isPending ||
      detail.isFetching ||
      (!!missing &&
        (names.isPending ||
          names.isFetching ||
          (!!fallback && fallback !== selected))));
  const error =
    !enabled || loading
      ? ""
      : missing && names.error
        ? gitErrorMessage(names.error)
        : detail.error && !empty
          ? gitErrorMessage(detail.error)
          : "";
  return {
    selected,
    empty,
    loading,
    error,
    remote: enabled && !loading && !error && !empty ? detail.data : undefined,
    refresh: () => {
      if (missing) void names.refetch();
      void detail.refetch();
    },
  };
}
