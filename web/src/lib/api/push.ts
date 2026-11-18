"use client";

// Web Push (phase 5, P4-011B-4, D-134, D-266, C-830, C-831): the instance's push config and the caller's own
// devices. Session-only routes; `404 push_unavailable` while the operator has not enabled push.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, api } from "./client";
import { retryUnlessClientError } from "./errors";
import { useInvalidatingMutation } from "./mutations";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type PushConfig = S["PushConfig"];
export type PushSubscriptionRow = S["PushSubscription"];
export type PushState = S["PushState"];

export const pushKeys = {
  config: ["push", "config"] as const,
  list: ["me", "push-subscriptions"] as const,
};

/** `GET /v1/push/config`; a 404 (`push_unavailable`) reads as "not available". */
export async function fetchPushConfig(): Promise<PushConfig> {
  try {
    return await unwrap(api.GET("/v1/push/config"));
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return { available: false, allowed_hosts: [], vapid_public_key: null };
    throw e;
  }
}

export function usePushConfig() {
  return useQuery({ queryKey: pushKeys.config, queryFn: fetchPushConfig, retry: retryUnlessClientError, staleTime: 60_000 });
}

export async function fetchPushSubscriptions(): Promise<PushSubscriptionRow[]> {
  return (await unwrap(api.GET("/v1/me/push-subscriptions"))).subscriptions;
}

export function usePushSubscriptions(enabled: boolean) {
  return useQuery({ queryKey: pushKeys.list, queryFn: fetchPushSubscriptions, retry: retryUnlessClientError, enabled });
}

export function createPushSubscription(body: S["CreatePushSubscription"]): Promise<PushSubscriptionRow> {
  return unwrap(api.POST("/v1/me/push-subscriptions", { body }));
}

export async function deletePushSubscription(id: string): Promise<void> {
  await unwrap(api.DELETE("/v1/me/push-subscriptions/{id}", { params: { path: { id } } }));
}

export function useUpdatePushSubscription() {
  return useInvalidatingMutation(({ id, ...body }: { id: string } & S["UpdatePushSubscription"]) =>
    unwrap(api.PATCH("/v1/me/push-subscriptions/{id}", { params: { path: { id } }, body })), pushKeys.list);
}

export function useTestPushSubscription() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.POST("/v1/me/push-subscriptions/{id}/test", { params: { path: { id } } })),
    // The outcome is visible in the row once the delivery ran.
    onSettled: () => qc.invalidateQueries({ queryKey: pushKeys.list }),
  });
}
