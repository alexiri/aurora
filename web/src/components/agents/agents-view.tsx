"use client";

import { useParams, useRouter } from "next/navigation";
import { useState } from "react";

import { AgentsList } from "@/components/agents-list";
import { ErrorState, Loading } from "@/components/states";
import { TemplateGalleryDialog } from "@/components/templates/gallery";
import { AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { MainAction } from "@/components/ui/main-action";
import { PageHeader } from "@/components/ui/page-header";
import { useAgentNeedsCounts } from "@/lib/api/agent-needs";
import { useApprovalsStream } from "@/lib/api/approvals";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";
import { wsRoutes } from "@/lib/routes";
import { whoCanFromContacts } from "@/lib/who-can";

import { CreateAgentDialog } from "./create-agent-dialog";

/** Keeps the pending approvals live (the stream refetches them), so a row's count drops when someone decides. */
function ApprovalsStreamOpen({ wsId }: { wsId: string }) {
  useApprovalsStream(wsId);
  return null;
}

/**
 * (FIX-245) **Start from a template**, beside **New agent**: the gallery of the workspace's offered
 * templates, always present — also when nothing is approved yet, because the empty gallery says why.
 * Choosing a card's **Use this template** opens the four-step page.
 */
function StartFromTemplate({ wsId, org, ws }: { wsId: string; org: string; ws: string }) {
  const router = useRouter();
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant="outline" onClick={() => setOpen(true)}>
        Start from a template
      </Button>
      <TemplateGalleryDialog
        open={open}
        onOpenChange={setOpen}
        wsId={wsId}
        onUse={(templateId) => {
          setOpen(false);
          router.push(wsRoutes(org, ws).newAgent(templateId));
        }}
      />
    </>
  );
}

export function AgentsView() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const workspace = useWorkspace(wsId);
  // (P5-REFINE-11 item 11) Approvals waiting for the viewer, per agent row; nothing for a person who cannot decide.
  const needs = useAgentNeedsCounts(wsId);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  return (
    <AltitudeScope>
    <div className="space-y-4">
      <PageHeader
        title="Agents"
        description="The agents in this workspace."
        actions={
          workspace.data ? (
            <div className="flex flex-wrap items-center justify-end gap-2">
              <StartFromTemplate wsId={wsId} org={org} ws={ws} />
              <MainAction canAct={workspace.data.my_role === "owner"} whoCan={whoCanFromContacts("workspace_owner", workspace.data.contacts, "add_agent")}>
                <CreateAgentDialog wsId={wsId} />
              </MainAction>
            </div>
          ) : null
        }
        className="mb-2"
      />
      {agents.isPending ? <Loading /> : null}
      {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
      {agents.data ? (
        <AgentsList
          agents={agents.data.items}
          base={`/o/${org}/w/${ws}/agents`}
          showTokenBadges={workspace.data?.my_role === "owner"}
          needs={needs.counts}
          wsId={wsId}
          orgId={resolved.data?.org.id}
        />
      ) : null}
      {needs.canDecide && wsId ? (
        <ApprovalsStreamOpen wsId={wsId} />
      ) : null}
      <MoreControlsLine />
    </div>
    </AltitudeScope>
  );
}
