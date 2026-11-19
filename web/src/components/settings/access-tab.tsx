"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Avatar } from "@/components/agent-avatar";
import { Button } from "@/components/ui/button";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { useMe } from "@/lib/api/queries";
import { sourceText } from "@/lib/api/groups";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { IdpRoleBadge } from "@/components/identity/idp-role-badge";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { matchesFilter, PickerFilter } from "@/components/ui/picker-filter";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import {
  useAccess,
  useDeleteAccess,
  useOrgMembers,
  usePutAccess,
  type AccessBinding,
} from "@/lib/api/account";
import { OffboardingDialog } from "@/components/automations/offboarding-dialog";
import { AddPeople, WaitingList } from "./add-people";
import { GroupAccess } from "./group-access";
import { useWorkspaceOffboarding } from "@/lib/api/automations";

type Role = "owner" | "approver" | "member";

/** One-line explanation of each role (phase 2, RFE-012). */
const ROLE_LABEL: Record<Role, string> = { owner: "Owner", approver: "Approver", member: "Member" };

export const ROLE_HELP: Record<Role, string> = {
  owner: "Owner: manages the workspace, its access and settings, and can decide approvals.",
  approver: "Approver: chats with agents and can decide approvals.",
  member: "Member: chats with agents; cannot decide approvals.",
};

function AddBinding({
  wsId,
  orgId,
  bound,
}: {
  wsId: string;
  orgId: string;
  bound: Set<string>;
}) {
  const members = useOrgMembers(orgId);
  const put = usePutAccess(wsId);
  const [user, setUser] = useState("");
  const [role, setRole] = useState<Role>("member");
  const [filter, setFilter] = useState("");

  // Plain org members cannot list members (403): hide the picker entirely.
  if (members.isPending || members.isError) return null;
  const all = members.data.items.filter((m) => !bound.has(m.user_id));
  if (all.length === 0) {
    return <p className="text-sm text-ink-2">Every active org member already has access.</p>;
  }
  // Long lists get a name/e-mail filter; the picked person stays listed while the filter changes.
  const candidates = all.filter((m) => m.user_id === user || matchesFilter(filter, m.display_name, m.email));
  return (
    <Section id="acc-add" title="Add access" description="Give an organization member a role in this workspace." card>
      <form
        aria-label="Add access"
        className="flex flex-wrap items-start gap-x-4 gap-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (!user) return;
          put.mutate({ user, role }, { onSuccess: () => setUser("") });
        }}
      >
        <PickerFilter id="acc-add-filter" label="Find by name or e-mail" count={all.length} value={filter} onChange={setFilter} />
        <Field id="acc-add-user" label="Organization member">
          <select
            id="acc-add-user"
            className={cn(inputClass, "w-72 max-w-full")}
            value={user}
            onChange={(e) => setUser(e.target.value)}
          >
            <option value="">Select a member…</option>
            {candidates.map((m) => (
              <option key={m.user_id} value={m.user_id}>
                {m.display_name} ({m.email})
              </option>
            ))}
          </select>
        </Field>
        <Field id="acc-add-role" label="Role">
          <Select id="acc-add-role" value={role} className="w-40 max-w-full" onChange={(v) => setRole(v as Role)} options={[{ value: "member", label: "member" }, { value: "approver", label: "approver" }, { value: "owner", label: "owner" }]} />
        </Field>
        <Button type="submit" disabled={!user || put.isPending} className="sm:mt-label-offset">
          Add
        </Button>
        <ErrorNotice error={put.error} className="basis-full" />
      </form>
    </Section>
  );
}

function Row({
  b,
  wsId,
  canOwn,
  candidates,
  onError,
  mine,
  lastOwner,
}: {
  b: AccessBinding;
  /** This row is the viewer's own: no menu and no role change. */
  mine: boolean;
  /** The only direct owner: the roles below Owner are not offered. */
  lastOwner: boolean;
  wsId: string;
  canOwn: boolean;
  candidates: { id: string; label: string }[];
  onError: (e: unknown) => void;
}) {
  const put = usePutAccess(wsId);
  const del = useDeleteAccess(wsId);
  const busy = put.isPending || del.isPending;
  const [confirm, setConfirm] = useState(false);
  const offboarding = useWorkspaceOffboarding(wsId, confirm ? b.user_id : null);
  return (
    <tr>
      <Td>
        <div className="flex items-center gap-3">
          <Avatar name={b.display_name || b.email} tintKey={b.user_id} size={32} />
          <div className="min-w-0">
            <div className="font-medium">
              {b.display_name}
              {mine ? <span className="ml-1 font-normal text-muted">(you)</span> : null}
            </div>
            <div className="text-xs text-muted">{b.email}</div>
          </div>
        </div>
      </Td>
      <Td>
        {b.implicit ? (
          <span>
            Owner <span className="text-xs text-muted">(organization)</span>
          </span>
        ) : b.idp_value ? (
          <span className="flex flex-wrap items-center gap-1.5">
            {ROLE_LABEL[b.role as Role] ?? b.role}
            <IdpRoleBadge value={b.idp_value} />
          </span>
        ) : canOwn && !mine ? (
          <select
            aria-label={`Role for ${b.email}`}
            className={cn(inputClass, "w-36")}
            value={b.role}
            disabled={busy}
            onChange={(e) =>
              put.mutate({ user: b.user_id, role: e.target.value as Role }, { onError })
            }
          >
            {/* The last owner's role cannot be lowered (the server refuses): the option is not offered. */}
            {lastOwner ? null : <option value="member">Member</option>}
            {lastOwner ? null : <option value="approver">Approver</option>}
            <option value="owner">Owner</option>
          </select>
        ) : (
          ROLE_LABEL[b.role as Role] ?? b.role
        )}
      </Td>
      <Td className="text-right">
        {/* (P5-POLISH-7, D6) A … menu; never on your own row, and not for access that comes from the organization or the IdP. */}
        {canOwn && !b.implicit && !b.idp_value && !mine ? (
          <Menu>
            <MenuTrigger asChild>
              <Button variant="outline" size="sm" disabled={busy} aria-label={`More for ${b.display_name || b.email}`}>
                <span aria-hidden="true">…</span>
              </Button>
            </MenuTrigger>
            <MenuContent align="end">
              <MenuItem className="text-danger-ink" onSelect={() => setConfirm(true)} aria-label={`Remove ${b.email}`}>
                Remove from workspace…
              </MenuItem>
            </MenuContent>
          </Menu>
        ) : null}
        <OffboardingDialog
          open={confirm}
          title="Remove access?"
          description={`${b.email} will lose access to this workspace.`}
          who={b.display_name || b.email}
          confirmLabel="Remove access"
          query={offboarding}
          candidates={candidates}
          busy={del.isPending}
          error={del.error}
          onClose={() => {
            setConfirm(false);
            del.reset();
          }}
          onConfirm={(reassign_to) =>
            del.mutate(
              { user: b.user_id, reassign_to },
              { onSuccess: () => setConfirm(false), onError },
            )
          }
        />
      </Td>
    </tr>
  );
}

