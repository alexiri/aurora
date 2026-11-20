"use client";

import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type PlacementView = components["schemas"]["PlacementView"];
export type PutPlacement = components["schemas"]["PutPlacement"];
export type PlacementMode = components["schemas"]["PlacementMode"];
export type PlacementChoices = components["schemas"]["PlacementChoices"];
export type PlacementPreview = components["schemas"]["PlacementPreview"];
/** (C-1061) What a save gave to agents that already existed: "Given to Daneel and 1 other agent". */
export type StarterApplied = components["schemas"]["StarterApplied"];

/** The saved new-user placement (D-140; instance admins). */
export function usePlacement(enabled = true) {
  return useQuery({
    queryKey: ["admin", "placement"] as const,
    queryFn: () => unwrap(api.GET("/v1/admin/placement")),
    enabled,
    retry: false,
  });
}

export function useSavePlacement() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PutPlacement) => unwrap(api.PUT("/v1/admin/placement", { body })),
    onSuccess: (v) => {
      qc.setQueryData(["admin", "placement"], v);
      void qc.invalidateQueries({ queryKey: ["admin", "placement"] });
    },
  });
}

/** (C-922) Give the starter grants to the agents of people who signed in before: `{ granted, skipped }`. */
export function useBackfillStarterGrants() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.POST("/v1/admin/placement/starter-grants/backfill", { body: {} })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["admin", "setup"] });
    },
  });
}

/** Workspaces and org-level credentials the form may offer for the chosen default organization. */
export function usePlacementChoices(orgId: string | null) {
  return useQuery({
    queryKey: ["admin", "placement", "choices", orgId] as const,
    queryFn: () => unwrap(api.GET("/v1/admin/placement/choices", { params: { query: { org_id: orgId! } } })),
    enabled: !!orgId,
    retry: false,
  });
}

/** Live preview of a template for a typed sample (empty sample = the caller's own profile). */
export function usePlacementPreview(template: string, sample: string) {
  return useQuery({
    queryKey: ["admin", "placement", "preview", template, sample] as const,
    queryFn: () =>
      unwrap(
        api.GET("/v1/admin/placement/preview", {
          params: { query: { template, sample: sample.trim() || undefined } },
        }),
      ),
    placeholderData: keepPreviousData,
    retry: false,
  });
}
