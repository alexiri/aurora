"use client";

import { useState, type FormEvent } from "react";

import { KeyTable } from "@/components/account/key-table";
import { SecretOnce } from "@/components/ui/secret-once";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { DateField } from "@/components/ui/date-field";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { cn } from "@/lib/utils";
import { useRevokeApiKey } from "@/lib/api/account";
import { fieldErrors } from "@/lib/api/errors";
import { orgKeys, useCreateOrgKey, useOrgKeys } from "@/lib/api/org";

export function OrgKeysView({ orgId }: { orgId: string }) {
  const keys = useOrgKeys(orgId);
  const create = useCreateOrgKey(orgId);
  const revoke = useRevokeApiKey(orgKeys.keys(orgId));
  const [name, setName] = useState("");
  const [expires, setExpires] = useState("");
  const [clientErr, setClientErr] = useState("");
  const [created, setCreated] = useState<{ secret: string; name: string } | null>(null);
  const fe = create.error ? fieldErrors(create.error) : {};

  async function submit(e: FormEvent) {
    e.preventDefault();
    const n = name.trim();
    if (!n || n.length > 80) return setClientErr("Name must be 1 to 80 characters.");
    let expires_at: string | null = null;
    if (expires) {
      const d = new Date(`${expires}T23:59:59`);
      if (!(d.getTime() > Date.now())) return setClientErr("Expiry must be in the future.");
      expires_at = d.toISOString();
    }
    setClientErr("");
    try {
      const res = await create.mutateAsync({ name: n, expires_at });
      setCreated({ secret: res.key, name: res.api_key.name });
      setName("");
      setExpires("");
      create.reset();
    } catch {
      // surfaced through create.error
    }
  }

  return (
    <div className="max-w-page space-y-8">
      <PageHeader title="Organization API keys" description="Keys that act for the whole organization, for scripts and integrations." className="mb-6" />
      <Section id="okey-create" title="Create a key" description="The key is shown once, right after you create it." card>
        <form onSubmit={(e) => void submit(e)} noValidate className="flex flex-wrap items-start gap-x-4 gap-y-3">
          <Field id="okey-name" label="Key name" error={clientErr || fe.name}>
            <input
              id="okey-name"
              className={cn(inputClass, "w-64 max-w-full")}
              value={name}
              maxLength={80}
              onChange={(e) => setName(e.target.value)}
            />
          </Field>
          <Field id="okey-exp" label="Expires (optional)" error={fe.expires_at}>
            <DateField id="okey-exp" value={expires} onChange={setExpires} />
          </Field>
          <Button type="submit" disabled={create.isPending} className="sm:mt-label-offset">
            Create key
          </Button>
          {create.error && Object.keys(fe).length === 0 ? (
            <ErrorNotice error={create.error} className="basis-full" />
          ) : null}
        </form>
      </Section>
      <Section id="okeys" title="Keys">
        {keys.isPending ? <Loading /> : null}
        {keys.isError ? <ErrorState error={keys.error} onRetry={() => keys.refetch()} /> : null}
        {keys.data ? (
          <KeyTable
            keys={keys.data.items}
            onRevoke={(id) => revoke.mutateAsync(id)}
            revoking={revoke.isPending}
            error={revoke.error}
          />
        ) : null}
      </Section>
      <SecretOnce
        secret={created?.secret ?? null}
        title="Copy your new API key"
        description={`Key${created?.name ? ` "${created.name}"` : ""} created. You won't see it again, so store it somewhere safe now.`}
        onClose={() => setCreated(null)}
      />
    </div>
  );
}
