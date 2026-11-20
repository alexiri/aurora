"use client";

import { useState, type FormEvent } from "react";

import { copyToClipboard } from "@/components/agents/terminal-clipboard";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice } from "@/components/ui/notice";
import { Select } from "@/components/ui/select";
import { fieldErrors } from "@/lib/api/errors";
import { useCreateInvitation, validateInvite } from "@/lib/api/org";
import { useOrg, useWorkspaces } from "@/lib/api/queries";

import { OrgRoleHelp } from "./role-help";

/** "dana@example.test" → "Dana"; nothing typed yet → "They". */
function nameFromEmail(email: string): string {
  const local = email.split("@")[0]?.trim() ?? "";
  return email.includes("@") && local ? `${local.charAt(0).toUpperCase()}${local.slice(1)}` : "They";
}

export function InviteForm({ orgId, onDone, onCancel }: { orgId: string; onDone: (created: { email: string; link?: string | null }) => void; onCancel?: () => void }) {
  const create = useCreateInvitation(orgId);
  const workspaces = useWorkspaces(orgId);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<"admin" | "member">("member");
  const [wsIds, setWsIds] = useState<string[]>([]);
  const [approverIds, setApproverIds] = useState<string[]>([]);
  const [clientErr, setClientErr] = useState("");
  const fe = create.error ? fieldErrors(create.error) : {};
  // (org-11) Only team workspaces: everybody's personal workspace is made for them on accept.
  const teamWorkspaces = (workspaces.data?.items ?? []).filter((w) => !w.personal_of);

  function submit(e: FormEvent) {
    e.preventDefault();
    const msg = validateInvite({ email, role });
    if (msg) return setClientErr(msg);
    setClientErr("");
    create.mutate(
      { email: email.trim(), role, workspace_ids: wsIds, approver_workspace_ids: approverIds },
      { onSuccess: (inv) => onDone({ email: inv.email, link: inv.link }) },
    );
  }

  return (
    <form onSubmit={submit} noValidate className="space-y-4">
      <Field id="inv-email" label="E-mail" error={clientErr || fe.email}>
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
        <Select id="inv-role" value={role} onChange={(v) => setRole(v as "admin" | "member")} options={[{ value: "member", label: "Member" }, { value: "admin", label: "Admin" }]} />
      </Field>
      <OrgRoleHelp />
      {teamWorkspaces.length > 0 ? (
        <fieldset className="space-y-2">
          <legend className="text-sm font-medium text-ink">Also add to team workspaces</legend>
          <p className="text-xs text-muted">
            Member chats with agents. Approver also decides approvals.
          </p>
          {teamWorkspaces.map((w) => {
            const level = approverIds.includes(w.id) ? "approver" : wsIds.includes(w.id) ? "member" : "none";
            return (
              <div key={w.id} className="flex items-center justify-between gap-3 text-sm">
                <label htmlFor={`inv-ws-${w.id}`} className="min-w-0 truncate text-ink">
                  {w.name}
                </label>
                <Select id={`inv-ws-${w.id}`} value={level} className="w-40 shrink-0" onChange={(v) => {
                    setWsIds((cur) => cur.filter((x) => x !== w.id).concat(v === "member" ? [w.id] : []));
                    setApproverIds((cur) => cur.filter((x) => x !== w.id).concat(v === "approver" ? [w.id] : []));
                  }} options={[{ value: "none", label: "No access" }, { value: "member", label: "Member" }, { value: "approver", label: "Approver" }]} />
              </div>
            );
          })}
        </fieldset>
      ) : null}
      <p className="text-sm text-muted">{nameFromEmail(email)} gets their own personal workspace automatically.</p>
      {create.error && Object.keys(fe).length === 0 ? (
        <ErrorNotice error={create.error} />
      ) : null}
      <div className="flex flex-wrap justify-end gap-2">
        {onCancel ? (
          <Button type="button" variant="outline" size="lg" onClick={onCancel}>
            Cancel
          </Button>
        ) : null}
        <Button type="submit" size="lg" disabled={create.isPending}>
          {create.isPending ? "Creating…" : "Create invitation"}
        </Button>
      </div>
    </form>
  );
}

export function InviteDialog({ orgId }: { orgId: string }) {
  const [open, setOpen] = useState(false);
  const [created, setCreated] = useState<{ email: string; link?: string | null } | null>(null);
  const [copied, setCopied] = useState(false);
  const orgName = useOrg(orgId).data?.name;
  return (
    <Dialog open={open} onOpenChange={(o) => { setOpen(o); if (!o) { setCreated(null); setCopied(false); } }}>
      <DialogTrigger asChild>
        <Button>Invite member</Button>
      </DialogTrigger>
      <DialogContent title={`Invite someone to ${orgName ?? "the organization"}`} description="They get a link to join this organization.">
        {created ? (
          <div className="space-y-4" data-testid="invite-created">
            <p className="text-sm text-ink">Invitation created for {created.email}.</p>
            {created.link ? (
              <Button
                variant="outline"
                onClick={async () => setCopied(await copyToClipboard(created.link!))}
              >
                Copy invitation link
              </Button>
            ) : null}
            {copied ? <p role="status" className="text-sm text-ink-2">Invitation link copied. Send it to {nameFromEmail(created.email)} yourself.</p> : null}
            <div className="flex justify-end">
              <Button onClick={() => setOpen(false)}>Done</Button>
            </div>
          </div>
        ) : (
          <InviteForm orgId={orgId} onDone={(c) => setCreated(c)} onCancel={() => setOpen(false)} />
        )}
      </DialogContent>
    </Dialog>
  );
}
