"use client";

import { useState, type ReactNode } from "react";

import { HelmProviderDialog, ProviderDialog } from "@/components/admin/settings/sign-in-provider-dialog";
import { SettingsSectionForm, type SettingControlCtx, type SettingsGroup } from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { LockPill } from "@/components/ui/lock-pill";
import { Button } from "@/components/ui/button";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow } from "@/components/ui/section";
import { SettingField, settingControlId } from "@/components/ui/setting-field";
import { useInstance } from "@/lib/api/queries";
import { useSettingsSection } from "@/lib/api/settings";

export const PROVIDERS = "auth.oidc_providers";

/** The three groups of the page (`15 §Instance settings`, row `sign_in`); every key of the section appears once. */
export const SIGN_IN_GROUPS: SettingsGroup[] = [
  {
    title: "Sign-up",
    description: "Who may create an account and an organization. Opening sign-up loosens access and asks for confirmation.",
    keys: ["auth.signup_mode", "auth.org_creation"],
  },
  {
    title: "Sessions",
    description:
      "How long a sign-in lasts. A shorter value applies to existing sessions at their next request; a longer one only to new sessions and keys.",
    keys: ["auth.session_idle_secs", "auth.session_absolute_secs", "auth.cli_key_ttl_days"],
  },
  {
    title: "Identity providers",
    description:
      "The OpenID Connect providers people sign in with. A provider defined in Helm stays available and is managed there; providers added here sit beside it. Saving applies at once, without a restart.",
    keys: [PROVIDERS],
  },
];

const hostOf = (v: unknown): string => (typeof v === "string" ? v : "");
const idOf = (e: Record<string, unknown>): string => (typeof e.id === "string" ? e.id : "");

