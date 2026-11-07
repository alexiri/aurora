"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type Offboarding = S["Offboarding"];
export type OffboardingItem = S["OffboardingItem"];

/** Who an automation's authority goes to: a user id, or the workspace itself. */
export type ReassignTarget = string | "workspace";

/** What removing or suspending `user` would disable, across the org (org owners/admins). */
export function useOrgOffboarding(orgId: string, user: string | null) {
  return useQuery({
    queryKey: ["orgs", orgId, "members", user, "offboarding"] as const,
    queryFn: () =>
      unwrap(
        api.GET("/v1/orgs/{org}/members/{user}/offboarding", {
          params: { path: { org: orgId, user: user! } },
        }),
      ),
    enabled: !!user,
    retry: false,
    gcTime: 0,
  });
}

/** What removing `user`'s binding on a workspace would disable (workspace owners). */
export function useWorkspaceOffboarding(ws: string, user: string | null) {
  return useQuery({
    queryKey: ["workspaces", ws, "members", user, "offboarding"] as const,
    queryFn: () =>
      unwrap(
        api.GET("/v1/workspaces/{ws}/members/{user}/offboarding", {
          params: { path: { ws, user: user! } },
        }),
      ),
    enabled: !!user,
    retry: false,
    gcTime: 0,
  });
}

/** `PUT /v1/{schedules|webhooks}/{id}/creator`; refreshes the agent's list afterwards. */
export function useReassignCreator(agent: string, kind: "schedule" | "webhook") {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ id, user_id, re_enable }: { id: string; user_id: string; re_enable?: boolean }): Promise<unknown> =>
      kind === "schedule"
        ? unwrap(
            api.PUT("/v1/schedules/{id}/creator", {
              params: { path: { id } },
              body: { user_id, re_enable },
            }),
          )
        : unwrap(
            api.PUT("/v1/webhooks/{id}/creator", {
              params: { path: { id } },
              body: { user_id, re_enable },
            }),
          ),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: ["agents", agent, kind === "schedule" ? "schedules" : "webhooks"] }),
  });
}
