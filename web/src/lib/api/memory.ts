"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Memory = components["schemas"]["Memory"];
export type MemoryRevision = components["schemas"]["MemoryRevision"];
export type MemorySearchHit = components["schemas"]["MemorySearchHit"];
export type PutMemoryRequest = components["schemas"]["PutMemoryRequest"];

/** Limits from docs/02 (`MEMORY_CONTENT_MAX`, `MEMORY_MAX_PER_AGENT`); the API stays authoritative. */
export const MEMORY_CONTENT_MAX = 100_000;
export const MEMORY_MAX_PER_AGENT = 100;
export const MEMORY_KEY_MAX = 64;
export const MEMORY_KEY_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;

export const memKeys = {
  list: (agent: string) => ["agents", agent, "memories"] as const,
  search: (agent: string, q: string) => ["agents", agent, "memories", "search", q] as const,
  revisions: (agent: string, key: string) => ["agents", agent, "memories", key, "revisions"] as const,
};

export function useMemories(agent: string) {
  return useQuery({
    queryKey: memKeys.list(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/memories", { params: { path: { agent } } })),
    retry: false,
  });
}

export function useMemorySearch(agent: string, q: string) {
  return useQuery({
    queryKey: memKeys.search(agent, q),
    queryFn: () =>
      unwrap(api.GET("/v1/agents/{agent}/memories/search", { params: { path: { agent }, query: { q } } })),
    enabled: q.trim().length > 0,
    retry: false,
  });
}

export function useMemoryRevisions(agent: string, key: string, enabled: boolean) {
  return useQuery({
    queryKey: memKeys.revisions(agent, key),
    queryFn: () =>
      unwrap(api.GET("/v1/agents/{agent}/memories/{key}/revisions", { params: { path: { agent, key } } })),
    enabled,
    retry: false,
  });
}

function useInvalidate(agent: string) {
  const qc = useQueryClient();
  return () => qc.invalidateQueries({ queryKey: memKeys.list(agent) });
}

export function useCreateMemory(agent: string) {
  const inv = useInvalidate(agent);
  return useMutation({
    mutationFn: (body: components["schemas"]["CreateMemoryRequest"]) =>
      unwrap(api.POST("/v1/agents/{agent}/memories", { params: { path: { agent } }, body })),
    onSuccess: inv,
  });
}

export function usePutMemory(agent: string, key: string) {
  const inv = useInvalidate(agent);
  return useMutation({
    mutationFn: (body: PutMemoryRequest) =>
      unwrap(api.PUT("/v1/agents/{agent}/memories/{key}", { params: { path: { agent, key } }, body })),
    onSuccess: inv,
  });
}

export function useDeleteMemory(agent: string) {
  const inv = useInvalidate(agent);
  return useMutation({
    mutationFn: (key: string) =>
      unwrap(api.DELETE("/v1/agents/{agent}/memories/{key}", { params: { path: { agent, key } } })),
    onSuccess: inv,
  });
}

export function useRestoreMemory(agent: string, key: string) {
  const inv = useInvalidate(agent);
  return useMutation({
    mutationFn: (revision: number) =>
      unwrap(
        api.POST("/v1/agents/{agent}/memories/{key}/restore", {
          params: { path: { agent, key } },
          body: { revision },
        }),
      ),
    onSuccess: inv,
  });
}

export function useRedactRevision(agent: string, key: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (rev: number) =>
      unwrap(
        api.POST("/v1/agents/{agent}/memories/{key}/revisions/{rev}/redact", {
          params: { path: { agent, key, rev } },
        }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: memKeys.revisions(agent, key) }),
  });
}

/** Meter state for a text against a limit. */
export function meter(used: number, max: number) {
  return { used, max, over: used > max, ratio: Math.min(1, used / max) };
}

/** The latest revision is the one equal to the memory's current `revision`; it can never be redacted. */
export function canRedact(rev: MemoryRevision, latest: number): boolean {
  return !rev.redacted && rev.revision !== latest;
}
