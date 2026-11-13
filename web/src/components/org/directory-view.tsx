"use client";

// Directory sync (P4-GRP-2, docs/04 §SCIM): the org owner's view of `/scim/v2` — the enable
// switch, the e-mail domain allow-list, the `aud_` tokens (shown once) and the sync status.
// Owners only: every management route answers 403 for admins.

import { useEffect, useState, type FormEvent } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { oidcStartUrl } from "@/lib/api/auth";
import { useInstance } from "@/lib/api/queries";
import {
  lastActivityText,
  useCreateScimToken,
  usePutScimSettings,
  useRevokeScimToken,
  useScimSettings,
  useScimStatus,
  useScimTokens,
  type ScimSettings,
  type ScimTokenEntry,
} from "@/lib/api/scim";
import { fmtDate } from "@/components/account/key-table";

const reauthErr = (err: unknown) => err instanceof ApiError && err.code === "reauth_required";

/** The re-authentication notice: the provider sign-in link comes back to this page (docs/14 §Directory sync). */
function ReauthNotice({ err, reauthUrl }: { err: unknown; reauthUrl: string | null }) {
  if (!reauthErr(err)) return null;
  return (
    <Notice tone="pending" role="alert">
      Confirm it is you:{" "}
      {reauthUrl ? (
        <a className="font-medium underline" href={reauthUrl} data-testid="scim-reauth-link">
          sign in again with your provider
        </a>
      ) : (
        "sign in again with your provider"
      )}
      , then apply the change once more.
    </Notice>
  );
}

const parseDomains = (text: string) =>
  text
    .split(/[,\s]+/)
    .map((d) => d.trim().toLowerCase())
    .filter(Boolean);

function CopyUrlButton({ url }: { url: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="outline"
      aria-label="Copy base URL"
      onClick={() => {
        void navigator.clipboard?.writeText(url);
        setCopied(true);
      }}
    >
      {copied ? "Copied" : "Copy"}
    </Button>
  );
}

/** The enable switch + domains + exclusive mode. Only the fields that changed are sent. */
function SettingsForm({
  orgId,
  initial,
  reauthUrl,
}: {
  orgId: string;
  initial: ScimSettings;
  reauthUrl: string | null;
}) {
  const put = usePutScimSettings(orgId);
  const [enabled, setEnabled] = useState(initial.enabled);
  const [exclusive, setExclusive] = useState(initial.exclusive);
  const [domains, setDomains] = useState(initial.email_domains.join(", "));
  const [saved, setSaved] = useState(false);
  const fe = put.error && !reauthErr(put.error) ? errorMessage(put.error) : undefined;

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const list = parseDomains(domains);
    const body: { enabled?: boolean; email_domains?: string[]; exclusive?: boolean } = {};
    if (enabled !== initial.enabled) body.enabled = enabled;
    if (exclusive !== initial.exclusive) body.exclusive = exclusive;
    if (list.join(",") !== initial.email_domains.join(",")) body.email_domains = list;
    if (Object.keys(body).length === 0) {
      setSaved(true);
      return;
    }
    put.mutate(body, { onSuccess: () => setSaved(true) });
  }

  return (
    <Form onSubmit={submit} noValidate className="max-w-xl" aria-label="Directory sync settings">
      <CheckboxField
        id="scim-enabled"
        label="Enable directory sync (SCIM 2.0)"
        hint="Your identity provider provisions and deprovisions members of this organization through /scim/v2. People who already have an account show as “Waiting for first sign-in” until they next sign in."
        checked={enabled}
        onChange={setEnabled}
      />
      <Field
        id="scim-domains"
        label="Allowed e-mail domains"
        hint="Comma-separated (at most 20). The provider may only provision addresses under these domains; at least one is required to enable."
        error={fe}
      >
        <input
          id="scim-domains"
          className={inputClass}
          value={domains}
          placeholder="example.com, example.org"
          autoComplete="off"
          onChange={(e) => {
            setDomains(e.target.value);
            setSaved(false);
          }}
        />
      </Field>
      <CheckboxField
        id="scim-exclusive"
        label="Directory-managed membership (exclusive)"
        hint="Only people the directory provisioned can join or sign in to this organization; invitations and just-in-time sign-in are refused for everyone else."
        checked={exclusive}
        onChange={setExclusive}
      />
      <ReauthNotice err={put.error} reauthUrl={reauthUrl} />
      {saved ? (
        <Notice tone="accent" role="status">
          Directory sync settings saved.
        </Notice>
      ) : null}
      <FormActions>
        <Button type="submit" size="lg" disabled={put.isPending}>
          {put.isPending ? "Saving…" : "Save settings"}
        </Button>
      </FormActions>
    </Form>
  );
}