export function AccessTab({
  wsId,
  orgId,
  canOwn,
  people,
  personal = false,
}: {
  /** A personal workspace has no Groups. */
  personal?: boolean;
  wsId: string;
  orgId: string;
  canOwn: boolean;
  /** The People page's **Add people** (P5-UX-5): the workspace's address to send, and whether `?add=1` opened it. */
  people?: { workspacePath: string; initiallyOpen: boolean };
}) {
  const access = useAccess(wsId);
  const meId = useMe().data?.user.id;
  const [err, setErr] = useState<unknown>(null);
  if (access.isPending) return <Loading />;
  if (access.isError) return <ErrorState error={access.error} onRetry={() => access.refetch()} />;
  const items = access.data.items;
  const bound = new Set(items.map((b) => b.user_id));
  const inherited = (access.data.derived_members?.items ?? []).filter((d) => !bound.has(d.user.id));
  const directOwners = items.filter((b) => b.role === "owner" && !b.implicit).length;
  return (
    <div className="space-y-6">
      {!canOwn ? <Notice role="status">Only workspace owners can change who has access.</Notice> : null}
      {canOwn && people ? (
        <div className="flex justify-end">
          <AddPeople
            wsId={wsId}
            orgId={orgId}
            workspacePath={people.workspacePath}
            hasApprover={items.some((b) => b.role === "approver")}
            initiallyOpen={people.initiallyOpen}
          />
        </div>
      ) : null}
      <ErrorNotice error={err} />
      {items.length === 0 && inherited.length === 0 ? (
        <EmptyState title="No one has access yet" />
      ) : (
        <DataTable caption="People with access to this workspace">
          <thead>
            <tr>
              <Th>Person</Th>
              <Th>Role</Th>
              <Th>
                <span className="sr-only">Actions</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {items.map((b) => (
              <Row
                key={b.user_id}
                b={b}
                mine={b.user_id === meId}
                lastOwner={b.role === "owner" && !b.implicit && directOwners === 1}
                wsId={wsId}
                canOwn={canOwn}
                candidates={items
                  .filter((x) => x.user_id !== b.user_id)
                  .map((x) => ({ id: x.user_id, label: `${x.display_name} (${x.email})` }))}
                onError={setErr}
              />
            ))}
            {/* (P5-POLISH-7, D6) Access through a group or the organization: a row with the reason, and no menu. */}
            {inherited.map((d) => (
              <tr key={`via-${d.user.id}`} data-testid="inherited-row">
                <Td>
                  <div className="flex items-center gap-3">
                    <Avatar name={d.user.display_name || d.user.email} tintKey={d.user.id} size={32} />
                    <div className="min-w-0">
                      <div className="font-medium">
                        {d.user.display_name}
                        {d.user.id === meId ? <span className="ml-1 font-normal text-muted">(you)</span> : null}
                      </div>
                      <div className="text-xs text-muted">{d.user.email}</div>
                      <div className="text-xs text-ink-2">{d.sources.map((s) => sourceText(s)).join(" · ")}</div>
                    </div>
                  </div>
                </Td>
                <Td>{ROLE_LABEL[d.role as Role] ?? d.role}</Td>
                <Td />
              </tr>
            ))}
          </tbody>
        </DataTable>
      )}
      <details className="text-sm text-ink-2">
        <summary className="cursor-pointer font-medium text-ink">What can each role do?</summary>
        <ul aria-label="Roles" className="mt-2 space-y-0.5">
          {(Object.keys(ROLE_HELP) as Role[]).map((r) => (
            <li key={r}>{ROLE_HELP[r]}</li>
          ))}
        </ul>
      </details>
      {canOwn && people ? <WaitingList wsId={wsId} orgId={orgId} /> : null}
      {canOwn ? <AddBinding wsId={wsId} orgId={orgId} bound={bound} /> : null}
      {/* Groups bind people to a shared workspace; a personal workspace has none. */}
      {personal ? null : <GroupAccess wsId={wsId} orgId={orgId} canOwn={canOwn} data={access.data} />}
    </div>
  );
}
