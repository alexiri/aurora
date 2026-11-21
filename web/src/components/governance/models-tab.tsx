"use client";

import { Errors, Locked, PostureSelect, TextArea } from "@/components/governance/controls";
import { IdPicker, joinIds, splitIds, type PickOption } from "@/components/governance/id-picker";
import { Switch } from "@/components/ui/switch";
import { useAccounts } from "@/lib/api/accounts";
import { issuesUnder, type EndpointForm, type GovForm, type GovIssues } from "@/lib/api/governance";
import { useGroups } from "@/lib/api/groups";
import { useMe, useWorkspaces, type Org } from "@/lib/api/queries";
import { peopleMayAddOwnKey } from "@/lib/policy-presets";

/**
 * The names of the organization's own AI keys, the starter Company key included: from the organization's accounts list
 * (owners and admins), else, for a member (403), from a workspace's list where they carry `scope: "organization"`. The
 * first workspace is no one's to rely on (an admin may not be a member of it), so it is only the fallback.
 */
export function useOrgKeyNames(orgId: string): { names: string[]; pending: boolean } {
  const orgList = useAccounts({ kind: "organization", id: orgId });
  const workspaces = useWorkspaces(orgId);
  const first = workspaces.data?.items[0]?.id;
  const fallback = orgList.isError;
  const accounts = useAccounts({ kind: "workspace", id: first ?? "" }, fallback && !!first);
  const items = fallback ? (accounts.data?.items ?? []) : (orgList.data?.items ?? []);
  const names = items.filter((a) => a.kind === "ai_key" && a.scope === "organization").map((a) => a.name);
  return { names, pending: fallback ? workspaces.isPending || (!!first && accounts.isPending) : orgList.isPending };
}

const listWords = (xs: string[]) => (xs.length <= 1 ? (xs[0] ?? "") : `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`);

/** "Agents in Acme use **Company key**. People may add their own." — from the form, so it never disagrees with the switches. */
export function modelsSummarySentence(orgName: string, keyNames: string[], peopleMayAdd: boolean): string {
  const use = keyNames.length === 0 ? "keys added in a workspace" : listWords(keyNames);
  return `Agents in ${orgName} use ${use}. ${peopleMayAdd ? "People may add their own." : "People may not add their own."}`;
}

/** The key scopes of an endpoint in words, from the same value the switches show. */
export function keyScopesWords(scopes: string[]): string {
  if (scopes.length === 0) return "Nobody may add keys";
  const w = scopes.map((s) => (s === "organization" ? "the organization" : "workspaces"));
  return `Keys may be added by ${listWords(w)}`;
}

/**
 * The organization's model rules (C-966, P5-REFINE-17 org-7): the summary and three plain rows first (which key agents
 * use, whether people may add their own, who can use it), then the endpoints and the advanced settings collapsed. Origin
 * settings are pick lists of names, never typed ids.
 */
