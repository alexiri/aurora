"use client";

import { useState, type FormEvent } from "react";

import { KeyTable } from "@/components/account/key-table";
import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { useRevokeApiKey } from "@/lib/api/account";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
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
    <div className="max-w-4xl space-y-6">
      <h1 className="text-xl font-semibold">Organization API keys</h1>
      <form onSubmit={(e) => void submit(e)} noValidate className="flex flex-wrap items-end gap-3">
        <Field id="okey-name" label="Key name" error={clientErr || fe.name}>
          <input
            id="okey-name"
            className={inputClass}
            value={name}
            maxLength={80}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
        <Field id="okey-exp" label="Expires (optional)" error={fe.expires_at}>
          <input
            id="okey-exp"
            type="date"
            className={inputClass}
            value={expires}
            onChange={(e) => setExpires(e.target.value)}
          />
        </Field>
        <Button type="submit" disabled={create.isPending}>
          Create key
        </Button>
        {create.error && Object.keys(fe).length === 0 ? (
          <p role="alert" className="basis-full text-sm text-red-700">
            {errorMessage(create.error)}
          </p>
        ) : null}
      </form>
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
      <OneTimeKeyDialog
        secret={created?.secret ?? null}
        name={created?.name}
        onClose={() => setCreated(null)}
      />
    </div>
  );
}
