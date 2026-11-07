"use client";

import Link from "next/link";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
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

type Tab = "models" | "egress" | "compliance";
const TABS: { id: Tab; label: string }[] = [
  { id: "models", label: "Model access" },
  { id: "egress", label: "Egress" },
  { id: "compliance", label: "Compliance" },
];

export function Locked() {
  return (
    <span className="ml-2 rounded bg-neutral-100 px-1.5 py-0.5 text-xs font-normal text-neutral-700" title="Set by the instance operator">
      enforced by instance
    </span>
  );
}

function Errors({ messages }: { messages: string[] }) {
  if (messages.length === 0) return null;
  return (
    <ul role="alert" className="text-xs text-red-700">
      {messages.map((m, i) => (
        <li key={i}>{m}</li>
      ))}
    </ul>
  );
}

export function GovernanceView({ orgId, orgSlug, canEdit }: { orgId: string; orgSlug: string; canEdit: boolean }) {
  const eff = useEffectiveGovernance(orgId);
  const doc = useGovernanceDoc(orgId, canEdit);
  useInstanceGovernance(canEdit);
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
      initial={canEdit ? formFromDocument(doc.data?.document, eff.data!) : formFromEffective(eff.data!)}
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
    <div className="max-w-4xl space-y-4">
      <h1 className="text-xl font-semibold">Governance</h1>
      {!canEdit ? (
        <p className="text-sm text-neutral-600">These rules apply to your organization. Only owners and admins can change them.</p>
      ) : null}
      <div role="tablist" aria-label="Governance" className="flex gap-1 border-b border-neutral-200">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            type="button"
            id={`tab-${t.id}`}
            aria-selected={tab === t.id}
            aria-controls="gov-panel"
            onClick={() => setTab(t.id)}
            className={cn("-mb-px border-b-2 px-3 py-2 text-sm", tab === t.id ? "border-neutral-900 font-medium" : "border-transparent")}
          >
            {t.label}
            {t.id === "compliance" && eff.non_compliant_secrets.length > 0 ? ` (${eff.non_compliant_secrets.length})` : ""}
          </button>
        ))}
      </div>
      <div role="tabpanel" id="gov-panel" aria-labelledby={`tab-${tab}`} className="space-y-4 pt-2">
        {tab === "models" ? <ModelsTab form={form} patch={patch} issues={issues} readOnly={!canEdit} /> : null}
        {tab === "egress" ? (
          <EgressTab form={form} patch={patch} issues={issues} readOnly={!canEdit} eff={eff} orgId={orgId} />
        ) : null}
        {tab === "compliance" ? <ComplianceTab eff={eff} orgId={orgId} orgSlug={orgSlug} /> : null}
      </div>
      {canEdit && tab !== "compliance" ? (
        <div className="space-y-2 border-t border-neutral-200 pt-3">
          {stale ? (
            <div role="alert" className="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900">
              The governance settings were changed by someone else. Reload to see the latest version.{" "}
              <Button size="sm" variant="outline" onClick={() => { put.reset(); onReload(); }}>
                Reload latest
              </Button>
            </div>
          ) : put.isError ? (
            <div role="alert" className="text-sm text-red-700">
              {errorMessage(put.error)}
              <Errors messages={issues.general} />
            </div>
          ) : null}
          {saved ? <p role="status" className="text-sm text-green-700">Saved. Existing keys were re-checked against the new rules.</p> : null}
          <Button onClick={save} disabled={put.isPending}>
            {put.isPending ? "Saving…" : "Save governance"}
          </Button>
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
    <label className="block text-sm font-medium">
      <span>{label}{locked ? <Locked /> : null}</span>
      <select className={cn(inputClass, "mt-1 max-w-xs")} value={value} disabled={locked || readOnly} onChange={(e) => onChange(e.target.value as T)}>
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
      {form.endpoints.length === 0 ? <p className="text-sm text-neutral-600">No LLM endpoints are configured on this instance.</p> : null}
      {form.endpoints.map((e, i) => {
        const base = `/llm/endpoints/${e.id}`;
        const dis = readOnly;
        return (
          <fieldset key={e.id} className="space-y-3 rounded-md border border-neutral-200 p-4">
            <legend className="px-1 text-sm font-medium">
              {e.display_name} <span className="font-mono text-xs text-neutral-500">{e.id}</span>
            </legend>
            <label className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={e.allowed} disabled={dis || e.locked.permitted} onChange={(ev) => setEp(i, { allowed: ev.target.checked })} />
              Permitted
              {e.locked.permitted ? <Locked /> : null}
            </label>
            <Errors messages={issuesUnder(issues, `${base}/allowed`)} />
            <div role="group" aria-label={`Key scopes for ${e.display_name}`} className="text-sm">
              <span className="font-medium">Key scopes</span>
              {e.locked.key_scopes ? <Locked /> : null}
              <div className="mt-1 flex gap-4">
                {KEY_SCOPES.map((s) => (
                  <label key={s} className="flex items-center gap-1">
                    <input
                      type="checkbox"
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
    <div className="space-y-1">
      <label htmlFor={id} className="block text-sm font-medium">
        {label}{locked ? <Locked /> : null}
      </label>
      <textarea id={id} rows={3} className={cn(inputClass, "font-mono")} value={value} disabled={readOnly || locked} onChange={(e) => onChange(e.target.value)} aria-invalid={errors.length > 0} />
      {hint ? <p className="text-xs text-neutral-500">{hint}</p> : null}
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
      <p className="text-xs text-neutral-600">
        Tunnels cannot be inspected or given credentials; <code>allowlist_only</code> limits them to the allow-list.
      </p>
      <Errors messages={issuesUnder(issues, "/egress/open_lane")} />
      <section aria-label="Organization allow-list" className="space-y-2">
        <h2 className="text-sm font-medium">Organization allow-list</h2>
        {form.allowlist.length === 0 ? <p className="text-sm text-neutral-600">No hosts allowed beyond the instance baseline.</p> : null}
        {form.allowlist.map((r, i) => (
          <div key={i} className="space-y-1">
            <div className="flex items-center gap-2">
              <input aria-label={`Host ${i + 1}`} className={inputClass} placeholder="host, e.g. *.example.com" value={r.host} disabled={readOnly} onChange={(e) => setRow(i, { host: e.target.value })} />
              <input aria-label={`Path ${i + 1}`} className={inputClass} placeholder="path pattern (optional)" value={r.path} disabled={readOnly} onChange={(e) => setRow(i, { path: e.target.value })} />
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
        <section aria-label="Recently blocked hosts" className="space-y-2">
          <h2 className="text-sm font-medium">Recently blocked hosts</h2>
          <ul className="divide-y divide-neutral-100 rounded-md border border-neutral-200 text-sm">
            {blocked.map((b) => (
              <li key={b.host} className="flex items-center justify-between px-3 py-1.5">
                <span><span className="font-mono">{b.host}</span> <span className="text-xs text-neutral-500">{b.count}x</span></span>
                {!readOnly && !listed.has(b.host) ? (
                  <Button variant="ghost" size="sm" onClick={() => patch({ allowlist: [...form.allowlist, { host: b.host, path: "" }] })}>
                    Add to allow-list
                  </Button>
                ) : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      <section aria-label="Instance baseline" className="space-y-2">
        <h2 className="text-sm font-medium">Instance baseline<Locked /></h2>
        {eff.egress.baseline_allowlist.length === 0 ? (
          <p className="text-sm text-neutral-600">The instance has no baseline allow-list.</p>
        ) : (
          <ul className="rounded-md border border-neutral-200 px-3 py-2 font-mono text-sm">
            {eff.egress.baseline_allowlist.map((e, i) => (
              <li key={i}>{e.host}{e.path ?? ""}</li>
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
    return <p className="text-sm text-neutral-600">All LLM keys comply with the current rules.</p>;
  return (
    <ul aria-label="Non-compliant LLM keys" className="divide-y divide-neutral-100 rounded-md border border-neutral-200 text-sm">
      {eff.non_compliant_secrets.map((s) => {
        const ws = s.workspace_id ? slugOf.get(s.workspace_id) : null;
        const href = s.workspace_id ? (ws ? `/o/${orgSlug}/w/${ws}/credentials` : null) : `/o/${orgSlug}/credentials`;
        return (
          <li key={s.id} className="flex items-center justify-between px-3 py-2">
            <span>
              <span className="font-medium">{s.name}</span>{" "}
              <span className="text-neutral-600">{REASONS[s.reason] ?? s.reason}</span>
            </span>
            {href ? <Link className="underline" href={href}>Fix</Link> : null}
          </li>
        );
      })}
    </ul>
  );
}
