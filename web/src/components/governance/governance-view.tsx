"use client";

import Link from "next/link";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { ByoPanel } from "./byo-panel";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow } from "@/components/ui/section";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { errorMessage } from "@/lib/api/errors";
import {
  formFromDocument,
  formFromEffective,
  formToDocument,
  governanceIssues,
  isStale,
  issuesUnder,
  KEY_SCOPES,
  useBlockedHosts,
  useEffectiveGovernance,
  useGovernanceDoc,
  useInstanceGovernance,
  usePutGovernance,
  type EffectiveGovernance,
  type EndpointForm,
  type GovForm,
  type GovIssues,
} from "@/lib/api/governance";
import { useWorkspaces } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

type Tab = "models" | "egress" | "byo" | "compliance";
const TABS: { id: Tab; label: string }[] = [
  { id: "models", label: "Model access" },
  { id: "egress", label: "Egress" },
  { id: "byo", label: "BYO agents" },
  { id: "compliance", label: "Compliance" },
];

export function Locked() {
  return (
    <Badge className="ml-2 align-middle normal-case tracking-normal" title="Set by the instance operator">
      enforced by instance
    </Badge>
  );
}

function Errors({ messages }: { messages: string[] }) {
  if (messages.length === 0) return null;
  return (
    <ul role="alert" className="text-xs text-danger-ink">
      {messages.map((m, i) => (
        <li key={i}>{m}</li>
      ))}
    </ul>
  );
}

export function GovernanceView({ orgId, orgSlug, canEdit }: { orgId: string; orgSlug: string; canEdit: boolean }) {
  const eff = useEffectiveGovernance(orgId);
  const doc = useGovernanceDoc(orgId, canEdit);
  const instance = useInstanceGovernance(canEdit);
  if (eff.isPending || (canEdit && doc.isPending)) return <Loading />;
  if (eff.isError) return <ErrorState error={eff.error} onRetry={() => eff.refetch()} />;
  if (canEdit && doc.isError) return <ErrorState error={doc.error} onRetry={() => doc.refetch()} />;
  return (
    <Body
      key={canEdit ? doc.data?.revision : "ro"}
      orgId={orgId}
      orgSlug={orgSlug}
      eff={eff.data!}
      revision={doc.data?.revision ?? 0}
      initial={canEdit ? formFromDocument(doc.data?.document, eff.data!, instance.data) : formFromEffective(eff.data!)}
      canEdit={canEdit}
      onReload={() => void Promise.all([doc.refetch(), eff.refetch()])}
    />
  );
}

function Body({
  orgId,
  orgSlug,
  eff,
  revision,
  initial,
  canEdit,
  onReload,
}: {
  orgId: string;
  orgSlug: string;
  eff: EffectiveGovernance;
  revision: number;
  initial: GovForm;
  canEdit: boolean;
  onReload: () => void;
}) {
  const [tab, setTab] = useState<Tab>("models");
  const [form, setForm] = useState<GovForm>(initial);
  const [saved, setSaved] = useState(false);
  const put = usePutGovernance(orgId);
  const issues: GovIssues = governanceIssues(put.error);
  const stale = isStale(put.error);
  const patch = (p: Partial<GovForm>) => {
    setSaved(false);
    setForm((f) => ({ ...f, ...p }));
  };
  const save = () =>
    put.mutate({ document: formToDocument(form), revision }, { onSuccess: () => setSaved(true) });

  return (
    <div className="max-w-[880px] space-y-4">
      <PageHeader
        title="Governance"
        description={
          canEdit
            ? "Model access, egress and BYO rules for the whole organization."
            : "These rules apply to your organization. Only owners and admins can change them."
        }
        className="mb-2"
      />
      <Tabs
        items={TABS.map((t) => ({
          value: t.id,
          label: `${t.label}${t.id === "compliance" && eff.non_compliant_secrets.length > 0 ? ` (${eff.non_compliant_secrets.length})` : ""}`,
        }))}
        value={tab}
        onValueChange={(v) => setTab(v as Tab)}
        variant="underline"
        label="Governance"
        idBase="gov"
      />
      <div role="tabpanel" id={tabPanelId("gov", tab)} aria-labelledby={tabId("gov", tab)} className="space-y-5 pt-2">
        {tab === "models" ? <ModelsTab form={form} patch={patch} issues={issues} readOnly={!canEdit} /> : null}
        {tab === "egress" ? (
          <EgressTab form={form} patch={patch} issues={issues} readOnly={!canEdit} eff={eff} orgId={orgId} />
        ) : null}
        {tab === "byo" ? <ByoPanel form={form} patch={patch} issues={issues} readOnly={!canEdit} eff={eff} /> : null}
        {tab === "compliance" ? <ComplianceTab eff={eff} orgId={orgId} orgSlug={orgSlug} /> : null}
      </div>
      {canEdit && tab !== "compliance" ? (
        <div className="space-y-3 border-t border-line pt-4">
          {stale ? (
            <Notice tone="pending" role="alert">
              The governance settings were changed by someone else. Reload to see the latest version.{" "}
              <Button size="sm" variant="outline" onClick={() => { put.reset(); onReload(); }}>
                Reload latest
              </Button>
            </Notice>
          ) : put.isError ? (
            <Notice tone="danger" role="alert">
              {errorMessage(put.error)}
              <Errors messages={issues.general} />
            </Notice>
          ) : null}
          {saved ? <Notice tone="accent" role="status">Saved. Existing keys were re-checked against the new rules.</Notice> : null}
          <FormActions>
            <Button size="lg" onClick={save} disabled={put.isPending}>
              {put.isPending ? "Saving…" : "Save governance"}
            </Button>
          </FormActions>
        </div>
      ) : null}
    </div>
  );
}

