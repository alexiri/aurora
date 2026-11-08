"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { cn } from "@/lib/utils";
import {
  useAccess,
  useDeleteAccess,
  useOrgMembers,
  usePutAccess,
  type AccessBinding,
} from "@/lib/api/account";
import { OffboardingDialog } from "@/components/automations/offboarding-dialog";
import { useWorkspaceOffboarding } from "@/lib/api/automations";
import { errorMessage } from "@/lib/api/errors";

type Role = "owner" | "approver" | "member";

/** One-line explanation of each role (phase 2, RFE-012). */
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

  // Plain org members cannot list members (403): hide the picker entirely.
  if (members.isPending || members.isError) return null;
  const candidates = members.data.items.filter((m) => !bound.has(m.user_id));
  if (candidates.length === 0) {
    return <p className="text-sm text-ink-2">Every active org member already has access.</p>;
  }
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
          <select
            id="acc-add-role"
            className={cn(inputClass, "w-40 max-w-full")}
            value={role}
            onChange={(e) => setRole(e.target.value as Role)}
          >
            <option value="member">member</option>
            <option value="approver">approver</option>
            <option value="owner">owner</option>
          </select>
        </Field>
        <Button type="submit" disabled={!user || put.isPending} className="sm:mt-[1.625rem]">
          Add
        </Button>
        {put.error ? (
          <Notice tone="danger" role="alert" className="basis-full">
            {errorMessage(put.error)}
          </Notice>
        ) : null}
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
}: {
  b: AccessBinding;
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
        <div className="font-medium">{b.display_name}</div>
        <div className="text-xs text-muted">{b.email}</div>
      </Td>
      <Td>
        {b.implicit ? (
          <span>
            owner{" "}
            <Badge title="Organization owner or admin" className="normal-case tracking-normal">
              Implicit (org)
            </Badge>
          </span>
        ) : canOwn ? (
          <select
            aria-label={`Role for ${b.email}`}
            className={cn(inputClass, "w-36")}
            value={b.role}
            disabled={busy}
            onChange={(e) =>
              put.mutate({ user: b.user_id, role: e.target.value as Role }, { onError })
            }
          >
            <option value="member">member</option>
            <option value="approver">approver</option>
            <option value="owner">owner</option>
          </select>
        ) : (
          b.role
        )}
      </Td>
      <Td className="text-right">
        {canOwn && !b.implicit ? (
          <Button
            variant="outline"
            size="sm"
            disabled={busy}
            aria-label={`Remove ${b.email}`}
            onClick={() => setConfirm(true)}
          >
            Remove
          </Button>
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
}: {
  wsId: string;
  orgId: string;
  canOwn: boolean;
}) {
  const access = useAccess(wsId);
  const [err, setErr] = useState<unknown>(null);
  if (access.isPending) return <Loading />;
  if (access.isError) return <ErrorState error={access.error} onRetry={() => access.refetch()} />;
  const items = access.data.items;
  const bound = new Set(items.map((b) => b.user_id));
  return (
    <div className="space-y-6">
      {!canOwn ? <Notice role="status">Only workspace owners can change who has access.</Notice> : null}
      <ul aria-label="Roles" className="space-y-0.5 text-sm text-ink-2">
        {(Object.keys(ROLE_HELP) as Role[]).map((r) => (
          <li key={r}>{ROLE_HELP[r]}</li>
        ))}
      </ul>
      {err ? (
        <Notice tone="danger" role="alert">
          {errorMessage(err)}
        </Notice>
      ) : null}
      {items.length === 0 ? (
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
                wsId={wsId}
                canOwn={canOwn}
                candidates={items
                  .filter((x) => x.user_id !== b.user_id)
                  .map((x) => ({ id: x.user_id, label: `${x.display_name} (${x.email})` }))}
                onError={setErr}
              />
            ))}
          </tbody>
        </DataTable>
      )}
      {canOwn ? <AddBinding wsId={wsId} orgId={orgId} bound={bound} /> : null}
    </div>
  );
}
