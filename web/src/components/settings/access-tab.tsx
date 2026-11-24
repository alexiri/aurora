"use client";

import { useState, useSyncExternalStore } from "react";
import { copyLabel, useCopy } from "@/lib/use-copy";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Avatar } from "@/components/agent-avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { ROLE_CAN } from "@/lib/roles-explain";
import { formatWhen } from "@/lib/format-date";
import { When } from "@/components/ui/when";
import { InlineSaved, useSaveBar } from "@/components/ui/save-bar";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { useMe, useOrg, useWorkspace } from "@/lib/api/queries";
import { BulkBar, BulkCheckbox, BulkSelectAll, useBulkOn, useSelection } from "@/components/ui/bulk-bar";
import { useBulk } from "@/lib/bulk";
import { sourceText } from "@/lib/api/groups";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { IdpRoleBadge } from "@/components/identity/idp-role-badge";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { matchesFilter, PickerFilter } from "@/components/ui/picker-filter";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { personName } from "@/lib/workspace-name";
import { WORKSPACE_ROLE_OPTIONS } from "@/lib/labels";
import {
  useAccess,
  useDeleteAccess,
  useOrgMembers,
  usePutAccess,
  useSetAccessStatus,
  type AccessBinding,
} from "@/lib/api/account";
import { OffboardingDialog } from "@/components/automations/offboarding-dialog";
import { WaitingList } from "./add-people";
import { GroupAccess } from "./group-access";
import { useWorkspaceOffboarding } from "@/lib/api/automations";

type Role = "owner" | "approver" | "member";

const ROLE_LABEL: Record<Role, string> = { owner: "Owner", approver: "Approver", member: "Member" };

const ROLE_ORDER: Role[] = ["owner", "approver", "member"];

function CopyLink({ address }: { address: string }) {
  const { state, copy } = useCopy();
  return (
    <Button type="button" variant="ghost" size="sm" onClick={() => copy(address)}>
      {copyLabel(state, "Copy link")}
    </Button>
  );
}

function AddBinding({
  wsId,
  orgId,
  bound,
}: {
  wsId: string;
  orgId: string;
  bound: Set<string>;
}) {
  // The member list is for organization owners and admins (a workspace owner who is only a member gets 403): ask only for them.
  const orgRole = useOrg(orgId).data?.my_role;
  const members = useOrgMembers(orgId, orgRole === "owner" || orgRole === "admin");
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
          <Select id="acc-add-role" value={role} className="w-40 max-w-full" onChange={(v) => setRole(v as Role)} options={WORKSPACE_ROLE_OPTIONS} />
        </Field>
        <Button type="submit" disabled={!user || put.isPending} className="sm:mt-label-offset">
          Add
        </Button>
        <ErrorNotice error={put.error} className="basis-full" />
      </form>
    </Section>
  );
}

/** "Joined 18 Nov" (C-1050); the exact time is the cell's tooltip. */
export function joinedText(at: string): string {
  return `Joined ${formatWhen(at)}`;
}

/** What a suspension stops and what it keeps (C-1051), in the words of the confirmation. */
export const SUSPEND_STOPS =
  "Until you restore them, they can't open this workspace, chat with its agents, use their computers (terminal, SSH or files) or decide its approvals, and what they started here stops now.";
export const SUSPEND_KEEPS = "They keep their role and everything they made here. Their other workspaces aren't affected.";

