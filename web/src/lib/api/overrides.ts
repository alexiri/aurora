"use client";

import { useInfiniteQuery } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type WorkspaceOverride = components["schemas"]["WorkspaceOverride"];

/** (C-964) The workspaces that tightened Internet access below the organization; `all` lists every workspace. */
export function useWorkspaceOverrides(orgId: string, all: boolean, enabled = true) {
  return useInfiniteQuery({
    queryKey: ["org", orgId, "workspace-overrides", all] as const,
    enabled,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(api.GET("/v1/orgs/{org}/workspace-overrides", { params: { path: { org: orgId }, query: { limit: 50, cursor: pageParam, all } } })),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry: false,
  });
}

const DEFAULT_RULE_WORDS: Record<string, string> = { none: "No default", allow: "Allow", block: "Block" };

/** One workspace's row of the organization's table, in words (`14 §8`). */
export function overrideValues(o: WorkspaceOverride, orgName: string): string[] {
  return [
    o.uncredentialed === "block" ? "Blocked unless listed" : `Same as ${orgName}`,
    o.open_lane === "allowlist_only" ? "Only listed websites" : o.open_lane === "disabled" ? "Turned off" : `Same as ${orgName}`,
    o.default_rule === "none" ? "None" : (DEFAULT_RULE_WORDS[o.default_rule] ?? o.default_rule),
    String(o.rule_count),
  ];
}
