"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type Webhook = S["Webhook"];
export type WebhookDelivery = S["WebhookDelivery"];
export type CreateWebhookRequest = S["CreateWebhookRequest"];
export type UpdateWebhookRequest = S["UpdateWebhookRequest"];

export const webhookKeys = {
  list: (agent: string) => ["agents", agent, "webhooks"] as const,
  deliveries: (agent: string, id: string) => ["agents", agent, "webhooks", id, "deliveries"] as const,
};

export function useWebhooks(agent: string) {
  return useQuery({
    queryKey: webhookKeys.list(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/webhooks", { params: { path: { agent } } })),
    retry: false,
  });
}

export function useWebhookDeliveries(agent: string, id: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: webhookKeys.deliveries(agent, id),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/webhooks/{id}/deliveries", {
          params: { path: { id }, query: { cursor: pageParam } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled,
    retry: false,
  });
}

// The create and rotate responses carry the one-time URL. These mutations deliberately return it to the
// caller only: the list is refreshed with the token-free `Webhook`, and the secret never enters the query cache
// (mutation results are dropped as soon as the mutation is reset/unmounted: see `gcTime: 0`).
export function useCreateWebhook(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (body: CreateWebhookRequest) =>
      unwrap(api.POST("/v1/agents/{agent}/webhooks", { params: { path: { agent } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: webhookKeys.list(agent) }),
  });
}

export function useRotateWebhook(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (id: string) => unwrap(api.POST("/v1/webhooks/{id}/rotate", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: webhookKeys.list(agent) }),
  });
}

export function useUpdateWebhook(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateWebhookRequest }) =>
      unwrap(api.PATCH("/v1/webhooks/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: webhookKeys.list(agent) }),
  });
}

export function useDeleteWebhook(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/webhooks/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: webhookKeys.list(agent) }),
  });
}
