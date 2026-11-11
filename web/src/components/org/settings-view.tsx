"use client";

import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { OrgFreeze } from "@/components/freeze/freeze-sections";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { DangerZone, Section } from "@/components/ui/section";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { deleteOrg, usePatchOrg, usePutRetention, useRetention } from "@/lib/api/org";
import { useInstance } from "@/lib/api/queries";
import type { Org } from "@/lib/api/queries";

function parseLimit(v: string): number | null | "invalid" {
  const t = v.trim();
  if (t === "") return null;
  if (!/^\d+$/.test(t)) return "invalid";
  const n = Number(t);
  return n >= 1 ? n : "invalid";
}

function GeneralForm({ org, isOwner }: { org: Org; isOwner: boolean }) {
  const router = useRouter();
  const patch = usePatchOrg(org.id);
  const [name, setName] = useState(org.name);
  const [slug, setSlug] = useState(org.slug);
  const [canCreate, setCanCreate] = useState(org.settings.members_can_create_workspaces);
  const [maxMembers, setMaxMembers] = useState(org.settings.max_members?.toString() ?? "");
  const [maxAgents, setMaxAgents] = useState(org.settings.max_agents?.toString() ?? "");
  const [clientErr, setClientErr] = useState<Record<string, string>>({});
  const [saved, setSaved] = useState(false);
  const fe = patch.error ? fieldErrors(patch.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const errs: Record<string, string> = {};
    const n = name.trim();
    const s = slug.trim();
    if (!n || n.length > 200) errs.name = "Name must be 1 to 200 characters.";
    if (!s) errs.slug = "URL name is required.";
    const mm = parseLimit(maxMembers);
    const ma = parseLimit(maxAgents);
    if (mm === "invalid") errs.max_members = "Enter a whole number of 1 or more, or leave empty.";
    if (ma === "invalid") errs.max_agents = "Enter a whole number of 1 or more, or leave empty.";
    setClientErr(errs);
    if (Object.keys(errs).length > 0) return;
    patch.mutate(
      {
        name: n,
        slug: s,
        settings: {
          members_can_create_workspaces: canCreate,
          max_members: mm as number | null,
          max_agents: ma as number | null,
        },
      },
      {
        onSuccess: (updated) => {
          setSaved(true);
          if (updated.slug !== org.slug) router.replace(`/o/${updated.slug}/settings`);
        },
      },
    );
  }

  return (
    <Form onSubmit={submit} noValidate className="max-w-md">
      {!isOwner ? <Notice role="status">Only organization owners can change these settings.</Notice> : null}
      <fieldset disabled={!isOwner || patch.isPending} className="space-y-4">
        <Field id="org-name" label="Name" error={clientErr.name || fe.name}>
          <input
            id="org-name"
            className={inputClass}
            value={name}
            maxLength={200}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
        <Field id="org-slug" label="URL name" error={clientErr.slug || fe.slug}>
          <input
            id="org-slug"
            className={inputClass}
            value={slug}
            onChange={(e) => setSlug(e.target.value)}
          />
        </Field>
        <CheckboxField
          id="org-can-create"
          label="Members can create workspaces"
          checked={canCreate}
          onChange={setCanCreate}
        />
        <Field
          id="org-max-members"
          label="Member limit"
          hint="Leave empty for no limit."
          error={clientErr.max_members || fe["settings/max_members"] || fe.max_members}
        >
          <input
            id="org-max-members"
            inputMode="numeric"
            className={inputClass}
            value={maxMembers}
            onChange={(e) => setMaxMembers(e.target.value)}
          />
        </Field>
        <Field
          id="org-max-agents"
          label="Agent limit"
          hint="Leave empty for no limit."
          error={clientErr.max_agents || fe["settings/max_agents"] || fe.max_agents}
        >
          <input
            id="org-max-agents"
            inputMode="numeric"
            className={inputClass}
            value={maxAgents}
            onChange={(e) => setMaxAgents(e.target.value)}
          />
        </Field>
      </fieldset>
      {patch.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {errorMessage(patch.error)}
        </Notice>
      ) : null}
      {saved ? (
        <Notice tone="accent" role="status">
          Settings saved.
        </Notice>
      ) : null}
      {isOwner ? (
        <FormActions>
          <Button type="submit" size="lg" disabled={patch.isPending}>
            {patch.isPending ? "Saving…" : "Save changes"}
          </Button>
        </FormActions>
      ) : null}
    </Form>
  );
}

/** The org's Port previews switch (P4-PRV-1, C-549, docs/14). The card exists only when the
 * instance runs previews; the switch is owners/admins and audited. Turning it off asks for
 * confirmation: existing links stop working until it is turned back on. */
