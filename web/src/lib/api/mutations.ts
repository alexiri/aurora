"use client";

import { type QueryKey, useMutation, useQueryClient } from "@tanstack/react-query";

/**
 * A mutation that, once it succeeds, invalidates `keys` and waits for their refetch (so the caller's
 * `await mutateAsync()` resolves with fresh data on screen). Anything beyond that (an `onSettled`, a cache
 * write, invalidating from the response) stays a hand-written `useMutation`.
 */
export function useInvalidatingMutation<V = void, R = unknown>(mutationFn: (vars: V) => Promise<R>, ...keys: QueryKey[]) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: () => Promise.all(keys.map((queryKey) => qc.invalidateQueries({ queryKey }))),
  });
}
