"use client";

import { useState, useSyncExternalStore, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { CopyValue } from "@/components/ui/copy-value";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { Segmented } from "@/components/ui/segmented";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { useOrg } from "@/lib/api/queries";
import {
  useAddByEmail,
  usePendingAccess,
  useRemovePending,
  type AddByEmailResponse,
} from "@/lib/api/pending-access";
import { ROLE_CAN } from "@/lib/roles-explain";
import { copyLabel, useCopy } from "@/lib/use-copy";
import { askSentence, whoCanFromContacts } from "@/lib/who-can";

type Role = "owner" | "approver" | "member";
const ROLE_LABEL: Record<Role, string> = { member: "Member", approver: "Approver", owner: "Owner" };

/** The people waiting for their first sign-in per workspace at most (`PENDING_ACCESS_MAX`, C-920). */
const WAITING_MAX = 50;

/** What a refusal of the add says, in words (`14 §10`). */
export function addError(err: unknown, orgName: string): string {
  if (err instanceof ApiError) {
    if (err.code === "pending_access_limit") return `${WAITING_MAX} people are already waiting. Remove one first.`;
    if (err.code === "owner_requires_member") {
      return `Only someone who is already in ${orgName} can be an owner. Choose Member, and make them an owner after they have signed in.`;
    }
  }
  return errorMessage(err);
}

/** How many whole days until `iso` (a started day counts), never below 0. */
export function daysLeft(iso: string, now: number = Date.now()): number {
  return Math.max(0, Math.ceil((Date.parse(iso) - now) / 86_400_000));
}

/** The one sentence of the result (`14 §10`). */
export function resultSentence(email: string, role: Role, r: Pick<AddByEmailResponse, "result">): string {
  return r.result === "added"
    ? `${email} can now use this workspace.`
    : `${email} will have access as ${ROLE_LABEL[role]} after they first sign in.`;
}

/**
 * **Add people** (P5-UX-5, C-919, `14 §10`): one address and a role. Someone already in the organization gets access at
 * once; anyone else waits for their first sign-in. Aurora sends no e-mail, so the result ends with the link to send.
 * The dialog never lists or searches the organization's people (SEC-128).
 */
export function AddPeople({
  wsId,
  orgId,
  workspacePath,
  hasApprover,
  initiallyOpen = false,
}: {
  wsId: string;
  orgId: string;
  /** `/o/<org>/w/<ws>`: the address to send. */
  workspacePath: string;
  /** Approver is offered only in a workspace that already has one (D-352). */
  hasApprover: boolean;
  /** Opened by `?add=1` (Welcome's link). */
  initiallyOpen?: boolean;
}) {
  const [open, setOpen] = useState(initiallyOpen);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<Role>("member");
  const [done, setDone] = useState<{ email: string; role: Role; response: AddByEmailResponse } | null>(null);
  // The address bar's origin, known only in the browser ("" while rendering on the server).
  const origin = useSyncExternalStore(
    () => () => {},
    () => window.location.origin,
    () => "",
  );
  const add = useAddByEmail(wsId);
  const org = useOrg(orgId);
  const orgName = org.data?.name ?? "the organization";

  const options: { value: Role; label: string }[] = [
    { value: "member", label: "Member" },
    ...(hasApprover ? [{ value: "approver" as const, label: "Approver" }] : []),
    { value: "owner", label: "Owner" },
  ];
  const roleHelp =
    role === "owner"
      ? "Owners also give access and add people. Only for someone who is already in the organization."
      : role === "approver"
        ? "Approvers chat with agents and decide approvals."
        : "Members chat with agents and see what they can use.";

  const reset = () => {
    setEmail("");
    setRole("member");
    setDone(null);
    add.reset();
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const trimmed = email.trim();
    if (!trimmed) return;
    add.mutate({ email: trimmed, role }, { onSuccess: (response) => setDone({ email: trimmed, role, response }) });
  };

  return (
    <>
      <Button onClick={() => setOpen(true)}>Add people</Button>
      <Dialog
        open={open}
        onOpenChange={(o) => {
          setOpen(o);
          if (!o) reset();
        }}
      >
        <DialogContent title="Add people" description="Give one person access to this workspace." size="md">
          {done ? (
            <div className="space-y-3 text-sm text-ink-2">
              <p role="status" className="font-medium text-ink">
                {resultSentence(done.email, done.role, done.response)}
              </p>
              {done.response.org_entry === "invitation_needed" ? <InvitationNeeded orgId={orgId} orgName={orgName} /> : null}
              <p>Aurora does not e-mail them. Send them this link:</p>
              <CopyValue label="Workspace address" value={`${origin}${workspacePath}`} />
              <div className="flex flex-wrap justify-end gap-2 pt-1">
                <Button variant="outline" onClick={reset}>
                  Add another
                </Button>
                <Button
                  onClick={() => {
                    setOpen(false);
                    reset();
                  }}
                >
                  Done
                </Button>
              </div>
            </div>
          ) : (
            <form aria-label="Add people" onSubmit={submit} className="space-y-4" noValidate>
              <Field id="add-people-email" label="Who">
                <input
                  id="add-people-email"
                  type="email"
                  autoComplete="off"
                  className={inputClass}
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  placeholder="name@example.org"
                />
              </Field>
              <HowTheyOpenIt who={email.trim()} address={`${origin}${workspacePath}`} />
              <div className="space-y-1.5">
                <span className="block text-sm font-medium text-ink">
                  Role
                </span>
                <Segmented label="Role" value={role} options={options} onChange={setRole} />
                <p className="text-sm text-ink-2">{roleHelp}</p>
              </div>
              {/* (rec 16) Open here: the first owner never learns Approver exists otherwise. */}
              <details open className="text-sm text-ink-2" data-testid="role-fold">
                <summary className="cursor-pointer font-medium text-ink">What can each role do?</summary>
                <ul aria-label="Roles" className="mt-2 space-y-0.5">
                  {(["owner", "approver", "member"] as const).map((r) => (
                    <li key={r}>
                      <strong className="text-ink">{ROLE_CAN.workspace[r].label}:</strong> {ROLE_CAN.workspace[r].summary}
                    </li>
                  ))}
                </ul>
              </details>
              {add.error ? (
                <Notice tone="danger" role="alert">
                  {addError(add.error, orgName)}
                </Notice>
              ) : null}
              <div className="flex justify-end gap-2">
                <Button type="button" variant="outline" onClick={() => setOpen(false)}>
                  Cancel
                </Button>
                <Button type="submit" disabled={!email.trim() || add.isPending}>
                  {add.isPending ? "Adding…" : "Add"}
                </Button>
              </div>
            </form>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}

/** (workspace-4) Before the person submits: Aurora sends nothing, so say how they open the workspace, with the link to copy. */
function HowTheyOpenIt({ who, address }: { who: string; address: string }) {
  const { state, copy } = useCopy();
  return (
    <Notice tone="neutral" className="flex flex-wrap items-start gap-x-3 gap-y-2" data-testid="how-they-open">
      <p className="min-w-0 flex-1 basis-56 text-sm">
        <strong>Aurora doesn&apos;t send an e-mail.</strong> {who || "The person"} can open this workspace at{" "}
        <span className="break-all font-mono text-xs">{address.replace(/^https?:\/\//, "")}</span> once they sign in.
      </p>
      <Button type="button" variant="outline" size="sm" onClick={() => copy(address)}>
        {copyLabel(state, "Copy link")}
      </Button>
    </Notice>
  );
}

/** "They also need to be invited to <org>. Ask an organization admin." */
function InvitationNeeded({ orgId, orgName }: { orgId: string; orgName: string }) {
  const org = useOrg(orgId);
  const who = whoCanFromContacts("org_admin", org.data?.contacts, "invite_people");
  return (
    <p>
      They also need to be invited to {orgName}. {askSentence(who)}
    </p>
  );
}

/** "Waiting for first sign-in": the people added by e-mail who have not signed in yet. Not shown when empty. */
export function WaitingList({ wsId, orgId }: { wsId: string; orgId: string }) {
  const pending = usePendingAccess(wsId);
  const remove = useRemovePending(wsId);
  const org = useOrg(orgId);
  const items = pending.data?.items ?? [];
  if (items.length === 0) return null;
  return (
    <Section
      id="acc-waiting"
      title="Waiting for first sign-in"
      description={`Access starts when they first sign in to ${org.data?.name ?? "the organization"} with this verified e-mail.`}
    >
      <ul aria-label="Waiting for first sign-in" className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface shadow-card">
        {items.map((p) => {
          const n = daysLeft(p.expires_at);
          return (
            <li key={p.id} className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 px-4 py-3 text-sm">
              <span className="min-w-0 break-words text-ink">
                {p.email} · {ROLE_LABEL[p.role]} · expires in {n} {n === 1 ? "day" : "days"}
              </span>
              <Button variant="outline" size="sm" aria-label={`Remove ${p.email}`} disabled={remove.isPending} onClick={() => remove.mutate(p.id)}>
                Remove
              </Button>
            </li>
          );
        })}
      </ul>
      <ErrorNotice error={remove.error} className="mt-2" />
    </Section>
  );
}
