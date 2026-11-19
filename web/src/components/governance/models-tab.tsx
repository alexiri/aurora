"use client";

import { Errors, Locked, PostureSelect, TextArea } from "@/components/governance/controls";
import { Switch } from "@/components/ui/switch";
import { KEY_SCOPES, issuesUnder, type EndpointForm, type GovForm, type GovIssues } from "@/lib/api/governance";

/** The organization's model rules (C-966): the AI key posture and, for each endpoint, what it permits. */
export function ModelsTab({ form, patch, issues, readOnly }: { form: GovForm; patch: (p: Partial<GovForm>) => void; issues: GovIssues; readOnly: boolean }) {
  const setEp = (i: number, p: Partial<EndpointForm>) =>
    patch({ endpoints: form.endpoints.map((e, j) => (j === i ? { ...e, ...p } : e)) });
  return (
    <>
      <PostureSelect
        label="LLM posture"
        value={form.llmPosture}
        locked={form.llmPostureLocked}
        setting="governance.llm.minimum_posture"
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
              <Switch checked={e.allowed} disabled={dis || e.locked.permitted} onCheckedChange={(v) => setEp(i, { allowed: v })} />
              Permitted
              {e.locked.permitted ? <Locked /> : null}
            </label>
            <Errors messages={issuesUnder(issues, `${base}/allowed`)} />
            <div role="group" aria-label={`Key scopes for ${e.display_name}`} className="text-sm text-ink">
              <span className="font-medium">Key scopes</span>
              {e.locked.key_scopes ? <Locked setting="governance.llm.allowed_key_scopes" /> : null}
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
