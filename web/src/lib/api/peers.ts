"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type AgentLink = S["AgentLink"];
export type LinkPolicy = S["LinkPolicy"];
export type PeerTask = components["schemas"]["Page_PeerTask"]["items"][number];
/** Who may hand tasks to whom, read from the asked agent's side (C-1085). */
export type LinkDirection = S["LinkDirection"];
/** The direction a person picks, read from the agent whose settings are open (C-1085). */
export type LinkDirectionChoice = S["LinkDirectionRequest"];
export type LinkableAgent = S["LinkableAgent"];

export const LINK_POLICIES: LinkPolicy[] = ["ask", "allow", "blocked"];
/** The three choices of the "Link another agent" dialog, in the mockup's order. */
export const LINK_DIRECTION_CHOICES: LinkDirectionChoice[] = ["both", "to_peer", "from_peer"];

export const peerKeys = {
  links: (agent: string) => ["agents", agent, "links"] as const,
  linkable: (agent: string) => ["agents", agent, "linkable"] as const,
  tasks: (agent: string) => ["agents", agent, "peer-tasks"] as const,
};

function oneWay(direction: LinkDirection | LinkDirectionChoice): "both" | "outgoing" | "incoming" {
  if (direction === "to_peer") return "outgoing";
  if (direction === "from_peer") return "incoming";
  return direction;
}

/** The words for an agent not chosen yet (the link dialog before a pick). */
export const OTHER_AGENT = "the other agent";

/** A name at the start of a sentence: the placeholder is capitalised ("The other agent"), a real name is left as it is. */
const lead = (name: string): string => (name === OTHER_AGENT ? "The other agent" : name);

/** The option title of a direction: "Both ways", "Daneel can hand tasks to Rex", "Rex can hand tasks to Daneel". */
export function directionTitle(selfName: string, peerName: string, direction: LinkDirection | LinkDirectionChoice): string {
  switch (oneWay(direction)) {
    case "outgoing":
      return `${selfName} can hand tasks to ${peerName}`;
    case "incoming":
      return `${lead(peerName)} can hand tasks to ${selfName}`;
    default:
      return "Both ways";
  }
}

/** What a direction means, as the mockup says it under each option (C-1085). */
export function directionSentence(selfName: string, peerName: string, direction: LinkDirection | LinkDirectionChoice): string {
  switch (oneWay(direction)) {
    case "outgoing":
      return `${lead(peerName)} only works on what ${selfName} sends.`;
    case "incoming":
      return `${selfName} only works on what ${peerName} sends.`;
    default:
      return `${selfName} and ${peerName} can each give the other tasks.`;
  }
}

/** Agents this one may be linked to (C-1085); fetched only while the picker is open. */
export function useLinkableAgents(agent: string, enabled: boolean) {
  return useQuery({
    queryKey: peerKeys.linkable(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/linkable", { params: { path: { agent } } })),
    enabled,
    retry: false,
  });
}

function useInvalidateLinks(agent: string) {
  const qc = useQueryClient();
  return () => {
    void qc.invalidateQueries({ queryKey: peerKeys.links(agent) });
    void qc.invalidateQueries({ queryKey: peerKeys.linkable(agent) });
  };
}

/** Link `agent` to another agent (C-1085). A 409 `link_exists` carries `details.link_id`. */
export function useCreateLink(agent: string) {
  const invalidate = useInvalidateLinks(agent);
  return useMutation({
    mutationFn: ({ peerAgentId, direction }: { peerAgentId: string; direction: LinkDirectionChoice }) =>
      unwrap(
        api.POST("/v1/agents/{agent}/links", {
          params: { path: { agent } },
          body: { peer_agent_id: peerAgentId, direction },
        }),
      ),
    onSettled: invalidate,
  });
}

/** Change who may hand tasks to whom, read from `link.self` (C-1085); widening needs an approver of both sides. */
export function useSetLinkDirection(agent: string) {
  const invalidate = useInvalidateLinks(agent);
  return useMutation({
    mutationFn: ({ link, direction }: { link: AgentLink; direction: LinkDirectionChoice }) =>
      unwrap(
        api.PUT("/v1/agent-links/{id}/direction", {
          params: { path: { id: link.id } },
          body: { agent_id: link.self.agent_id, direction, revision: link.revision },
        }),
      ),
    onSettled: invalidate,
  });
}

/** Remove a link (C-1085): its open and queued tasks are cancelled. */
export function useDeleteLink(agent: string) {
  const invalidate = useInvalidateLinks(agent);
  return useMutation({
    mutationFn: (link: AgentLink) =>
      unwrap(api.DELETE("/v1/agent-links/{id}", { params: { path: { id: link.id } } })),
    onSettled: invalidate,
  });
}

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
