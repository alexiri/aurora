"use client";

// (FIX-235, D-477) The manage page for a workspace an administrator is not in
// (`/o/<org>/workspaces/<ws>`): people, agents, pause and delete — everything an organization
// admin may do from the outside, and nothing that opens the workspace's chats or agents.

import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useState } from "react";

import { AgentFreeze, WorkspaceFreeze } from "@/components/freeze/freeze-sections";
import { OpenEmergencyAccess } from "@/components/emergency/emergency-open";
import { AccessTab } from "@/components/settings/access-tab";
import { AddPeople } from "@/components/settings/add-people";
import { DangerTab } from "@/components/settings/danger-tab";
import { ErrorState, Loading } from "@/components/states";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { useAgents, useMe, useOrg, useResolve, useWorkspace, type Agent } from "@/lib/api/queries";
import { usePutAccess } from "@/lib/api/account";
import { emergencyOpenText, isEmergencyReader } from "@/lib/emergency";
import { orgRoutes, wsRoutes } from "@/lib/routes";
import { personName } from "@/lib/workspace-name";

/**
 * The notice at the top of the manage page (FIX-235): why this page is here, not the workspace. While the viewer's own
 * emergency read access is open (`emergency`, FIX-277) they can open the chats, so it says that it is open and until
 * when instead of offering it, with the way in: the organization's lists lead the reader to this page.
 */
function ManageNotice({ personal, personFirstName, orgOwner, emergency }: { personal: boolean; personFirstName: string | null; orgOwner: boolean; emergency: { until: string | null; agentsPath: string } | null }) {
  return (
    <Notice role="note" data-testid="manage-notice">
      <div className="space-y-1 text-sm">
        <p>You manage this workspace as an organization admin. You&rsquo;re not one of its people, so you can&rsquo;t {emergency ? "" : "open its chats or "}use its agents.</p>
        {personal && personFirstName ? <p>Only {personFirstName} can add people to it.</p> : null}
        {emergency ? (
          <p>
            {emergencyOpenText(emergency.until)}{" "}
            <Link href={emergency.agentsPath} className="font-medium text-link underline">
              Read the workspace&rsquo;s agents
            </Link>
          </p>
        ) : orgOwner ? (
          <p>In an emergency you can open read access.</p>
        ) : null}
      </div>
    </Notice>
  );
}

/** Adding yourself (C-1161's self-grant): the confirmation ladder's danger step, then `PUT access/{me}`. */
function AddYourself({ wsId, wsName, meId }: { wsId: string; wsName: string; meId: string }) {
  const [open, setOpen] = useState(false);
  const [role, setRole] = useState<"member" | "owner">("member");
  const put = usePutAccess(wsId);
  return (
    <>
      <Button variant="outline" size="sm" onClick={() => setOpen(true)} data-testid="add-yourself">
        Add yourself…
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={`Add yourself to ${wsName}?`}
        description="You'll be able to use its agents and their accounts, and as an owner read its people's private chats. Its owners will be told."
        confirmLabel={`Add yourself to ${wsName}`}
        danger
        busy={put.isPending}
        onConfirm={() => put.mutate({ user: meId, role }, { onSuccess: () => setOpen(false) })}
      >
        <div className="space-y-2 text-sm">
          <p className="text-ink-2">
            Your role there:
            <span className="ml-2 inline-flex gap-4">
              <label className="inline-flex items-center gap-1.5">
                <input type="radio" name="add-yourself-role" checked={role === "member"} onChange={() => setRole("member")} /> Member
              </label>
              <label className="inline-flex items-center gap-1.5">
                <input type="radio" name="add-yourself-role" checked={role === "owner"} onChange={() => setRole("owner")} /> Owner
              </label>
            </span>
          </p>
          {put.isError ? <ErrorNotice error={put.error} className="text-danger-ink" data-testid="add-yourself-error" /> : null}
        </div>
      </ConfirmDialog>
    </>
  );
}