export function ModelsTab({
  org,
  form,
  patch,
  issues,
  readOnly,
}: {
  org: Org;
  form: GovForm;
  patch: (p: Partial<GovForm>) => void;
  issues: GovIssues;
  readOnly: boolean;
}) {
  const me = useMe();
  const workspaces = useWorkspaces(org.id);
  const groups = useGroups(org.id);
  const keys = useOrgKeyNames(org.id);
  const setEp = (i: number, p: Partial<EndpointForm>) =>
    patch({ endpoints: form.endpoints.map((e, j) => (j === i ? { ...e, ...p } : e)) });
  const setAll = (fn: (e: EndpointForm) => Partial<EndpointForm>) => patch({ endpoints: form.endpoints.map((e) => ({ ...e, ...fn(e) })) });

  const orgOptions: PickOption[] = (me.data?.memberships ?? []).flatMap((m) => (m.org.id ? [{ id: m.org.id, label: m.org.name ?? m.org.slug }] : []));
  const wsOptions: PickOption[] = (workspaces.data?.items ?? []).map((w) => ({ id: w.id, label: w.personal_of ? `${w.personal_of.name} (personal)` : w.name }));
  const teamOptions: PickOption[] = (groups.data?.items ?? []).map((g) => ({ id: g.id, label: g.name }));

  const live = form.endpoints.filter((e) => e.allowed);
  // The one reading of "people may add their own key": the switch, the sentence and the preset card all use it.
  const peopleMayAdd = peopleMayAddOwnKey(form.endpoints) === true;
  const lead = form.endpoints[0];
  const chosenTeams = lead ? splitIds(lead.team_ids) : [];
  const chosenWs = lead ? splitIds(lead.workspace_ids) : [];
  const everyone = chosenTeams.length === 0 && chosenWs.length === 0;
  const nameOf = (id: string, opts: PickOption[]) => opts.find((o) => o.id === id)?.label ?? `Unknown (${id.slice(0, 8)})`;
  const addUse = (kind: "team_ids" | "workspace_ids", id: string) =>
    setAll((e) => (e.locked.permitted ? {} : { [kind]: joinIds([...splitIds(e[kind]), id]) }));
  const removeUse = (kind: "team_ids" | "workspace_ids", id: string) =>
    setAll((e) => ({ [kind]: joinIds(splitIds(e[kind]).filter((x) => x !== id)) }));
  const addable = [
    ...teamOptions.filter((o) => !chosenTeams.includes(o.id)).map((o) => ({ ...o, kind: "team_ids" as const, text: `Team: ${o.label}` })),
    ...wsOptions.filter((o) => !chosenWs.includes(o.id)).map((o) => ({ ...o, kind: "workspace_ids" as const, text: `Workspace: ${o.label}` })),
  ];

  return (
    <>
      <p className="text-base text-ink" data-testid="models-summary">
        {modelsSummarySentence(org.name, keys.names, peopleMayAdd)}
      </p>
      <dl className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface text-sm shadow-card">
        <div className="px-4 py-3">
          <dt className="font-medium text-ink">Key agents use</dt>
          <dd className="text-ink-2">Used unless someone picks another</dd>
          <dd className="mt-0.5 text-ink">{keys.names.length === 0 ? "No company key yet" : listWords(keys.names)}</dd>
        </div>
        {/* (axe definition-list) Each row is one div directly under the dl, holding only dt and dd; the switch sits in the dd. */}
        <div className="px-4 py-3">
          <dt className="font-medium text-ink">People may add their own key</dt>
          <dd className="flex items-center justify-between gap-3 text-ink-2">
            <span>{peopleMayAdd ? "Allowed" : "Not allowed"}</span>
            <Switch
              aria-label="People may add their own key"
              checked={peopleMayAdd}
              disabled={readOnly || live.length === 0}
              onCheckedChange={(v) =>
                setAll((e) =>
                  e.locked.key_scopes || !e.allowed ? {} : { key_scopes: v ? [...new Set([...e.key_scopes, "workspace"])] : e.key_scopes.filter((s) => s !== "workspace") },
                )
              }
            />
          </dd>
        </div>
        <div className="px-4 py-3">
          <dt className="font-medium text-ink">Who can use {keys.names[0] ?? "the company key"}</dt>
          <dd className="mt-2 space-y-2">
            <ul aria-label="Who can use the key" className="flex flex-wrap gap-1.5">
              {everyone ? <li className="text-ink-2">Everyone in {org.name}</li> : null}
              {chosenTeams.map((id) => (
                <li key={`t${id}`} className="inline-flex items-center gap-1 rounded-full bg-subtle px-2.5 py-0.5 text-ink">
                  {nameOf(id, teamOptions)}
                  {readOnly ? null : (
                    <button type="button" aria-label={`Remove ${nameOf(id, teamOptions)}`} className="px-1 text-ink-2" onClick={() => removeUse("team_ids", id)}>
                      ×
                    </button>
                  )}
                </li>
              ))}
              {chosenWs.map((id) => (
                <li key={`w${id}`} className="inline-flex items-center gap-1 rounded-full bg-subtle px-2.5 py-0.5 text-ink">
                  {nameOf(id, wsOptions)}
                  {readOnly ? null : (
                    <button type="button" aria-label={`Remove ${nameOf(id, wsOptions)}`} className="px-1 text-ink-2" onClick={() => removeUse("workspace_ids", id)}>
                      ×
                    </button>
                  )}
                </li>
              ))}
            </ul>
          {readOnly || addable.length === 0 ? null : (
            <select
              aria-label="Add team or workspace"
              className="h-9 rounded-control border border-line bg-surface px-2 text-sm text-ink"
              value=""
              onChange={(ev) => {
                const pick = addable.find((o) => `${o.kind}:${o.id}` === ev.target.value);
                if (pick) addUse(pick.kind, pick.id);
              }}
            >
              <option value="">Add team or workspace</option>
              {addable.map((o) => (
                <option key={`${o.kind}:${o.id}`} value={`${o.kind}:${o.id}`}>
                  {o.text}
                </option>
              ))}
            </select>
          )}
          </dd>
        </div>
      </dl>

      <details className="rounded-card border border-line bg-surface shadow-card" data-testid="models-endpoints">
        <summary className="cursor-pointer px-4 py-3 font-display text-base font-semibold text-ink">Endpoints ({form.endpoints.length})</summary>
        <div className="space-y-3 border-t border-line p-4">
          {form.endpoints.length === 0 ? <p className="text-sm text-ink-2">No LLM endpoints are configured on this instance.</p> : null}
          {form.endpoints.map((e, i) => {
            const base = `/llm/endpoints/${e.id}`;
            const dis = readOnly;
            return (
              <details key={e.id} className="min-w-0 rounded-control border border-line px-3 py-2">
                <summary className="cursor-pointer font-medium text-ink">
                  {e.display_name}
                  <span className="ml-2 font-normal text-ink-2">{e.allowed ? "Permitted" : "Not permitted"}</span>
                </summary>
                <div className="mt-3 space-y-4">
                  <label className="flex items-center gap-2 text-sm font-medium text-ink">
                    <Switch checked={e.allowed} disabled={dis || e.locked.permitted} onCheckedChange={(v) => setEp(i, { allowed: v })} />
                    Permitted
                    {e.locked.permitted ? <Locked /> : null}
                  </label>
                  <Errors messages={issuesUnder(issues, `${base}/allowed`)} />
                  <div role="group" aria-label={`Key scopes for ${e.display_name}`} className="space-y-1.5 text-sm text-ink">
                    <span className="font-medium">Key scopes</span>
                    {e.locked.key_scopes ? <Locked setting="governance.llm.allowed_key_scopes" /> : null}
                    <p className="text-ink-2">{keyScopesWords(e.key_scopes)}</p>
                    <label className="flex items-center gap-2">
                      <Switch
                        checked={e.key_scopes.includes("organization")}
                        disabled={dis || e.locked.key_scopes}
                        onCheckedChange={(v) => setEp(i, { key_scopes: v ? [...e.key_scopes, "organization"] : e.key_scopes.filter((x) => x !== "organization") })}
                      />
                      Organization keys
                    </label>
                    <label className="flex items-center gap-2">
                      <Switch
                        checked={e.key_scopes.includes("workspace")}
                        disabled={dis || e.locked.key_scopes}
                        onCheckedChange={(v) => setEp(i, { key_scopes: v ? [...e.key_scopes, "workspace"] : e.key_scopes.filter((x) => x !== "workspace") })}
                      />
                      Workspace keys
                    </label>
                  </div>
                  <Errors messages={issuesUnder(issues, `${base}/key_scopes`)} />
                  <TextArea id={`${e.id}-models`} label="Allowed models" hint="One per line. Empty means the instance list (or any)." value={e.allowed_models} locked={e.locked.allowed_models} readOnly={dis} onChange={(v) => setEp(i, { allowed_models: v })} errors={issuesUnder(issues, `${base}/allowed_models`)} />
                </div>
              </details>
            );
          })}
        </div>
      </details>

      <details className="rounded-card border border-line bg-surface shadow-card" data-testid="models-advanced">
        <summary className="cursor-pointer px-4 py-3 font-display text-base font-semibold text-ink">Advanced</summary>
        <div className="space-y-4 border-t border-line p-4">
          <PostureSelect
            label="Which AI keys agents may use"
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
          {form.endpoints.map((e, i) => {
            const base = `/llm/endpoints/${e.id}`;
            return (
              <div key={e.id} className="space-y-3">
                <p className="font-medium text-ink">Who can use {e.display_name}</p>
                <div className="grid gap-3 sm:grid-cols-3">
                  <IdPicker id={`${e.id}-orgs`} label="Organizations" value={e.org_ids} options={orgOptions} readOnly={readOnly} onChange={(v) => setEp(i, { org_ids: v })} />
                  <IdPicker id={`${e.id}-wss`} label="Workspaces" value={e.workspace_ids} options={wsOptions} readOnly={readOnly} onChange={(v) => setEp(i, { workspace_ids: v })} />
                  <IdPicker id={`${e.id}-teams`} label="Teams" value={e.team_ids} options={teamOptions} readOnly={readOnly} onChange={(v) => setEp(i, { team_ids: v })} />
                </div>
                <Errors messages={issuesUnder(issues, `${base}/origin`)} />
              </div>
            );
          })}
        </div>
      </details>
    </>
  );
}
