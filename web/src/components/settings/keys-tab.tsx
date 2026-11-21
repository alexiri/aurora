"use client";

import { useState, type FormEvent } from "react";

import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { Altitude } from "@/components/ui/altitude";
import { CopyCurlMenu } from "@/components/ui/copy-curl";
import { FromYourTerminal } from "@/components/ui/from-your-terminal";
import { apiKeyCurl } from "@/lib/copy-as-curl";
import { useOrigin } from "@/lib/use-origin";
import { DateField } from "@/components/ui/date-field";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice } from "@/components/ui/notice";
import { SecretOnce } from "@/components/ui/secret-once";
import { SkeletonRows } from "@/components/ui/skeleton";
import { When } from "@/components/ui/when";
import { EmptyState } from "@/components/states";
import { accountKeys, useCreateWorkspaceKey, useRevokeApiKey, useWorkspaceKeys, type ApiKey } from "@/lib/api/account";
import { fieldErrors } from "@/lib/api/errors";
import { cn } from "@/lib/utils";

/** What a key can do, as the key row says it (the API has no scopes: a key acts as its owner in the workspace). */
export function keyCanText(workspaceName: string): string {
  return `Can do everything you can in ${workspaceName}`;
}

/**
 * (P5-REFINE-16, workspace-6) API keys of the workspace: what a key can do, a New API key dialog, and a revoke that asks
 * first. The secret is shown once in `SecretOnce`.
 */
export function KeysTab({ wsId, workspaceName = "this workspace" }: { wsId: string; workspaceName?: string }) {
  const origin = useOrigin();
  const keys = useWorkspaceKeys(wsId);
  const create = useCreateWorkspaceKey(wsId);
  const revoke = useRevokeApiKey(accountKeys.wsKeys(wsId));
  const [dialog, setDialog] = useState(false);
  const [name, setName] = useState("");
  const [expires, setExpires] = useState("");
  const [clientErr, setClientErr] = useState("");
  const [created, setCreated] = useState<{ secret: string; name: string } | null>(null);
  const [target, setTarget] = useState<ApiKey | null>(null);
  const fe = create.error ? fieldErrors(create.error) : {};

  const close = (open: boolean) => {
    setDialog(open);
    if (!open) {
      setName("");
      setExpires("");
      setClientErr("");
      create.reset();
    }
  };

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
      close(false);
    } catch {
      // surfaced through create.error
    }
  }

  const active = (keys.data?.items ?? []).filter((k) => !k.revoked_at);
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="font-display text-lg font-semibold text-ink">API keys</h2>
        <Button onClick={() => setDialog(true)}>New API key</Button>
      </div>
      <p className="text-sm text-ink-2">
        Acts as you in this workspace through the Aurora API. Anyone who has a key can do what you can, so keep it secret.
        {/* The instance serves its API description at /v1/openapi.json (Expert: it is for people who write calls). */}
        <Altitude id="copy.curl" level="expert">
          {" "}
          <a className="text-link underline" href="/v1/openapi.json" target="_blank" rel="noreferrer">
            OpenAPI description
          </a>
        </Altitude>
      </p>
      {keys.isPending ? <SkeletonRows rows={2} label="Loading your keys…" /> : null}
      {keys.isError ? <ErrorState error={keys.error} onRetry={() => keys.refetch()} /> : null}
      {keys.data && active.length === 0 ? <EmptyState title="No API keys yet" /> : null}
      <ErrorNotice error={revoke.error} />
      {active.length > 0 ? (
        <ul aria-label="API keys" className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface shadow-card">
          {active.map((k) => (
            <li key={k.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
              <div className="min-w-0 flex-1 basis-56">
                <p className="text-sm font-medium text-ink">{k.name}</p>
                <p className="text-xs text-muted">
                  {keyCanText(workspaceName)} · created <When value={k.created_at} /> ·{" "}
                  {k.last_used_at ? (
                    <>
                      last used <When value={k.last_used_at} />
                    </>
                  ) : (
                    "never used"
                  )}
                </p>
              </div>
              {/* (P5-REFINE-19, Expert) The call the key's own row makes: placeholders only, never the key. */}
              <CopyCurlMenu subject={k.name} command={() => apiKeyCurl(window.location.origin)} />
              <Button variant="danger" size="sm" aria-label={`Revoke ${k.name}`} onClick={() => setTarget(k)}>
                Revoke
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
      <FromYourTerminal
        commands={[
          { label: "the aurora login command", text: "aurora login", note: "Sign in from your terminal once; the CLI stores a key of its own." },
          { label: "the curl command", text: apiKeyCurl(origin), note: "Call the API with a key you saved: it answers with who the key acts as." },
        ]}
      />

      <FormDialog
        open={dialog}
        onOpenChange={close}
        title="New API key"
        description="The key is shown once, right after you create it."
        submitLabel="Create key"
        busy={create.isPending}
        error={create.error && Object.keys(fe).length === 0 ? create.error : null}
        onSubmit={(e) => void submit(e)}
      >
        <Field id="key-name" label="Name" error={clientErr || fe.name}>
          <input id="key-name" className={cn(inputClass, "w-full")} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field id="key-exp" label="Expires (optional)" error={fe.expires_at}>
          <DateField id="key-exp" value={expires} onChange={setExpires} />
        </Field>
      </FormDialog>

      <ConfirmDialog
        open={target !== null}
        onOpenChange={(o) => !o && setTarget(null)}
        title={`Revoke ${target?.name ?? "this key"}?`}
        description="Anything using it stops working now."
        confirmLabel="Revoke key"
        danger
        busy={revoke.isPending}
        onConfirm={() => {
          if (!target) return;
          void revoke.mutateAsync(target.id).then(
            () => setTarget(null),
            () => setTarget(null),
          );
        }}
      />

      <SecretOnce
        secret={created?.secret ?? null}
        title="Copy your new API key"
        description={`Key${created?.name ? ` "${created.name}"` : ""} created. You won't see it again, so store it somewhere safe now.`}
        onClose={() => setCreated(null)}
      />
    </div>
  );
}
