"use client";

import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { OrgFreeze } from "@/components/freeze/freeze-sections";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { deleteOrg, usePatchOrg, usePutRetention, useRetention } from "@/lib/api/org";
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
    <form onSubmit={submit} noValidate className="max-w-md space-y-4">
      {!isOwner ? (
        <p className="rounded-md bg-neutral-100 p-3 text-sm text-neutral-700">
          Only organization owners can change these settings.
        </p>
      ) : null}
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
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={canCreate}
            onChange={(e) => setCanCreate(e.target.checked)}
          />
          Members can create workspaces
        </label>
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
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(patch.error)}
        </p>
      ) : null}
      {saved ? (
        <p role="status" className="text-sm text-green-700">
          Settings saved.
        </p>
      ) : null}
      {isOwner ? (
        <Button type="submit" disabled={patch.isPending}>
          {patch.isPending ? "Saving…" : "Save changes"}
        </Button>
      ) : null}
    </form>
  );
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
    <form onSubmit={submit} noValidate aria-label="Conversation retention" className="max-w-md space-y-3">
      <h2 className="text-lg font-medium">Conversation retention</h2>
      <p className="text-sm text-neutral-600">
        Delete conversations (messages, attachments and the agent&apos;s stored session) after this many days without activity. Leave
        empty to keep them until someone deletes them.
      </p>
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
        <p role="status" className="text-sm text-green-700">
          Retention saved.
        </p>
      ) : null}
      <Button type="submit" disabled={put.isPending}>
        {put.isPending ? "Saving…" : "Save retention"}
      </Button>
    </form>
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
    <form onSubmit={(e) => void submit(e)} className="max-w-md space-y-4">
      <h2 className="text-lg font-medium text-red-800">Delete this organization</h2>
      <p className="text-sm text-neutral-600">
        This permanently deletes the organization, its workspaces and everything in them. Type{" "}
        <code className="font-mono">{slug}</code> to confirm.
      </p>
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
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(error)}
        </p>
      ) : null}
      <Button type="submit" disabled={pending || confirm !== slug} className="bg-red-700 text-white">
        {pending ? "Deleting…" : "Delete organization"}
      </Button>
    </form>
  );
}

export function OrgSettingsView({ org }: { org: Org }) {
  const router = useRouter();
  const isOwner = org.my_role === "owner";
  return (
    <div className="max-w-4xl space-y-8">
      <h1 className="text-xl font-semibold">Organization settings</h1>
      <GeneralForm key={`${org.slug}:${org.name}`} org={org} isOwner={isOwner} />
      {isOwner || org.my_role === "admin" ? (
        <section aria-label="Retention" className="border-t border-neutral-200 pt-6">
          <RetentionForm orgId={org.id} />
        </section>
      ) : null}
      {isOwner ? (
        <section aria-label="Danger zone" className="space-y-8 border-t border-neutral-200 pt-6">
          <OrgFreeze orgId={org.id} name={org.name} frozen={org.frozen} />
          <OrgDangerZone orgId={org.id} slug={org.slug} onDeleted={() => router.replace("/")} />
        </section>
      ) : null}
    </div>
  );
}
