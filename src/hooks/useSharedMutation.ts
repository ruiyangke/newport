import {
  useMutation,
  useMutationState,
  useQueryClient,
  type MutationKey,
  type MutationState,
  type UseMutationOptions,
} from "@tanstack/react-query";

// Actions outlive the page that started them. Observe their cached state so a
// remounted page cannot launch a duplicate or lose the eventual error/result.
export function useSharedMutation<TData = unknown, TVariables = void>(
  options: UseMutationOptions<TData, Error, TVariables> & {
    mutationKey: MutationKey;
  },
) {
  const client = useQueryClient();
  const filters = { mutationKey: options.mutationKey, exact: true };
  const mutation = useMutation({ gcTime: 5 * 60_000, ...options });
  const states = useMutationState({
    filters,
    select: (entry) => entry.state as MutationState<TData, Error, TVariables>,
  });
  const latest = states.at(-1);
  const status = latest?.status ?? "idle";
  return {
    status,
    data: latest?.data,
    variables: latest?.variables,
    error: latest?.error ?? null,
    isPending: status === "pending",
    isSuccess: status === "success",
    mutate: (variables: TVariables) => {
      // Consult the cache synchronously; React may not have rendered pending yet.
      if (!client.isMutating(filters)) mutation.mutate(variables);
    },
    reset: () => {
      for (const entry of client.getMutationCache().findAll(filters)) {
        if (entry.state.status !== "pending")
          client.getMutationCache().remove(entry);
      }
      mutation.reset();
    },
  };
}
