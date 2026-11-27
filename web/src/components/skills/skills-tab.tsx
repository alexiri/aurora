"use client";

import { useParams } from "next/navigation";

import { useMe } from "@/lib/api/queries";
import { wsRoutes } from "@/lib/routes";

import type { SkillPage } from "./model";
import { SkillsList } from "./skills-list";

/** The agent's Skills page: one list of everything it can do, from all four levels. */
export function SkillsTab({
  agentId,
  orgId,
  wsId,
  handle,
  agentName = "your agent",
  readOnly = false,
}: {
  agentId: string;
  orgId?: string;
  wsId?: string;
  /** The agent's address, for Try it in chat. */
  handle?: string;
  agentName?: string;
  /** Emergency read access: skills can be read, not written. */
  readOnly?: boolean;
}) {
  const me = useMe().data;
  const params = useParams<{ org?: string; ws?: string }>();
  const membership = me?.memberships?.find((m) => m.org.id === orgId);
  // `/v1/me` sends the role that applies as `effective_role` where a sign-in rule raises it.
  const orgRole = (membership as { effective_role?: string | null } | undefined)?.effective_role ?? membership?.role;
  const workspaceRole = membership?.workspaces.find((w) => w.id === wsId)?.my_role;
  const org = params?.org;
  const ws = params?.ws;
  const page: SkillPage = {
    kind: "agent",
    agentId,
    agentName,
    orgId,
    orgName: membership?.org.name ?? "the organization",
    chat: handle && org && ws ? (draft) => wsRoutes(org, ws).agent(handle, { tab: "chat", draft }) : null,
    readOnly,
    // The API decides; this only keeps buttons nobody could use off the page.
    canDecide: {
      agent: !readOnly,
      workspace: !readOnly && workspaceRole === "owner",
      org: !readOnly && (orgRole === "owner" || orgRole === "admin"),
    },
  };
  return <SkillsList page={page} />;
}