/** The `aud_` tokens: create (shown once), list, revoke. */
function TokenSection({
  orgId,
  enabled,
  reauthUrl,
}: {
  orgId: string;
  enabled: boolean;
  reauthUrl: string | null;
}) {
  const tokens = useScimTokens(orgId, enabled);
  const create = useCreateScimToken(orgId);
  const revoke = useRevokeScimToken(orgId);
  const [name, setName] = useState("");
  const [copied, setCopied] = useState(false);
  const [toRevoke, setToRevoke] = useState<ScimTokenEntry | null>(null);
  const err = create.error ?? revoke.error;
  const fe = err && !reauthErr(err) ? errorMessage(err) : undefined;
  // The secret lives only in the mutation result; leaving the page drops it (docs/17: shown once).
  const { reset: resetCreate } = create;
  useEffect(() => () => resetCreate(), [resetCreate]);
  const created = create.data?.token ?? null;
  const expiring = (tokens.data?.tokens ?? []).filter((t) => t.expiring && !t.revoked);

  function submit(e: FormEvent) {
    e.preventDefault();
    setCopied(false);
    create.mutate({ name: name.trim() }, { onSuccess: () => setName("") });
  }

  return (
    <div className="max-w-xl space-y-4">
      {enabled ? (
      <Form onSubmit={submit} noValidate aria-label="Create a SCIM token">
        <Field
          id="scim-token-name"
          label="New token name"
          hint="Shown in the audit trail as scim:<name>."
          error={fe}
        >
          <div className="flex gap-2">
            <input
              id="scim-token-name"
              className={inputClass}
              value={name}
              maxLength={80}
              placeholder="Entra ID"
              autoComplete="off"
              onChange={(e) => setName(e.target.value)}
            />
            <Button type="submit" disabled={create.isPending || !name.trim()}>
              {create.isPending ? "Creating…" : "Create token"}
            </Button>
          </div>
        </Field>
        <ReauthNotice err={err} reauthUrl={reauthUrl} />
      </Form>
      ) : null}
      <Dialog open={!!created} onOpenChange={(o) => !o && create.reset()}>
        <DialogContent
          title="Your new token"
          description="Store it in your identity provider now; it cannot be shown again."
        >
          <div className="flex items-center gap-2">
            <code className="block min-w-0 max-w-full flex-1 break-all rounded bg-surface-2 px-2 py-1 font-mono text-xs">
              {created}
            </code>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                if (created) void navigator.clipboard?.writeText(created);
                setCopied(true);
              }}
            >
              {copied ? "Copied" : "Copy"}
            </Button>
          </div>
          <div className="mt-4 flex justify-end">
            <DialogClose asChild>
              <Button>Done</Button>
            </DialogClose>
          </div>
        </DialogContent>
      </Dialog>
      {expiring.length > 0 ? (
        <Notice tone="pending" role="status">
          {expiring.length === 1
            ? `The token “${expiring[0]!.name}” expires within 14 days.`
            : `${expiring.length} tokens expire within 14 days.`}{" "}
          Create a new one, paste it into your identity provider, then revoke the old one.
        </Notice>
      ) : null}
      <ConfirmDialog
        open={!!toRevoke}
        onOpenChange={(o) => !o && setToRevoke(null)}
        title="Revoke this token?"
        description={`Your identity provider stops syncing as soon as “${toRevoke?.name ?? ""}” is revoked. This cannot be undone.`}
        confirmLabel="Revoke token"
        danger
        busy={revoke.isPending}
        onConfirm={() => {
          if (toRevoke) revoke.mutate(toRevoke.id);
          setToRevoke(null);
        }}
      />
      {tokens.data && tokens.data.tokens.length > 0 ? (
        <DataTable caption="SCIM tokens of this organization" className="max-w-xl">
          <thead>
            <tr>
              <Th>Name</Th>
              <Th>Token</Th>
              <Th>Expires</Th>
              <Th>Last used</Th>
              <Th>
                <span className="sr-only">Actions</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {tokens.data.tokens.map((t) => (
              <TokenRow
                key={t.id}
                t={t}
                busy={revoke.isPending}
                onRevoke={() => setToRevoke(t)}
              />
            ))}
          </tbody>
        </DataTable>
      ) : (
        <p className="text-muted text-sm">
          {enabled
            ? "No tokens yet. Create one and paste it into your identity provider's provisioning settings."
            : "Enable directory sync to create a token."}
        </p>
      )}
    </div>
  );
}