export function SuspendDialog({
  open,
  who,
  busy,
  error,
  onClose,
  onConfirm,
}: {
  open: boolean;
  who: string;
  busy: boolean;
  error: unknown;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(o) => (o ? undefined : onClose())}
      title={`Suspend ${who} in this workspace?`}
      description={SUSPEND_STOPS}
      confirmLabel="Suspend"
      danger
      busy={busy}
      onConfirm={onConfirm}
    >
      <p className="text-sm text-ink-2">{SUSPEND_KEEPS}</p>
      <ErrorNotice error={error} className="mt-3" />
    </ConfirmDialog>
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
  address,
  protectedRow,
  select,
}: {
  /** (P5-REFINE-19, Expert) The row's bulk checkbox; `disabled` says why it cannot be ticked. */
  select?: { checked: boolean; onChange: (on: boolean) => void; disabled: boolean; title?: string };
  /** The workspace's full address to send, for a person who has not signed in yet. */
  address?: string;
  b: AccessBinding;
  /** This row is the viewer's own: no menu and no role change. */
  mine: boolean;
  /** (FIX-234, D-480) The person of a personal workspace: no role change, no suspend, no remove. */
  protectedRow?: boolean;
  /** The only direct owner: the roles below Owner are not offered. */
  lastOwner: boolean;
  wsId: string;
  canOwn: boolean;
  candidates: { id: string; label: string }[];
  onError: (e: unknown) => void;
}) {
  const put = usePutAccess(wsId);
  const del = useDeleteAccess(wsId);
  const status = useSetAccessStatus(wsId);
  const busy = put.isPending || del.isPending || status.isPending;
  const [confirm, setConfirm] = useState(false);
  const [confirmSuspend, setConfirmSuspend] = useState(false);
  const [promote, setPromote] = useState(false);
  const saved = useSaveBar();
  const offboarding = useWorkspaceOffboarding(wsId, confirm ? b.user_id : null);
  const who = personName(b.display_name);
  const suspended = b.status === "suspended";
  return (
    <tr data-testid={`access-row-${b.user_id}`}>
      <Td cell="title">
        <div className="flex items-center gap-3">
          {select ? <BulkCheckbox name={who} checked={select.checked} disabled={select.disabled} title={select.title} onChange={select.onChange} /> : null}
          <Avatar name={who} tintKey={b.user_id} size={32} />
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1 font-medium">
              <span>
                {personName(b.display_name)}
                {mine ? <span className="ml-1 font-normal text-muted">(you)</span> : null}
              </span>
              {suspended ? <Badge tone="neutral">Suspended</Badge> : null}
            </div>
            <div className="text-xs text-muted">{b.email}</div>
          </div>
        </div>
      </Td>
      <Td label="Role">
        {b.idp_value ? (
          <span className="flex flex-wrap items-center gap-1.5">
            {ROLE_LABEL[b.role as Role] ?? b.role}
            <IdpRoleBadge value={b.idp_value} />
          </span>
        ) : canOwn && !mine && !protectedRow ? (
          <span className="inline-flex items-center gap-2">
          <select
            aria-label={`Role for ${b.email}`}
            className={cn(inputClass, "w-36")}
            value={b.role}
            disabled={busy}
            onChange={(e) => {
              const next = e.target.value as Role;
              // Second rung of the confirm ladder: making someone an owner asks first.
              if (next === "owner" && b.role !== "owner") setPromote(true);
              else put.mutate({ user: b.user_id, role: next }, { onError, onSuccess: () => saved.flash() });
            }}
          >
            {/* The last owner's role cannot be lowered (the server refuses): the option is not offered. */}
            {lastOwner ? null : <option value="member">Member</option>}
            {lastOwner ? null : <option value="approver">Approver</option>}
            <option value="owner">Owner</option>
          </select>
          <InlineSaved show={saved.saved} />
          </span>
        ) : (
          ROLE_LABEL[b.role as Role] ?? b.role
        )}
      </Td>
      <Td label="Joined" className="text-ink-2">
        {/* (workspace-4, C-1086) `signed_in` is false until the person's first sign-in: "Not signed in yet" and the link to send. */}
        {b.signed_in === false ? (
          <span className="flex flex-wrap items-center gap-2" data-testid="not-signed-in">
            <span className="rounded-full bg-pending-panel px-2 py-0.5 text-label font-medium leading-4 text-pending-ink">Not signed in yet</span>
            {b.joined_at ? <span className="whitespace-nowrap">added <When value={b.joined_at} /></span> : null}
            {address ? <CopyLink address={address} /> : null}
          </span>
        ) : b.joined_at ? (
          <When value={b.joined_at} className="whitespace-nowrap" />
        ) : (
          <span className="text-muted">—</span>
        )}
      </Td>
      <Td cell="actions" className="text-right">
        {/* (workspace-5) Only a change to Owner asks: what an Owner can do, then Make Owner. */}
        <Dialog open={promote} onOpenChange={setPromote}>
          <DialogContent title={`Make ${who} an Owner?`}>
            <div className="space-y-3 text-sm text-ink-2">
              <div className="flex items-center gap-3" data-testid="promote-person">
                <Avatar name={who} tintKey={b.user_id} size={32} />
                <span>
                  <strong className="text-ink">{who}</strong>
                  <br />
                  <span className="text-xs text-muted">{ROLE_LABEL[b.role as Role] ?? b.role} now → Owner</span>
                </span>
              </div>
              <p>Owners can, in addition to what members can do:</p>
              <ul className="space-y-1" aria-label="What an Owner can do">
                {ROLE_CAN.workspace.owner.can.map((c) => (
                  <li key={c} className="flex items-center gap-1.5">
                    <span aria-hidden="true" className="text-accent">✓</span> {c}
                  </li>
                ))}
              </ul>
              <p>You can change them back to Member at any time.</p>
            </div>
            <div className="mt-4 flex flex-wrap justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline">Cancel</Button>
              </DialogClose>
              <Button
                disabled={put.isPending}
                onClick={() => {
                  setPromote(false);
                  put.mutate({ user: b.user_id, role: "owner" }, { onError, onSuccess: () => saved.flash() });
                }}
              >
                Make Owner
              </Button>
            </div>
          </DialogContent>
        </Dialog>
        {/* (P5-POLISH-7, D6) A … menu; never on your own row, and not for access that comes from the IdP. */}
        {canOwn && !b.idp_value && !mine && !protectedRow ? (
          <Menu>
            <MenuTrigger asChild>
              <Button variant="outline" size="sm" disabled={busy} aria-label={`More for ${who}`}>
                <span aria-hidden="true">…</span>
              </Button>
            </MenuTrigger>
            <MenuContent align="end">
              {/* (P5-APIGAP-4, C-1051) Suspend asks first and says what stops; Restore gives access back at once. */}
              {suspended ? (
                <MenuItem
                  onSelect={() => status.mutate({ user: b.user_id, status: "active" }, { onError })}
                  aria-label={`Restore ${b.email}`}
                >
                  Restore access
                </MenuItem>
              ) : (
                <MenuItem onSelect={() => setConfirmSuspend(true)} aria-label={`Suspend ${b.email}`}>
                  Suspend…
                </MenuItem>
              )}
              <MenuItem className="text-danger-ink" onSelect={() => setConfirm(true)} aria-label={`Remove ${b.email}`}>
                Remove from workspace…
              </MenuItem>
            </MenuContent>
          </Menu>
        ) : null}
        <SuspendDialog
          open={confirmSuspend}
          who={who}
          busy={status.isPending}
          error={status.error}
          onClose={() => {
            setConfirmSuspend(false);
            status.reset();
          }}
          onConfirm={() =>
            status.mutate({ user: b.user_id, status: "suspended" }, { onSuccess: () => setConfirmSuspend(false) })
          }
        />
        <OffboardingDialog
          open={confirm}
          title="Remove access?"
          description={`${b.email} will lose access to this workspace.`}
          who={personName(b.display_name)}
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
  const origin = useSyncExternalStore(
    () => () => {},
    () => window.location.origin,
    () => "",
  );
  const address = people ? `${origin}${people.workspacePath}` : undefined;
  const [err, setErr] = useState<unknown>(null);
  // (P5-REFINE-19, Expert) Bulk: a checkbox per person an owner may change, a bar with Change role… and Remove….
  const bulk = useBulk();
  const bulkOn = useBulkOn();
  const [bulkDialog, setBulkDialog] = useState<"role" | "remove" | null>(null);
  const [bulkRole, setBulkRole] = useState<Role>("member");
  const bulkPut = usePutAccess(wsId);
  const bulkDel = useDeleteAccess(wsId);
  const wsData = useWorkspace(wsId).data;
  const wsName = wsData?.name;
  const rows = access.data?.items ?? [];
  const soleOwner = rows.filter((b) => b.role === "owner" && b.status !== "suspended").length === 1;
  // (FIX-234, D-480) A personal workspace's person cannot be lowered, suspended or removed
  // there; they leave by deleting the workspace (the API refuses with 409
  // `personal_workspace_person`, and the UI hides the actions outright).
  const personId = wsData?.personal_of?.id ?? null;
  const whyNot = (b: AccessBinding): string | undefined =>
    !canOwn
      ? "Only workspace owners can change who has access"
      : b.user_id === meId
        ? "You cannot change yourself"
        : b.user_id === personId
          ? "The person of a personal workspace cannot be changed here"
          : b.idp_value
            ? "Managed by your identity provider"
            : b.role === "owner" && soleOwner
              ? "The last owner stays an owner"
              : undefined;
  const sel = useSelection(rows.filter((b) => whyNot(b) === undefined).map((b) => b.user_id));
  if (access.isPending) return <Loading />;
  if (access.isError) return <ErrorState error={access.error} onRetry={() => access.refetch()} />;
  // (P5-POLISH-11, D6) You first, then by name.
  const items = [...access.data.items].sort(
    (a, b) =>
      Number(b.user_id === meId) - Number(a.user_id === meId) ||
      personName(a.display_name).localeCompare(personName(b.display_name)),
  );
  const bound = new Set(items.map((b) => b.user_id));
  const inherited = (access.data.derived_members?.items ?? []).filter((d) => !bound.has(d.user.id));
  const directOwners = items.filter((b) => b.role === "owner" && b.status !== "suspended").length;
  const picked = items.filter((b) => sel.selected.has(b.user_id));
  const targets = picked.map((b) => ({ id: b.user_id, name: personName(b.display_name) }));
  const n = picked.length;
  const count = `${n} ${n === 1 ? "person" : "people"}`;
  const workspace = wsName ?? "this workspace";
  return (
    <div className="space-y-6">
      {!canOwn ? <Notice role="status">Only workspace owners can change who has access.</Notice> : null}
      <ErrorNotice error={err} />
      {canOwn ? (
        <BulkBar count={n} noun={n === 1 ? "person selected" : "people selected"} bulk={bulk} onClear={sel.clear}>
          <Button type="button" size="sm" variant="outline" disabled={bulk.running || n === 0} onClick={() => { setBulkRole("member"); setBulkDialog("role"); }}>
            Change role…
          </Button>
          <Button type="button" size="sm" variant="outline" disabled={bulk.running || n === 0} onClick={() => setBulkDialog("remove")}>
            Remove…
          </Button>
        </BulkBar>
      ) : null}
      {/* (P5-REFINE-19) The second rung, once for all of them: the consequence and the count. The calls are the per-person routes. */}
      <ConfirmDialog
        open={bulkDialog === "role"}
        onOpenChange={(o) => !o && setBulkDialog(null)}
        title={`Change the role of ${count} in ${workspace}?`}
        description={`${count} will become ${ROLE_LABEL[bulkRole]}.`}
        confirmLabel={`Change role of ${count}`}
        onConfirm={() => {
          const role = bulkRole;
          setBulkDialog(null);
          setErr(null);
          void bulk.run(targets, (t) => bulkPut.mutateAsync({ user: t.id, role }));
        }}
      >
        <div className="space-y-3 text-sm">
          <Field id="bulk-role" label="New role">
            <Select id="bulk-role" value={bulkRole} onChange={(v) => setBulkRole(v as Role)} options={ROLE_ORDER.map((r) => ({ value: r, label: ROLE_LABEL[r] }))} />
          </Field>
          <ul aria-label={`What ${ROLE_LABEL[bulkRole]}s can do`} className="space-y-1">
            {ROLE_CAN.workspace[bulkRole].can.map((c) => (
              <li key={c} className="text-ink">
                {c}
              </li>
            ))}
          </ul>
        </div>
      </ConfirmDialog>
      <ConfirmDialog
        open={bulkDialog === "remove"}
        onOpenChange={(o) => !o && setBulkDialog(null)}
        title={`Remove ${count} from ${workspace}?`}
        description="They lose access now."
        confirmLabel={`Remove ${count}`}
        danger
        onConfirm={() => {
          setBulkDialog(null);
          setErr(null);
          void bulk.run(targets, (t) => bulkDel.mutateAsync({ user: t.id }));
        }}
      />
      {items.length === 0 && inherited.length === 0 ? (
        <EmptyState title="No one has access yet" />
      ) : (
        <DataTable caption="People with access to this workspace" stackBelow={900}>
          <thead>
            <tr>
              <Th>
                {bulkOn && canOwn ? <BulkSelectAll label="Select all people" all={sel.all} some={sel.some} disabled={bulk.running} onChange={sel.setAll} /> : null}
                Person
              </Th>
              <Th>Role</Th>
              <Th>Joined</Th>
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
                protectedRow={b.user_id === personId}
                lastOwner={b.role === "owner" && directOwners === 1}
                address={address}
                select={canOwn ? { checked: sel.selected.has(b.user_id), onChange: (on) => sel.toggle(b.user_id, on), disabled: whyNot(b) !== undefined || bulk.running, title: whyNot(b) } : undefined}
                wsId={wsId}
                canOwn={canOwn}
                candidates={items
                  .filter((x) => x.user_id !== b.user_id)
                  .map((x) => ({ id: x.user_id, label: `${personName(x.display_name)} (${x.email})` }))}
                onError={setErr}
              />
            ))}
            {/* (P5-POLISH-7, D6) Access through a group or the organization: a row with the reason, and no menu. */}
            {inherited.map((d) => (
              <tr key={`via-${d.user.id}`} data-testid="inherited-row">
                <Td cell="title">
                  <div className="flex items-center gap-3">
                    <Avatar name={personName(d.user.display_name)} tintKey={d.user.id} size={32} />
                    <div className="min-w-0">
                      <div className="font-medium">
                        {personName(d.user.display_name)}
                        {d.user.id === meId ? <span className="ml-1 font-normal text-muted">(you)</span> : null}
                      </div>
                      <div className="text-xs text-muted">{d.user.email}</div>
                      <div className="text-xs text-ink-2">{d.sources.map((s) => sourceText(s)).join(" · ")}</div>
                    </div>
                  </div>
                </Td>
                <Td label="Role">{ROLE_LABEL[d.role as Role] ?? d.role}</Td>
                <Td label="Joined" className="whitespace-nowrap text-ink-2">
                  {d.joined_at ? <When value={d.joined_at} /> : <span className="text-muted">—</span>}
                </Td>
                <Td />
              </tr>
            ))}
          </tbody>
        </DataTable>
      )}
      <details className="text-sm text-ink-2">
        <summary className="cursor-pointer font-medium text-ink">What can each role do?</summary>
        <ul aria-label="Roles" className="mt-2 space-y-0.5">
          {ROLE_ORDER.map((r) => (
            <li key={r}>
              <strong className="text-ink">{ROLE_CAN.workspace[r].label}:</strong> {ROLE_CAN.workspace[r].summary} {ROLE_CAN.workspace[r].can.join(" · ")}.
            </li>
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
