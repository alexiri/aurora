"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { cn } from "@/lib/utils";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useCreateInvitation, validateInvite } from "@/lib/api/org";
import { useWorkspaces } from "@/lib/api/queries";

export function InviteForm({ orgId, onDone }: { orgId: string; onDone: () => void }) {
  const create = useCreateInvitation(orgId);
  const workspaces = useWorkspaces(orgId);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<"admin" | "member">("member");
  const [wsIds, setWsIds] = useState<string[]>([]);
  const [approverIds, setApproverIds] = useState<string[]>([]);
  const [clientErr, setClientErr] = useState("");
  const fe = create.error ? fieldErrors(create.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    const msg = validateInvite({ email, role });
    if (msg) return setClientErr(msg);
    setClientErr("");
    create.mutate(
      { email: email.trim(), role, workspace_ids: wsIds, approver_workspace_ids: approverIds },
      { onSuccess: onDone },
    );
  }

  return (
    <form onSubmit={submit} noValidate className="space-y-4">
      <Field id="inv-email" label="Email" error={clientErr || fe.email}>
        <input
          id="inv-email"
          type="email"
          className={inputClass}
          value={email}
          autoComplete="off"
          onChange={(e) => setEmail(e.target.value)}
        />
      </Field>
      <Field id="inv-role" label="Role" error={fe.role}>
        <select
          id="inv-role"
          className={inputClass}
          value={role}
          onChange={(e) => setRole(e.target.value as "admin" | "member")}
        >
          <option value="member">member</option>
          <option value="admin">admin</option>
        </select>
      </Field>
      {workspaces.data && workspaces.data.items.length > 0 ? (
        <fieldset className="space-y-2">
          <legend className="text-sm font-medium text-ink">Workspace access (optional)</legend>
          <p className="text-xs text-muted">
            Member chats with agents. Approver also decides approvals.
          </p>
          {workspaces.data.items.map((w) => {
            const level = approverIds.includes(w.id) ? "approver" : wsIds.includes(w.id) ? "member" : "none";
            return (
              <div key={w.id} className="flex items-center justify-between gap-3 text-sm">
                <label htmlFor={`inv-ws-${w.id}`} className="min-w-0 truncate text-ink">
                  {w.name}
                </label>
                <select
                  id={`inv-ws-${w.id}`}
                  className={cn(inputClass, "w-40 shrink-0")}
                  value={level}
                  onChange={(e) => {
                    const v = e.target.value;
                    setWsIds((cur) => cur.filter((x) => x !== w.id).concat(v === "member" ? [w.id] : []));
                    setApproverIds((cur) => cur.filter((x) => x !== w.id).concat(v === "approver" ? [w.id] : []));
                  }}
                >
                  <option value="none">No access</option>
                  <option value="member">Member</option>
                  <option value="approver">Approver</option>
                </select>
              </div>
            );
          })}
        </fieldset>
      ) : null}
      {create.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {errorMessage(create.error)}
        </Notice>
      ) : null}
      <div className="flex justify-end">
        <Button type="submit" size="lg" disabled={create.isPending}>
          {create.isPending ? "Inviting…" : "Send invitation"}
        </Button>
      </div>
    </form>
  );
}

export function InviteDialog({ orgId }: { orgId: string }) {
  const [open, setOpen] = useState(false);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button>Invite member</Button>
      </DialogTrigger>
      <DialogContent title="Invite a member" description="They will receive a link to join this organization.">
        <InviteForm orgId={orgId} onDone={() => setOpen(false)} />
      </DialogContent>
    </Dialog>
  );
}