function TokenRow({
  t,
  busy,
  onRevoke,
}: {
  t: ScimTokenEntry;
  busy?: boolean;
  onRevoke: () => void;
}) {
  return (
    <tr className="border-t border-line">
      <Td className="font-medium">{t.name}</Td>
      <Td className="font-mono text-xs">{t.display_prefix}…</Td>
      <Td>
        <span className="flex items-center gap-1.5">
          <span className="whitespace-nowrap">{fmtDate(t.expires_at)}</span>
          {t.expiring && !t.revoked ? <Badge tone="pending">expiring</Badge> : null}
        </span>
      </Td>
      <Td className="text-ink-2">{t.last_used_at ? fmtDate(t.last_used_at) : "never"}</Td>
      <Td className="text-right">
        {t.revoked ? (
          <Badge tone="neutral">revoked</Badge>
        ) : (
          <Button variant="outline" size="sm" disabled={busy} onClick={onRevoke}>
            Revoke
          </Button>
        )}
      </Td>
    </tr>
  );
}

/** The sync status card: counts, last request, recent errors. */
function StatusCard({ orgId, enabled }: { orgId: string; enabled: boolean }) {
  const status = useScimStatus(orgId, enabled);
  if (!enabled || status.isPending || !status.data) return null;
  const s = status.data;
  return (
    <div className="max-w-xl space-y-2 text-sm">
      <p className="text-muted">{lastActivityText(s)}</p>
      <p>
        {s.users.active} active, {s.users.suspended} suspended, {s.users.removed} removed ·{" "}
        {s.groups} SCIM group{s.groups === 1 ? "" : "s"}
      </p>
      {s.errors.length > 0 ? (
        <div>
          <p className="mt-2 font-medium">Recent rejected requests</p>
          <ul className="list-disc space-y-0.5 pl-5 text-muted">
            {s.errors.slice(0, 20).map((e, i) => (
              <li key={i}>
                <span className="font-mono text-xs">{e.method}</span> {e.resource} — {e.status}
                {e.scim_type ? ` (${e.scim_type})` : ""}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  );
}

export function OrgDirectoryView({ org, isOwner }: { org: { id: string; slug: string }; isOwner: boolean }) {
  const settings = useScimSettings(org.id);
  const instance = useInstance();
  if (settings.isPending) return null;
  if (settings.error) {
    return (
      <Notice tone="danger" role="alert">
        {errorMessage(settings.error)}
      </Notice>
    );
  }
  const s = settings.data;
  if (!s) return null;
  if (!s.available) {
    return (
      <Notice tone="neutral" role="note">
        Directory sync (SCIM) is not enabled on this Aurora instance. An instance administrator
        can turn it on with the <code className="font-mono text-ink">[scim]</code> configuration
        section.
      </Notice>
    );
  }
  const providers = instance.data?.oidc_providers ?? [];
  const reauthUrl = providers.length > 0
    ? oidcStartUrl(providers[0]!.id, { reauth: true, returnTo: `/o/${org.slug}/settings/directory` })
    : null;
  return (
    <div className="max-w-page-narrow space-y-8">
      <PageHeader
        title="Directory sync"
        description="Provision members from your identity provider over SCIM 2.0."
        className="mb-6"
      />
      {!isOwner ? (
        <Notice role="status">Only organization owners can manage directory sync.</Notice>
      ) : (
        <>
          <Section
            id="scim-settings"
            title="Settings"
            description="The base URL your identity provider needs, then the enable switch, the domains it may provision and whether the directory alone decides membership."
            card
          >
            <div className="mb-4 max-w-xl">
              <Field
                id="scim-base-url"
                label="Base URL"
                hint="Setup guide for Entra ID, Okta and Keycloak (base URL, token, attribute mapping userName / emails / active; do not send roles): docs/operator/scim-provisioning.md."
              >
                <div className="flex gap-2">
                  <input
                    id="scim-base-url"
                    className={inputClass}
                    readOnly
                    value={s.base_url}
                    onFocus={(e) => e.currentTarget.select()}
                  />
                  <CopyUrlButton url={s.base_url} />
                </div>
              </Field>
            </div>
            <SettingsForm orgId={org.id} initial={s} reauthUrl={reauthUrl} />
          </Section>
          <Section
            id="scim-tokens"
            title="Provisioning tokens"
            description="A token authenticates your identity provider against /scim/v2. It is shown once; create a new one and revoke the old one to rotate."
            card
          >
            <TokenSection orgId={org.id} enabled={s.enabled} reauthUrl={reauthUrl} />
          </Section>
          <Section
            id="scim-status"
            title="Status"
            description="What the directory provisioned so far and which requests were rejected."
            card
          >
            <StatusCard orgId={org.id} enabled={s.enabled} />
          </Section>
        </>
      )}
    </div>
  );
}