function ProvidersControl({ ctx, signedInWith }: { ctx: SettingControlCtx; signedInWith: string | null }) {
  const s = ctx.setting;
  const helm = (s.entries ?? []).filter((e) => e.source === "helm");
  const own = ctx.own;
  const helmIds = helm.map((e) => e.id);
  const instanceProviders = useInstance().data?.oidc_providers ?? [];
  const [editing, setEditing] = useState<{ index: number | null } | null>(null);
  const [viewing, setViewing] = useState<Record<string, unknown> | null>(null);
  const errorsOf = (id: string): Record<string, string> => {
    const out: Record<string, string> = {};
    for (const [k, m] of ctx.entryErrors) {
      const rest = k.startsWith(`${PROVIDERS}/${id}/`) ? k.slice(`${PROVIDERS}/${id}/`.length) : null;
      if (rest) out[rest] = m;
    }
    return out;
  };
  const editingEntry = editing && editing.index !== null ? own[editing.index]! : null;
  function save(e: Record<string, unknown>) {
    const next = [...own];
    if (editing === null || editing.index === null) next.push(e);
    else next[editing.index] = e;
    ctx.setValue(PROVIDERS, next);
    setEditing(null);
  }
  const rows: ReactNode[] = [];
  for (const e of helm) {
    const v = e.value as Record<string, unknown>;
    rows.push(
      <ListRow
        key={`h-${e.id}`}
        actions={
          <Button type="button" size="sm" variant="outline" aria-label={`View ${e.id}`} onClick={() => setViewing(v)}>
            View
          </Button>
        }
      >
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-ink">{String(v.display_name ?? e.id)}</span>
            <LockPill owner="Set in Helm" />
            {signedInWith === e.id ? <Badge tone="accent">You signed in with this</Badge> : null}
          </div>
          <p className="break-all text-xs text-ink-2">{hostOf(v.issuer) || hostOf(v.issuer_url)}</p>
        </div>
      </ListRow>,
    );
  }
  // (P5-POLISH-10, E7) A provider people sign in with that is not a list entry: the development provider, which Helm
  // registers through `auth.dev_idp`. It is listed read-only, so "No provider yet" only appears when there is none.
  const known = new Set([...helm.map((e) => e.id), ...own.map(idOf)]);
  for (const p of instanceProviders.filter((p) => !known.has(p.id))) {
    rows.push(
      <ListRow key={`i-${p.id}`}>
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-ink">{p.id === "dev" ? "Development sign-in" : p.display_name}</span>
            <LockPill owner="Set in Helm" />
            {signedInWith === p.id ? <Badge tone="accent">You signed in with this</Badge> : null}
          </div>
          {p.id === "dev" ? <p className="text-xs text-ink-2">Signs in without a password. For development only.</p> : null}
        </div>
      </ListRow>,
    );
  }
  own.forEach((e, i) => {
    const id = idOf(e);
    const errs = errorsOf(id);
    const bad = Object.keys(errs).length > 0;
    const warns = (s.entries ?? []).find((x) => x.id === id)?.warnings ?? [];
    rows.push(
      <ListRow
        key={`d-${id}-${i}`}
        actions={
          <>
            <Button type="button" size="sm" variant="outline" aria-label={`Edit ${id}`} onClick={() => setEditing({ index: i })}>
              Edit
            </Button>
            <Button type="button" size="sm" variant="danger" aria-label={`Remove ${id}`} onClick={() => ctx.setValue(PROVIDERS, own.filter((_, j) => j !== i))}>
              Remove
            </Button>
          </>
        }
      >
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-ink">{hostOf(e.display_name) || id}</span>
            <Badge tone="accent">Dashboard</Badge>
            {signedInWith === id ? <Badge tone="accent">You signed in with this</Badge> : null}
            {bad ? <Badge tone="danger">Needs attention</Badge> : null}
          </div>
          <p className="break-all text-xs text-ink-2">{hostOf(e.issuer) || hostOf(e.issuer_url)}</p>
          {bad ? (
            <ul className="list-disc pl-5 text-xs text-danger-ink">
              {Object.entries(errs).map(([f, m]) => (
                <li key={f}>
                  <code className="font-mono">{f}</code>: {m}
                </li>
              ))}
            </ul>
          ) : null}
          {warns.length > 0 ? <p className="text-xs text-muted">{warns.join(" ")}</p> : null}
        </div>
      </ListRow>,
    );
  });
  return (
    <SettingField
      id={s.key}
      label="Providers"
      hint="At least one provider must stay available. Client secrets are sealed and write-only."
      source={own.length > 0 ? "dashboard" : "default"}
      error={ctx.error}
    >
      <div id={settingControlId(s.key)} className="space-y-2">
        <ItemList aria-label="Identity providers">
          {rows.length === 0 ? <ListRow>No provider yet. Add one so people can sign in.</ListRow> : rows}
        </ItemList>
        <Button type="button" variant="outline" size="sm" onClick={() => setEditing({ index: null })}>
          Add provider…
        </Button>
      </div>
      {editing ? (
        <ProviderDialog
          key={editing.index ?? "new"}
          existing={editingEntry}
          helmIds={helmIds}
          otherIds={own.filter((_, j) => j !== editing.index).map(idOf)}
          errors={editingEntry ? errorsOf(idOf(editingEntry)) : {}}
          signedInWith={signedInWith}
          onSave={save}
          onClose={() => setEditing(null)}
        />
      ) : null}
      {viewing ? <HelmProviderDialog entry={viewing} helmKey={s.helm_key} onClose={() => setViewing(null)} /> : null}
    </SettingField>
  );
}

/** Admin › Settings › Sign-in (SET-4, C-780): sign-up, sessions and identity providers. Replaces the Identity page. */
export function SignInSettingsView() {
  const q = useSettingsSection("sign_in");
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const section = q.data;
  const extra = (section.extra ?? {}) as { signed_in_with?: unknown };
  const signedInWith = typeof extra.signed_in_with === "string" ? extra.signed_in_with : null;
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        breadcrumb={[{ label: "Settings", href: "/admin/settings" }, { label: section.title }]}
        title={section.title}
        description={section.description}
      />
      <SettingsSectionForm
        key={section.generation}
        section={section}
        groups={SIGN_IN_GROUPS}
        onReload={() => void q.refetch()}
        controls={{ [PROVIDERS]: (ctx) => <ProvidersControl ctx={ctx} signedInWith={signedInWith} /> }}
      />
      {/* (P5-POLISH-10, E7) The operator's recovery text is the last thing on the page, closed. */}
      <details className="max-w-3xl rounded-card border border-line bg-surface px-4 py-3 text-sm text-ink-2">
        <summary className="cursor-pointer font-medium text-ink">If nobody can sign in</summary>
        <p className="mt-2">
          <code className="font-mono">auth.instance_admins</code> and any provider defined in Helm stay in force whatever is
          saved here, and <code className="font-mono">aurora-api settings reset sign_in</code> restores the defaults. See
          “Sign-in providers” in the operator guide.
        </p>
      </details>
    </div>
  );
}