function PreviewsForm({ org }: { org: Org }) {
  const patch = usePatchOrg(org.id);
  const instance = useInstance();
  const [confirmOff, setConfirmOff] = useState(false);
  const on = org.settings.previews_enabled;
  const domain = instance.data?.previews?.domain ?? "";
  const enabled = instance.data?.previews?.enabled === true;
  if (!enabled) return null;
  return (
    <Section
      id="org-previews"
      title="Port previews"
      description="Lets people who can use a workspace open web apps that its agents run, at https://<random>.<previews domain>. Only signed-in people who can use that workspace can open them, and agents cannot create them."
      card
    >
      <CheckboxField
        id="org-previews-on"
        label="Allow port previews in this organization"
        checked={on}
        disabled={!isOwnerAdmin(org) || patch.isPending}
        onChange={(v) => {
          if (!v) {
            setConfirmOff(true);
            return;
          }
          patch.mutate({ settings: { previews_enabled: true } });
        }}
      />
      {domain ? (
        <Field id="org-previews-domain" label="Preview domain">
          <input id="org-previews-domain" className={inputClass} value={domain} readOnly aria-readonly="true" />
        </Field>
      ) : null}
      <ConfirmDialog
        open={confirmOff}
        onOpenChange={setConfirmOff}
        title="Turn port previews off?"
        description="Existing links stop working until you turn it back on. The shares are kept."
        confirmLabel="Turn off"
        danger
        busy={patch.isPending}
        onConfirm={() => {
          patch.mutate({ settings: { previews_enabled: false } }, { onSettled: () => setConfirmOff(false) });
        }}
      />
      {patch.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(patch.error)}
        </Notice>
      ) : null}
    </Section>
  );
}

function isOwnerAdmin(org: Org): boolean {
  return org.my_role === "owner" || org.my_role === "admin";
}

/** Conversation retention (RFE-010): a window of 7 to 3650 days, or none; owners and admins. */
function RetentionForm({ orgId }: { orgId: string }) {
  const current = useRetention(orgId);
  const put = usePutRetention(orgId);
  const [days, setDays] = useState<string | null>(null);
  const [clientErr, setClientErr] = useState<string | undefined>();
  const [saved, setSaved] = useState(false);
  if (current.isPending) return null;
  const max = current.data?.conversations_days_max ?? null;
  const value = days ?? current.data?.conversations_days?.toString() ?? "";

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const t = value.trim();
    let n: number | null = null;
    if (t !== "") {
      n = /^\d+$/.test(t) ? Number(t) : NaN;
      if (!(n >= 7 && n <= 3650)) return setClientErr("Enter a whole number of days from 7 to 3650, or leave empty to keep conversations.");
      if (max !== null && n > max) return setClientErr(`Your instance allows at most ${max} days.`);
    }
    setClientErr(undefined);
    put.mutate(n, { onSuccess: () => setSaved(true) });
  }

  const serverErr = put.error ? errorMessage(put.error) : undefined;
  return (
    <Form onSubmit={submit} noValidate aria-label="Conversation retention" className="max-w-md">
      <Field
        id="retention-days"
        label="Days of inactivity"
        hint={max !== null ? `Between 7 and ${max} days.` : "Between 7 and 3650 days."}
        error={clientErr ?? serverErr}
      >
        <input
          id="retention-days"
          inputMode="numeric"
          className={inputClass}
          value={value}
          onChange={(e) => {
            setDays(e.target.value);
            setSaved(false);
          }}
        />
      </Field>
      {saved ? (
        <Notice tone="accent" role="status">
          Retention saved.
        </Notice>
      ) : null}
      <FormActions>
        <Button type="submit" size="lg" disabled={put.isPending}>
          {put.isPending ? "Saving…" : "Save retention"}
        </Button>
      </FormActions>
    </Form>
  );
}

export function OrgDangerZone({
  orgId,
  slug,
  onDeleted,
}: {
  orgId: string;
  slug: string;
  onDeleted: () => void;
}) {
  const [confirm, setConfirm] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (confirm !== slug) return;
    setPending(true);
    setError(null);
    try {
      await deleteOrg(orgId, confirm);
      onDeleted();
    } catch (err) {
      setError(err);
      setPending(false);
    }
  }

  return (
    <DangerZone
      id="delete-org"
      title="Delete this organization"
      description={
        <>
          This permanently deletes the organization, its workspaces and everything in them. Type{" "}
          <code className="font-mono text-ink">{slug}</code> to confirm.
        </>
      }
    >
    <Form onSubmit={(e) => void submit(e)} className="max-w-md">
      <Field id="del-org" label="Organization URL name">
        <input
          id="del-org"
          className={inputClass}
          value={confirm}
          autoComplete="off"
          onChange={(e) => setConfirm(e.target.value)}
        />
      </Field>
      {error !== null ? (
        <Notice tone="danger" role="alert">
          {errorMessage(error)}
        </Notice>
      ) : null}
      <FormActions>
        <Button type="submit" variant="danger" disabled={pending || confirm !== slug}>
          {pending ? "Deleting…" : "Delete organization"}
        </Button>
      </FormActions>
    </Form>
    </DangerZone>
  );
}

export function OrgSettingsView({ org }: { org: Org }) {
  const router = useRouter();
  const isOwner = org.my_role === "owner";
  return (
    <div className="max-w-[880px] space-y-8">
      <PageHeader title="Organization settings" description="Name, limits, conversation retention and deletion." className="mb-6" />
      <Section id="org-general" title="General" card>
        <GeneralForm key={`${org.slug}:${org.name}`} org={org} isOwner={isOwner} />
      </Section>
      {isOwner || org.my_role === "admin" ? <PreviewsForm org={org} /> : null}
      {isOwner || org.my_role === "admin" ? (
        <Section
          id="org-retention"
          title="Conversation retention"
          description="Delete conversations (messages, attachments and the agent's stored session) after this many days without activity. Leave empty to keep them until someone deletes them."
          card
        >
          <RetentionForm orgId={org.id} />
        </Section>
      ) : null}
      {isOwner ? <OrgFreeze orgId={org.id} name={org.name} frozen={org.frozen} /> : null}
      {isOwner ? <OrgDangerZone orgId={org.id} slug={org.slug} onDeleted={() => router.replace("/")} /> : null}
    </div>
  );
}
