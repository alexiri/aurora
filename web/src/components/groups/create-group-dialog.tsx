"use client";

import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useCreateGroup, type CreateGroupRequest } from "@/lib/api/groups";
import { useInstance } from "@/lib/api/queries";

function CreateForm({ orgId, orgSlug, onDone }: { orgId: string; orgSlug: string; onDone: () => void }) {
  const router = useRouter();
  const create = useCreateGroup(orgId);
  const instance = useInstance();
  const providers = (instance.data?.oidc_providers ?? []).filter((p) => p.groups);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [linked, setLinked] = useState(false);
  const [provider, setProvider] = useState("");
  const [value, setValue] = useState("");
  const [orgRole, setOrgRole] = useState<"" | "member" | "admin">("");
  const [clientErr, setClientErr] = useState("");
  const fe = create.error ? fieldErrors(create.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim()) return setClientErr("Enter a name.");
    if (linked && (!(provider || providers[0]?.id) || !value.trim())) {
      return setClientErr("Choose the provider and type the claim value.");
    }
    setClientErr("");
    const body: CreateGroupRequest = {
      name: name.trim(),
      description: description.trim() || undefined,
      org_role: orgRole || undefined,
      ...(linked
        ? { source: "idp", idp_provider: provider || providers[0]?.id, idp_value: value.trim() }
        : {}),
    };
    create.mutate(body, {
      onSuccess: (g) => {
        onDone();
        router.push(`/o/${orgSlug}/groups/${g.id}`);
      },
    });
  }

  return (
    <form onSubmit={submit} noValidate className="space-y-4" aria-label="New group">
      <Field id="grp-name" label="Name" error={clientErr || fe.name}>
        <input
          id="grp-name"
          className={inputClass}
          value={name}
          maxLength={80}
          autoComplete="off"
          onChange={(e) => setName(e.target.value)}
        />
      </Field>
      <Field id="grp-desc" label="Description (optional)" error={fe.description}>
        <input
          id="grp-desc"
          className={inputClass}
          value={description}
          maxLength={280}
          onChange={(e) => setDescription(e.target.value)}
        />
      </Field>
      <Field id="grp-members" label="Members">
        <select
          id="grp-members"
          className={inputClass}
          value={linked ? "idp" : "manual"}
          onChange={(e) => setLinked(e.target.value === "idp")}
        >
          <option value="manual">Added by hand</option>
          {providers.length > 0 ? <option value="idp">Linked to a sign-in provider</option> : null}
        </select>
      </Field>
      {linked ? (
        <>
          <Field id="grp-provider" label="Sign-in provider">
            <select
              id="grp-provider"
              className={inputClass}
              value={provider || providers[0]?.id || ""}
              onChange={(e) => setProvider(e.target.value)}
            >
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.display_name}
                </option>
              ))}
            </select>
          </Field>
          <Field
            id="grp-value"
            label="Claim value"
            hint="Exact, case-sensitive value of the provider's group claim (for CERN, an e-group or application role)."
            error={fe.idp_value}
          >
            <input
              id="grp-value"
              className={inputClass}
              value={value}
              maxLength={256}
              autoComplete="off"
              onChange={(e) => setValue(e.target.value)}
            />
          </Field>
        </>
      ) : null}
      <Field
        id="grp-role"
        label="Organization role (optional)"
        hint="Members act as this role in the whole organization. Never Owner."
      >
        <select
          id="grp-role"
          className={inputClass}
          value={orgRole}
          onChange={(e) => setOrgRole(e.target.value as "" | "member" | "admin")}
        >
          <option value="">None</option>
          <option value="member">Member</option>
          <option value="admin">Admin</option>
        </select>
      </Field>
      {orgRole === "admin" ? (
        <Notice tone="pending" role="status">
          Admin gives every member of this group ownership of every workspace in the organization.
        </Notice>
      ) : null}
      {create.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {errorMessage(create.error)}
        </Notice>
      ) : null}
      <div className="flex justify-end">
        <Button type="submit" size="lg" disabled={create.isPending}>
          {create.isPending ? "Creating…" : "Create group"}
        </Button>
      </div>
    </form>
  );
}

export function CreateGroupDialog({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const [open, setOpen] = useState(false);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button>New group</Button>
      </DialogTrigger>
      <DialogContent
        title="New group"
        description="A named set of people in this organization that can hold workspace access."
      >
        <CreateForm orgId={orgId} orgSlug={orgSlug} onDone={() => setOpen(false)} />
      </DialogContent>
    </Dialog>
  );
}
