"use client";

// Port previews (P4-PRV-1, D-144, SEC-80): share management for hosted agents. Management only:
// these routes never accept an agent credential, and the browser-facing access is the relay (P4-PRV-2).
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type PreviewShare = S["PreviewShare"];
export type PreviewShareList = S["PreviewShareList"];
export type CreatePreviewShareRequest = S["CreatePreviewShareRequest"];
export type PatchPreviewShareRequest = S["PatchPreviewShareRequest"];

export const previewKeys = {
  list: (agent: string) => ["agents", agent, "previews"] as const,
};

/** How often the share list refetches while a share is `waking` (5 s: a wake or start is under way). */
export const PREVIEW_WAKING_POLL_MS = 5000;

export function usePreviews(agent: string) {
  return useQuery({
    queryKey: previewKeys.list(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/previews", { params: { path: { agent } } })),
    retry: false,
    // A share that is waking changes without anyone touching the page (the sandbox boots, the
    // command starts or exits): poll while any is, and stop as soon as none is.
    refetchInterval: (q) =>
      q.state.data?.shares.some((s) => s.status === "waking") ? PREVIEW_WAKING_POLL_MS : false,
  });
}

export function useCreatePreview(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreatePreviewShareRequest) =>
      unwrap(api.POST("/v1/agents/{agent}/previews", { params: { path: { agent } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: previewKeys.list(agent) }),
  });
}

export function usePatchPreview(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: PatchPreviewShareRequest }) =>
      unwrap(api.PATCH("/v1/previews/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: previewKeys.list(agent) }),
  });
}

export function useDeletePreview(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/previews/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: previewKeys.list(agent) }),
  });
}

// 202: the demand was recorded (the sandbox boots or the command launches); the port is not
// listening yet. The list refetch shows the derived `waking` status.
export function useStartPreview(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.POST("/v1/previews/{id}/start", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: previewKeys.list(agent) }),
  });
}