/** One agent row: name, handle, state, Pause/Resume — no link into the agent (D-477). */
function AgentRow({ agent, orgId, canOwn }: { agent: Agent; orgId: string; canOwn: boolean }) {
  return (
    <li className="flex flex-wrap items-center justify-between gap-2 border-b border-line py-2 last:border-0" data-testid="manage-agent-row">
      <div className="min-w-0">
        <span className="font-medium">{agent.name}</span>
        {agent.handle ? <span className="ml-2 text-sm text-muted">@{agent.handle}</span> : null}
        {agent.frozen ? (
          <Badge tone="neutral" className="ml-2">
            Paused
          </Badge>
        ) : null}
      </div>
      <AgentFreeze agentId={agent.id} name={agent.name} orgId={orgId} frozen={agent.frozen} canOwn={canOwn} buttonLabel={agent.frozen ? "Resume" : "Pause…"} buttonVariant="outline" />
    </li>
  );
}

/** The manage page's body: the notice, People, Agents, Pause and Delete. */
export function WorkspaceManage() {
  const router = useRouter();
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const agents = useAgents(wsId);
  const me = useMe();
  const meId = me.data?.user.id;
  const orgOwner = useOrg(resolved.data?.org.id).data?.my_role === "owner";

  if (resolved.isPending || (wsId && workspace.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  const w = workspace.data!;
  const personal = !!w.personal_of;
  const personFirstName = personal ? personName(w.personal_of?.name) : null;
  const canAdminister = w.administers || w.my_role === "owner";

  return (
    <div className="space-y-6">
      <PageHeader
        title={w.name}
        description="Managed as an organization admin."
        breadcrumb={[{ label: "Overview", href: orgRoutes(org).home() }]}
        actions={personal ? <Badge tone="neutral">Personal</Badge> : undefined}
      />
      {w.my_role == null ? <ManageNotice personal={personal} personFirstName={personFirstName} orgOwner={orgOwner} emergency={isEmergencyReader(w) ? { until: w.emergency_reader_expires_at ?? null, agentsPath: wsRoutes(org, ws).agents() } : null} /> : null}

      <Section id="manage-people" title="People" description="Who can use this workspace.">
        <div className="space-y-4">
          {personal ? null : (
            <AddPeople wsId={w.id} orgId={w.org_id} workspacePath={orgRoutes(org).workspace(ws)} />
          )}
          {meId && w.my_role == null ? <AddYourself wsId={w.id} wsName={w.name} meId={meId} /> : null}
          {/* (FIX-236, review LOW-11, D-470) Only where opening can succeed: an org owner who
              is one of the workspace's owners (409 already_has_access) or already holds a grant
              (409 emergency_access_active) gets no entry. */}
          {orgOwner && w.my_role !== "owner" && !isEmergencyReader(w) ? (
            <OpenEmergencyAccess wsId={w.id} wsName={w.name} />
          ) : null}
          <AccessTab wsId={w.id} orgId={w.org_id} canOwn={canAdminister} personal={personal} />
        </div>
      </Section>

      <Section
        id="manage-agents"
        title="Agents"
        // (FIX-280) While the viewer's own grant is open they can read the agents, so the page doesn't say otherwise.
        description={`The workspace's agents. You can pause or resume them${isEmergencyReader(w) ? "." : "; you cannot open them."}`}
      >
        {agents.isPending ? (
          <Loading />
        ) : agents.isError ? (
          <ErrorState error={agents.error} onRetry={() => agents.refetch()} />
        ) : agents.data.items.length === 0 ? (
          <p className="text-sm text-muted">No agents yet.</p>
        ) : (
          <ul>
            {agents.data.items.map((a) => (
              <AgentRow key={a.id} agent={a} orgId={w.org_id} canOwn={canAdminister} />
            ))}
          </ul>
        )}
      </Section>

      <Section id="manage-danger" title="Danger zone" description="The workspace's pause and delete controls.">
        <div className="space-y-6">
          <WorkspaceFreeze wsId={w.id} name={w.name} orgId={w.org_id} frozen={w.frozen} />
          <DangerTab wsId={w.id} slug={w.slug} name={w.name} onDeleted={() => router.replace(orgRoutes(org).home())} />
        </div>
      </Section>
    </div>
  );
}
