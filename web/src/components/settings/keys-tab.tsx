"use client";

import { useState, type FormEvent } from "react";

import { KeyTable } from "@/components/account/key-table";
import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { cn } from "@/lib/utils";
import {
  accountKeys,
  useCreateWorkspaceKey,
  useRevokeApiKey,
  useWorkspaceKeys,
} from "@/lib/api/account";
import { errorMessage, fieldErrors } from "@/lib/api/errors";

export function KeysTab({ wsId }: { wsId: string }) {
  const keys = useWorkspaceKeys(wsId);
  const create = useCreateWorkspaceKey(wsId);
  const revoke = useRevokeApiKey(accountKeys.wsKeys(wsId));
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
    <div className="space-y-8">
      <Section id="wkey-create" title="Create a key" description="The key is shown once, right after you create it." card>
        <form onSubmit={(e) => void submit(e)} noValidate className="flex flex-wrap items-start gap-x-4 gap-y-3">
          <Field id="key-name" label="Key name" error={clientErr || fe.name}>
            <input
              id="key-name"
              className={cn(inputClass, "w-64 max-w-full")}
              value={name}
              maxLength={80}
              onChange={(e) => setName(e.target.value)}
            />
          </Field>
          <Field id="key-exp" label="Expires (optional)" error={fe.expires_at}>
            <input
              id="key-exp"
              type="date"
              className={cn(inputClass, "w-48 max-w-full")}
              value={expires}
              onChange={(e) => setExpires(e.target.value)}
            />
          </Field>
          <Button type="submit" disabled={create.isPending} className="sm:mt-label-offset">
            Create key
          </Button>
          {create.error && Object.keys(fe).length === 0 ? (
            <Notice tone="danger" role="alert" className="basis-full">
              {errorMessage(create.error)}
            </Notice>
          ) : null}
        </form>
      </Section>
      <Section id="wkeys" title="Keys">
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
      <OneTimeKeyDialog
        secret={created?.secret ?? null}
        name={created?.name}
        onClose={() => setCreated(null)}
      />
    </div>
  );
}