function PostureSelect<T extends string>({
  label, value, options, locked, readOnly, onChange,
}: {
  label: string; value: T; options: { value: T; label: string }[]; locked: boolean; readOnly: boolean; onChange: (v: T) => void;
}) {
  return (
    <label className="block text-sm font-medium text-ink">
      <span>{label}{locked ? <Locked /> : null}</span>
      <select className={cn(inputClass, "mt-1.5 max-w-lg")} value={value} disabled={locked || readOnly} onChange={(e) => onChange(e.target.value as T)}>
        {options.map((o) => (
          <option key={o.value} value={o.value}>{o.label}</option>
        ))}
      </select>
    </label>
  );
}

function ModelsTab({ form, patch, issues, readOnly }: { form: GovForm; patch: (p: Partial<GovForm>) => void; issues: GovIssues; readOnly: boolean }) {
  const setEp = (i: number, p: Partial<EndpointForm>) =>
    patch({ endpoints: form.endpoints.map((e, j) => (j === i ? { ...e, ...p } : e)) });
  return (
    <>
      <PostureSelect
        label="LLM posture"
        value={form.llmPosture}
        locked={form.llmPostureLocked}
        readOnly={readOnly}
        onChange={(v) => patch({ llmPosture: v })}
        options={[
          { value: "open", label: "Open: agents may also use their own LLM credentials" },
          { value: "sanctioned_only", label: "Sanctioned only: only Aurora-held keys" },
        ]}
      />
      <Errors messages={issuesUnder(issues, "/llm/posture")} />
      {form.endpoints.length === 0 ? <p className="text-sm text-ink-2">No LLM endpoints are configured on this instance.</p> : null}
      {form.endpoints.map((e, i) => {
        const base = `/llm/endpoints/${e.id}`;
        const dis = readOnly;
        return (
          <fieldset key={e.id} className="min-w-0 space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
            <legend className="px-1 font-display text-base font-semibold text-ink">
              {e.display_name} <span className="font-mono text-xs font-normal text-muted">{e.id}</span>
            </legend>
            <label className="flex items-center gap-2 text-sm font-medium text-ink">
              <input type="checkbox" className="size-4 accent-accent" checked={e.allowed} disabled={dis || e.locked.permitted} onChange={(ev) => setEp(i, { allowed: ev.target.checked })} />
              Permitted
              {e.locked.permitted ? <Locked /> : null}
            </label>
            <Errors messages={issuesUnder(issues, `${base}/allowed`)} />
            <div role="group" aria-label={`Key scopes for ${e.display_name}`} className="text-sm text-ink">
              <span className="font-medium">Key scopes</span>
              {e.locked.key_scopes ? <Locked /> : null}
              <div className="mt-1.5 flex flex-wrap gap-x-4 gap-y-1">
                {KEY_SCOPES.map((s) => (
                  <label key={s} className="flex items-center gap-1.5">
                    <input
                      type="checkbox"
                      className="size-4 accent-accent"
                      checked={e.key_scopes.includes(s)}
                      disabled={dis || e.locked.key_scopes}
                      onChange={(ev) => setEp(i, { key_scopes: ev.target.checked ? [...e.key_scopes, s] : e.key_scopes.filter((x) => x !== s) })}
                    />
                    {s}
                  </label>
                ))}
              </div>
            </div>
            <Errors messages={issuesUnder(issues, `${base}/key_scopes`)} />
            <TextArea id={`${e.id}-models`} label="Allowed models" hint="One per line. Empty means the instance list (or any)." value={e.allowed_models} locked={e.locked.allowed_models} readOnly={dis} onChange={(v) => setEp(i, { allowed_models: v })} errors={issuesUnder(issues, `${base}/allowed_models`)} />
            <div className="grid gap-3 sm:grid-cols-3">
              <TextArea id={`${e.id}-orgs`} label="Origin: organization ids" value={e.org_ids} readOnly={dis} onChange={(v) => setEp(i, { org_ids: v })} errors={issuesUnder(issues, `${base}/origin/allowed_organization_ids`)} />
              <TextArea id={`${e.id}-wss`} label="Origin: workspace ids" value={e.workspace_ids} readOnly={dis} onChange={(v) => setEp(i, { workspace_ids: v })} errors={issuesUnder(issues, `${base}/origin/allowed_workspace_ids`)} />
              <TextArea id={`${e.id}-teams`} label="Origin: team ids" value={e.team_ids} readOnly={dis} onChange={(v) => setEp(i, { team_ids: v })} errors={issuesUnder(issues, `${base}/origin/allowed_team_ids`)} />
            </div>
            <Errors messages={issuesUnder(issues, `${base}/origin`).filter((m) => !/^allowed_/.test(m))} />
          </fieldset>
        );
      })}
    </>
  );
}

