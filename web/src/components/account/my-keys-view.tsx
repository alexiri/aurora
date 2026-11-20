"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { PageHeader } from "@/components/ui/page-header";
import { SecretOnce } from "@/components/ui/secret-once";
import { Select } from "@/components/ui/select";
import { accountKeys, useCreateWorkspaceKey, useMyApiKeys, useRevokeApiKey } from "@/lib/api/account";
import { fieldErrors } from "@/lib/api/errors";
import { useSession } from "@/lib/api/queries";
import { workspaceDisplayName } from "@/lib/workspace-name";

import { KeyTable } from "./key-table";

/**
 * (account-2) "Create an API key": a name and the workspace it can act in (every workspace the person is in). The key can act
 * only there; it calls the per-workspace create and shows the secret once.
 */
function CreateKeyDialog({ open, onOpenChange, onCreated }: { open: boolean; onOpenChange: (o: boolean) => void; onCreated: (k: { secret: string; name: string }) => void }) {
  const me = useSession();
  const options = (me.data?.memberships ?? []).flatMap((m) =>
    m.workspaces.map((w) => ({ value: w.id, label: `${workspaceDisplayName({ name: w.name ?? w.slug, personal: w.personal }, me.data?.user)} · ${m.org.name ?? m.org.slug}` })),
  );
  const [name, setName] = useState("");
  const [ws, setWs] = useState("");
  const wsId = ws || options[0]?.value || "";
  const create = useCreateWorkspaceKey(wsId);
  const fe = create.error ? fieldErrors(create.error) : {};
  const close = (o: boolean) => {
    if (!o) {
      setName("");
      setWs("");
      create.reset();
    }
    onOpenChange(o);
  };
  return (
    <FormDialog
      open={open}
      onOpenChange={close}
      title="Create an API key"
      submitLabel="Create key"
      busy={create.isPending}
      submitDisabled={name.trim() === "" || !wsId}
      error={create.error && Object.keys(fe).length === 0 ? create.error : null}
      onSubmit={(e) => {
        e.preventDefault();
        create.mutate(
          { name: name.trim(), expires_at: null },
          {
            onSuccess: (res) => {
              onCreated({ secret: res.key, name: res.api_key.name });
              close(false);
            },
          },
        );
      }}
    >
      <Field id="mykey-name" label="Name" error={fe.name}>
        <input id="mykey-name" className={inputClass} value={name} maxLength={80} placeholder="My laptop CLI" autoComplete="off" onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="mykey-ws" label="Workspace" hint="The key can act only in this workspace.">
        <Select id="mykey-ws" value={wsId} onChange={setWs} options={options} />
      </Field>
    </FormDialog>
  );
}

export function MyKeysView() {
  const keys = useMyApiKeys();
  const revoke = useRevokeApiKey(accountKeys.myKeys);
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<{ secret: string; name: string } | null>(null);
  if (keys.isPending) return <Loading />;
  if (keys.isError) return <ErrorState error={keys.error} onRetry={() => keys.refetch()} />;
  const empty = keys.data.items.length === 0;
  return (
    <div className="space-y-4">
      <PageHeader
        title="API keys"
        description="Personal keys you created for workspaces (for example for the CLI)."
        actions={<Button onClick={() => setCreating(true)}>Create key</Button>}
      />
      {empty ? (
        <EmptyState title="You haven't created an API key" action={<Button onClick={() => setCreating(true)}>Create key</Button>} />
      ) : (
        <>
          {/* (P5-REFINE-3) Set up: what the keys are for. The list itself stays for everyone who has keys. */}
          <Altitude id="apikeys" level="setup">
            <p className="text-sm text-ink-2">
              Acts as you in the workspace you chose through the Aurora API.
              <Altitude id="copy.curl" level="expert">
                {" "}
                <a className="text-link underline" href="/v1/openapi.json" target="_blank" rel="noreferrer">
                  OpenAPI description
                </a>
              </Altitude>
            </p>
          </Altitude>
          <KeyTable
            keys={keys.data.items}
            showWorkspace
            onRevoke={(id) => revoke.mutateAsync(id)}
            revoking={revoke.isPending}
            error={revoke.error}
          />
        </>
      )}
      <CreateKeyDialog open={creating} onOpenChange={setCreating} onCreated={setCreated} />
      <SecretOnce
        secret={created?.secret ?? null}
        title="Copy your new API key"
        description={`Key${created?.name ? ` "${created.name}"` : ""} created. You won't see it again, so store it somewhere safe now.`}
        onClose={() => setCreated(null)}
      />
    </div>
  );
}
