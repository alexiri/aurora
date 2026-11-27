"use client";

import { useParams, useRouter, useSearchParams } from "next/navigation";
import { useState } from "react";

import { AgentsList } from "@/components/agents-list";
import { ReviewUpdates } from "@/components/agents/review-updates";
import { ErrorState, Loading } from "@/components/states";
import { TemplateGalleryDialog } from "@/components/templates/gallery";
import { AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { MainAction } from "@/components/ui/main-action";
import { PageHeader } from "@/components/ui/page-header";
import { useAgentNeedsCounts } from "@/lib/api/agent-needs";
import { useApprovalsStream } from "@/lib/api/approvals";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";
import { queryKeys } from "@/lib/api/queries";
import { isEmergencyReader } from "@/lib/emergency";
import { wsRoutes } from "@/lib/routes";
import { whoCanFromContacts } from "@/lib/who-can";
import { useQueryClient } from "@tanstack/react-query";

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
  // (FIX-251, D-488(a)) `?needs_update=1`: the filter the update notice's **Review updates** lands
  // on — the list shows only the agents whose template offers a newer version.
  const needsUpdateFilter = useSearchParams().get("needs_update") === "1";
  const router = useRouter();
  const qc = useQueryClient();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const workspace = useWorkspace(wsId);
  // (P5-REFINE-11 item 11) Approvals waiting for the viewer, per agent row; nothing for a person who cannot decide.
  const needs = useAgentNeedsCounts(wsId);
  const base = `/o/${org}/w/${ws}/agents`;
  const behind = (agents.data?.items ?? []).filter((a) => a.needs_update);
  // The agents the running walkthrough reviews, fixed when it starts: every apply refetches the
  // list, so `behind` shrinks under a walkthrough that steps through it, skipping agents.
  const [reviewing, setReviewing] = useState<{ id: string; name: string; handle: string }[] | null>(null);
  // The walkthrough is over: the applies changed what is behind, so the list reads again and the
  // filter (now empty) is cleared.
  const onReviewed = () => {
    setReviewing(null);
    if (wsId) void qc.invalidateQueries({ queryKey: queryKeys.agents(wsId) });
    router.push(base);
  };

  // (FIX-277) The workspace answer first: it says whether the viewer only reads.
  if (resolved.isPending || (wsId && workspace.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  // (FIX-277, C-1165) Emergency read access: the list, with nothing to start or change, and no card summaries —
  // they read the workspace's accounts, which the grant doesn't cover.
  const emergency = isEmergencyReader(workspace.data);
  return (
    <AltitudeScope>
    <div className="space-y-4">
      <PageHeader
        title="Agents"
        description="The agents in this workspace."
        actions={
          workspace.data && !emergency ? (
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
      {!emergency && agents.data && (needsUpdateFilter || behind.length > 0) ? (
        <section className="space-y-2 rounded-card border border-line bg-surface p-4" data-testid="needs-update">
          <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
            <p className="text-sm text-ink-2">
              {behind.length === 0
                ? "No agents need an update."
                : behind.length === 1
                  ? "1 agent needs an update."
                  : `${behind.length} agents need an update.`}
            </p>
            <div className="flex flex-wrap gap-2">
              {behind.length > 0 ? (
                <Button type="button" size="sm" onClick={() => setReviewing(behind.map((a) => ({ id: a.id, name: a.name, handle: a.handle })))}>
                  Review updates
                </Button>
              ) : null}
              {needsUpdateFilter ? (
                <Button type="button" variant="outline" size="sm" onClick={() => router.push(base)}>
                  Show all agents
                </Button>
              ) : (
                <Button type="button" variant="outline" size="sm" onClick={() => router.push(`${base}?needs_update=1`)}>
                  Needs update
                </Button>
              )}
            </div>
          </div>
        </section>
      ) : null}
      {agents.data && !(needsUpdateFilter && behind.length === 0) ? (
        <AgentsList
          agents={needsUpdateFilter ? behind : agents.data.items}
          base={base}
          showTokenBadges={workspace.data?.my_role === "owner"}
          needs={needs.counts}
          wsId={emergency ? undefined : wsId}
          orgId={emergency ? undefined : resolved.data?.org.id}
          chat={!emergency}
        />
      ) : null}
      {reviewing ? (
        <ReviewUpdates
          agents={reviewing}
          agentBaseOf={(handle) => `${base}/${handle}`}
          onClose={onReviewed}
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