function TextArea({ id, label, hint, value, locked, readOnly, onChange, errors }: {
  id: string; label: string; hint?: string; value: string; locked?: boolean; readOnly: boolean; onChange: (v: string) => void; errors: string[];
}) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={id} className="block text-sm font-medium text-ink">
        {label}{locked ? <Locked /> : null}
      </label>
      <textarea id={id} rows={3} className={cn(inputClass, "font-mono")} value={value} disabled={readOnly || locked} onChange={(e) => onChange(e.target.value)} aria-invalid={errors.length > 0} />
      {hint ? <p className="text-xs text-muted">{hint}</p> : null}
      <Errors messages={errors} />
    </div>
  );
}

function EgressTab({ form, patch, issues, readOnly, eff, orgId }: {
  form: GovForm; patch: (p: Partial<GovForm>) => void; issues: GovIssues; readOnly: boolean; eff: EffectiveGovernance; orgId: string;
}) {
  const workspaces = useWorkspaces(orgId);
  const blocked = useBlockedHosts((workspaces.data?.items ?? []).map((w) => w.id), true);
  const setRow = (i: number, p: Partial<{ host: string; path: string }>) =>
    patch({ allowlist: form.allowlist.map((r, j) => (j === i ? { ...r, ...p } : r)) });
  const listed = new Set(form.allowlist.map((r) => r.host));
  return (
    <>
      <PostureSelect
        label="Egress posture"
        value={form.egressPosture}
        locked={form.egressPostureLocked}
        readOnly={readOnly}
        onChange={(v) => patch({ egressPosture: v })}
        options={[
          { value: "open", label: "Open: agents may reach hosts without a credential" },
          { value: "credentialed_only", label: "Credentialed only: only hosts covered by a credential or the allow-list" },
        ]}
      />
      <Errors messages={issuesUnder(issues, "/egress/posture")} />
      <PostureSelect
        label="Browser tunnels (open lane)"
        value={form.openLane}
        locked={form.openLaneLocked}
        readOnly={readOnly}
        onChange={(v) => patch({ openLane: v })}
        options={[
          { value: "enabled", label: "Enabled: browsers may open tunnels" },
          { value: "allowlist_only", label: "Allow-list only: tunnels reach only allow-listed hosts" },
          { value: "disabled", label: "Disabled: no browser tunnels" },
        ]}
      />
      <p className="text-xs text-muted">
        Tunnels cannot be inspected or given credentials; <code className="font-mono">allowlist_only</code> limits them to the allow-list.
      </p>
      <Errors messages={issuesUnder(issues, "/egress/open_lane")} />
      <section aria-label="Organization allow-list" className="space-y-3">
        <h2 className="font-display text-base font-semibold text-ink">Organization allow-list</h2>
        {form.allowlist.length === 0 ? <p className="text-sm text-ink-2">No hosts allowed beyond the instance baseline.</p> : null}
        {form.allowlist.map((r, i) => (
          <div key={i} className="space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <input aria-label={`Host ${i + 1}`} className={cn(inputClass, "min-w-0 flex-1 basis-48")} placeholder="host, e.g. *.example.com" value={r.host} disabled={readOnly} onChange={(e) => setRow(i, { host: e.target.value })} />
              <input aria-label={`Path ${i + 1}`} className={cn(inputClass, "min-w-0 flex-1 basis-48")} placeholder="path pattern (optional)" value={r.path} disabled={readOnly} onChange={(e) => setRow(i, { path: e.target.value })} />
              {!readOnly ? (
                <Button variant="ghost" size="sm" onClick={() => patch({ allowlist: form.allowlist.filter((_, j) => j !== i) })}>
                  Remove
                </Button>
              ) : null}
            </div>
            <Errors messages={issuesUnder(issues, `/egress/allowlist/${i}`)} />
          </div>
        ))}
        <Errors messages={issuesUnder(issues, "/egress/allowlist").filter((m) => !/^\d+/.test(m))} />
        {!readOnly ? (
          <Button variant="outline" size="sm" onClick={() => patch({ allowlist: [...form.allowlist, { host: "", path: "" }] })}>
            Add host
          </Button>
        ) : null}
      </section>
      {blocked.length > 0 ? (
        <section aria-label="Recently blocked hosts" className="space-y-3">
          <h2 className="font-display text-base font-semibold text-ink">Recently blocked hosts</h2>
          <ItemList aria-label="Recently blocked hosts">
            {blocked.map((b) => (
              <ListRow
                key={b.host}
                actions={
                  !readOnly && !listed.has(b.host) ? (
                    <Button variant="ghost" size="sm" onClick={() => patch({ allowlist: [...form.allowlist, { host: b.host, path: "" }] })}>
                      Add to allow-list
                    </Button>
                  ) : null
                }
              >
                <span className="font-mono text-sm text-ink">{b.host}</span>
                <span className="text-xs text-muted">{b.count}x</span>
              </ListRow>
            ))}
          </ItemList>
        </section>
      ) : null}
      <section aria-label="Instance baseline" className="space-y-3">
        <h2 className="font-display text-base font-semibold text-ink">Instance baseline<Locked /></h2>
        {eff.egress.baseline_allowlist.length === 0 ? (
          <p className="text-sm text-ink-2">The instance has no baseline allow-list.</p>
        ) : (
          <ul className="rounded-card border border-line bg-surface px-4 py-3 font-mono text-sm text-ink shadow-card">
            {eff.egress.baseline_allowlist.map((e, i) => (
              <li key={i} className="break-all">{e.host}{e.path ?? ""}</li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}

const REASONS: Record<string, string> = {
  mismatch: "Key belongs to an account not allowed by governance",
  unverifiable: "Key origin could not be verified",
  not_permitted: "Endpoint or scope no longer permitted",
  pending: "Verification pending",
};

function ComplianceTab({ eff, orgId, orgSlug }: { eff: EffectiveGovernance; orgId: string; orgSlug: string }) {
  const workspaces = useWorkspaces(orgId);
  const slugOf = new Map((workspaces.data?.items ?? []).map((w) => [w.id, w.slug]));
  if (eff.non_compliant_secrets.length === 0)
    return (
      <Notice tone="accent" role="status">
        All LLM keys comply with the current rules.
      </Notice>
    );
  return (
    <ItemList aria-label="Non-compliant LLM keys">
      {eff.non_compliant_secrets.map((s) => {
        const ws = s.workspace_id ? slugOf.get(s.workspace_id) : null;
        const href = s.workspace_id ? (ws ? `/o/${orgSlug}/w/${ws}/credentials` : null) : `/o/${orgSlug}/credentials`;
        return (
          <ListRow
            key={s.id}
            actions={
              href ? (
                <Link className="text-sm font-medium text-link underline" href={href}>
                  Fix
                </Link>
              ) : null
            }
          >
            <span className="text-sm">
              <span className="font-medium text-ink">{s.name}</span>{" "}
              <span className="text-ink-2">{REASONS[s.reason] ?? s.reason}</span>
            </span>
          </ListRow>
        );
      })}
    </ItemList>
  );
}
