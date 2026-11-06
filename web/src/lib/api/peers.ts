"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type AgentLink = S["AgentLink"];
export type LinkPolicy = S["LinkPolicy"];
export type PeerTask = components["schemas"]["Page_PeerTask"]["items"][number];

export const LINK_POLICIES: LinkPolicy[] = ["ask", "allow", "blocked"];

export const peerKeys = {
  links: (agent: string) => ["agents", agent, "links"] as const,
  tasks: (agent: string) => ["agents", agent, "peer-tasks"] as const,
};

export function useAgentLinks(agent: string) {
  return useQuery({
    queryKey: peerKeys.links(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/links", { params: { path: { agent } } })),
    retry: false,
  });
}

export function usePeerTasks(agent: string) {
  return useQuery({
    queryKey: peerKeys.tasks(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/peer-tasks", { params: { path: { agent } }, query: { limit: 50 } })),
    retry: false,
  });
}

export function useSetLinkPolicy(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ link, policy }: { link: AgentLink; policy: LinkPolicy }) =>
      unwrap(
        api.PUT("/v1/agent-links/{id}/policy", {
          params: { path: { id: link.id } },
          body: { agent_id: link.self.agent_id, policy, revision: link.revision },
        }),
      ),
    // On a stale revision (409) refetch so the next attempt carries the current revision.
    onSettled: (_d, err) => {
      if (!err || (err instanceof ApiError && err.status === 409)) {
        void qc.invalidateQueries({ queryKey: peerKeys.links(agent) });
      }
    },
  });
}
