"use client";

import { useQuery } from "@tanstack/react-query";

import { api } from "./client";
import { retryUnlessClientError } from "./errors";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type WorkspaceUsage = S["WorkspaceUsage"];
export type InstanceUsage = S["InstanceUsage"];
export type OrgUsage = S["OrgUsage"];
export type UsageCounters = S["UsageCounters"];
export type UsageModel = S["UsageModel"];

export const usageKeys = {
  workspace: (ws: string, from: string, to: string, agent: string) => ["usage", "workspace", ws, from, to, agent] as const,
  org: (org: string, from: string, to: string, ws: string) => ["usage", "org", org, from, to, ws] as const,
  instance: (from: string, to: string, org: string) => ["usage", "instance", from, to, org] as const,
};

/** (C-1000) A workspace's usage; `agent` narrows it to one agent (any id). */
export function useWorkspaceUsage(wsId: string | undefined, from: string, to: string, agent: string, enabled = true) {
  return useQuery({
    queryKey: usageKeys.workspace(wsId ?? "", from, to, agent),
    queryFn: () =>
      unwrap(
        api.GET("/v1/workspaces/{ws}/usage", {
          params: { path: { ws: wsId! }, query: { from, to, agent_id: agent || undefined } },
        }),
      ),
    enabled: !!wsId && enabled,
    retry: retryUnlessClientError,
  });
}

/** (C-1006) An organization's usage; `workspace` narrows it to one workspace. */
export function useOrgUsage(orgId: string | undefined, from: string, to: string, workspace: string, enabled = true) {
  return useQuery({
    queryKey: usageKeys.org(orgId ?? "", from, to, workspace),
    queryFn: () =>
      unwrap(
        api.GET("/v1/orgs/{org}/usage", {
          params: { path: { org: orgId! }, query: { from, to, workspace_id: workspace || undefined } },
        }),
      ),
    enabled: !!orgId && enabled,
    retry: retryUnlessClientError,
  });
}

/** (C-1003) The instance's usage; `org` narrows it to one organization. */
export function useInstanceUsage(from: string, to: string, org: string, enabled = true) {
  return useQuery({
    queryKey: usageKeys.instance(from, to, org),
    queryFn: () =>
      unwrap(api.GET("/v1/admin/usage", { params: { query: { from, to, org_id: org || undefined } } })),
    enabled,
    retry: retryUnlessClientError,
  });
}
