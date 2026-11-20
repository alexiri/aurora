"use client";

import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { OrgFreeze } from "@/components/freeze/freeze-sections";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, Form } from "@/components/ui/form";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { SaveBar, useSaveBar } from "@/components/ui/save-bar";
import { DangerZone, Section } from "@/components/ui/section";
import { DeleteWithName } from "@/components/ui/type-to-confirm";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { deleteOrg, usePatchOrg } from "@/lib/api/org";
import { useInstance } from "@/lib/api/queries";
import { orgRoutes } from "@/lib/routes";
import type { Org } from "@/lib/api/queries";

function parseLimit(v: string): number | null | "invalid" {
  const t = v.trim();
  if (t === "") return null;
  if (!/^\d+$/.test(t)) return "invalid";
  const n = Number(t);
  return n >= 1 ? n : "invalid";
}

export function GeneralForm({ org, isOwner }: { org: Org; isOwner: boolean }) {
  const router = useRouter();
  const patch = usePatchOrg(org.id);
  const [name, setName] = useState(org.name);
  const [slug, setSlug] = useState(org.slug);
  const [canCreate, setCanCreate] = useState(org.settings.members_can_create_workspaces);
  const [maxMembers, setMaxMembers] = useState(org.settings.max_members?.toString() ?? "");
  const [maxAgents, setMaxAgents] = useState(org.settings.max_agents?.toString() ?? "");
  const [clientErr, setClientErr] = useState<Record<string, string>>({});
  const bar = useSaveBar();
  // What is saved: the form's start, and again after each save.
  const [base, setBase] = useState({
    name: org.name,
    slug: org.slug,
    canCreate: org.settings.members_can_create_workspaces,
    maxMembers: org.settings.max_members?.toString() ?? "",
    maxAgents: org.settings.max_agents?.toString() ?? "",
  });
  const dirty =
    name !== base.name || slug !== base.slug || canCreate !== base.canCreate || maxMembers !== base.maxMembers || maxAgents !== base.maxAgents;
  const fe = patch.error ? fieldErrors(patch.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    bar.clear();
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
          setBase({ name, slug, canCreate, maxMembers, maxAgents });
          bar.flash();
          if (updated.slug !== org.slug) router.replace(orgRoutes(updated.slug).settings());
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
      {isOwner ? (
        <SaveBar
          dirty={dirty}
          saving={patch.isPending}
          saved={bar.saved}
          errorCount={Object.keys(clientErr).length + Object.keys(fe).length}
          error={patch.error && Object.keys(fe).length === 0 ? errorMessage(patch.error) : undefined}
          onDiscard={() => {
            setName(base.name);
            setSlug(base.slug);
            setCanCreate(base.canCreate);
            setMaxMembers(base.maxMembers);
            setMaxAgents(base.maxAgents);
            setClientErr({});
            patch.reset();
          }}
        />
      ) : null}
    </Form>
  );
}

/** The org's Port previews switch (P4-PRV-1, C-549, docs/14). The card exists only when the
 * instance runs previews; the switch is owners/admins and audited. Turning it off asks for
 * confirmation: existing links stop working until it is turned back on. */
export function PreviewsForm({ org }: { org: Org }) {
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
      <ErrorNotice error={patch.error} />
    </Section>
  );
}

function isOwnerAdmin(org: Org): boolean {
  return org.my_role === "owner" || org.my_role === "admin";
}

export function OrgDangerZone({
  orgId,
  slug,
  name,
  onDeleted,
}: {
  orgId: string;
  slug: string;
  /** The organization's visible name, which the person types to confirm. */
  name: string;
  onDeleted: () => void;
}) {
  return (
    <DangerZone
      id="delete-org"
      title="Delete this organization"
      description="This permanently deletes the organization, its workspaces and everything in them."
    >
      <DeleteWithName
        buttonLabel="Delete organization"
        title={`Delete ${name}?`}
        consequence="This deletes the organization, its workspaces, agents, files and conversations. It can't be undone."
        noun="organization"
        name={name}
        run={() => deleteOrg(orgId, slug)}
        onDone={onDeleted}
      />
    </DangerZone>
  );
}

/** Settings › Danger zone (P5-IA-1): freezing and deleting the organization, owners only. */
export function OrgDangerSections({ org }: { org: Org }) {
  const router = useRouter();
  if (org.my_role !== "owner") return <Notice role="status">Only organization owners can pause or delete the organization.</Notice>;
  return (
    <div className="space-y-8">
      <OrgFreeze orgId={org.id} name={org.name} frozen={org.frozen} />
      <OrgDangerZone orgId={org.id} slug={org.slug} name={org.name} onDeleted={() => router.replace("/")} />
    </div>
  );
}
