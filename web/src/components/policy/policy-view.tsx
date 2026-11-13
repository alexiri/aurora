"use client";

import Link from "next/link";
import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { errorMessage } from "@/lib/api/errors";
import { useGroups } from "@/lib/api/groups";
import {
  describeTarget,
  draftFromGeneration,
  isStale,
  allAgentsText,
  appliesToAllAgents,
  moveRule,
  newRule,
  policyIssues,
  toDraftRequest,
  usePolicy,
  usePolicyGenerations,
  usePolicyWarnings,
  usePublish,
  usePutDraft,
  useRollback,
  type DraftState,
  type PolicyDocument,
  type PolicyScope,
  type RuleInput,
} from "@/lib/api/policy";
import type { Agent } from "@/lib/api/queries";
import { useEffectiveGovernance } from "@/lib/api/governance";
import { useSecrets } from "@/lib/api/secrets";

import { RuleEditorDialog, type Option, type PickerData } from "./rule-editor";
import { SimulatePanel } from "./simulate-panel";

function History({ scope, canEdit, onClose }: { scope: PolicyScope; canEdit: boolean; onClose: () => void }) {
  const gens = usePolicyGenerations(scope, true);
  const rollback = useRollback(scope);
  // Rollback replaces the draft: it carries the draft revision this list showed (409 otherwise).
  const draftRevision = gens.data?.items.find((g) => g.status === "draft")?.draft_revision ?? undefined;
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent title="Policy history" className="max-h-[90vh] overflow-y-auto">
        {gens.isPending ? <Loading /> : null}
        {gens.isError ? <ErrorState error={gens.error} /> : null}
        {rollback.isError ? <ErrorState error={rollback.error} /> : null}
        <ul className="space-y-2">
          {gens.data?.items.map((g) => (
            <li key={g.id} className="flex flex-wrap items-center justify-between gap-2 rounded-control border border-line p-2.5 text-sm text-ink">
              <span>
                #{g.number} · {g.status} · {g.rule_count} rules
                {g.published_at ? <span className="text-ink-2"> · {new Date(g.published_at).toLocaleString()}</span> : null}
                {g.published_by ? <span className="text-ink-2"> · {g.published_by.display_name}</span> : null}
              </span>
              {canEdit && g.status !== "draft" ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={rollback.isPending || draftRevision === undefined}
                  aria-label={`Roll back to generation ${g.number}`}
                  onClick={() =>
                    draftRevision !== undefined &&
                    rollback.mutate({ generation: g.number, expectRevision: draftRevision }, { onSuccess: onClose })
                  }
                >
                  Roll back
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      </DialogContent>
    </Dialog>
  );
}

export interface PolicyViewProps {
  title: string;
  scope: PolicyScope;
  canEdit: boolean;
  /** Workspace policy only: agents for the identity picker and simulate panel. */
  agents?: Agent[];
  users?: Option[];
  /** The signed-in user: the simulate panel's default speaker (RFE-005). */
  currentUserId?: string;
  /** Org id used to list org-scope secrets in the target picker. */
  orgId: string;
}

export function PolicyView({ title, scope, canEdit, agents = [], users = [], currentUserId, orgId }: PolicyViewProps) {
  const policy = usePolicy(scope);
  const warnings = usePolicyWarnings(scope);
  const wsSecrets = useSecrets(scope.kind === "workspace" ? { kind: "workspace", id: scope.id } : { kind: "organization", id: orgId });
  const orgSecrets = useSecrets({ kind: "organization", id: orgId });
  const eff = useEffectiveGovernance(orgId);
  // (P4-GRP-1) The org's groups for the identity picker; unknown when the caller may not list them.
  const groupsQ = useGroups(orgId);
  const byoEnabled = eff.data?.byo?.posture.value === "enabled";
  const [confirmBroad, setConfirmBroad] = useState(false);
  const put = usePutDraft(scope);
  const publish = usePublish(scope);

  const [local, setLocal] = useState<DraftState | null>(null);
  const [editing, setEditing] = useState<{ index: number | null; rule: RuleInput } | null>(null);
  const [history, setHistory] = useState(false);
  const [stale, setStale] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const draftRev = policy.data?.draft.draft_revision ?? null;

  // Local edits shadow the server draft until saved, discarded or reloaded.
  const serverDraft = policy.data?.draft;
  const dirty = local !== null;
  const state = local ?? (serverDraft ? draftFromGeneration(serverDraft) : null);

  if (policy.isPending) return <Loading />;
  if (policy.isError) return <ErrorState error={policy.error} onRetry={() => policy.refetch()} />;
  if (!state) return <Loading />;

  const doc = policy.data;
  const edit = (next: DraftState) => {
    setLocal(next);
    setNotice(null);
  };
  const allSecrets = [...(wsSecrets.data?.items ?? []), ...(scope.kind === "workspace" ? (orgSecrets.data?.items ?? []) : [])]
    .filter((s) => s.status === "active")
    .filter((s, i, a) => a.findIndex((x) => x.id === s.id) === i);
  const picker: PickerData = {
    agents: agents.map((a) => ({ id: a.id, label: `${a.name} (@${a.handle})` })),
    users,
    groups: groupsQ.data?.items.map((g) => ({ id: g.id, label: `${g.name} (${g.member_count})` })),
    secrets: allSecrets.map((s) => ({ id: s.id, label: `${s.name} (${s.scope})` })),
  };
  const secretName = (id: string) => allSecrets.find((s) => s.id === id)?.name ?? id;
  const issues = policyIssues(put.error ?? publish.error);
  const err = put.error ?? publish.error;
  const busy = put.isPending || publish.isPending;

  function reload() {
    setStale(false);
    put.reset();
    publish.reset();
    setLocal(null);
    void policy.refetch();
    void warnings.refetch();
  }

  async function save(thenPublish: boolean) {
    if (!state || draftRev === null) return;
    setNotice(null);
    try {
      const draft = await put.mutateAsync(toDraftRequest(state, draftRev, scope.kind));
      if (thenPublish) {
        await publish.mutateAsync(draft.draft_revision ?? draftRev);
        setNotice("Changes applied.");
      } else setNotice("Draft saved.");
      setLocal(null);
      void warnings.refetch();
    } catch (e) {
      if (isStale(e)) setStale(true);
    }
  }

  async function discard() {
    if (!doc || draftRev === null) return;
    try {
      await put.mutateAsync(toDraftRequest(draftFromGeneration(doc.published), draftRev, scope.kind));
      setLocal(null);
      setNotice("Draft discarded.");
    } catch (e) {
      if (isStale(e)) setStale(true);
    }
  }

  const rules = state.rules;
  const workspace = scope.kind === "workspace";
  const broad = rules.filter(appliesToAllAgents);

  return (
    <div className="max-w-page space-y-8">
      <PageHeader
        title={title}
        description="Rules that decide which requests agents may make, and which wait for approval."
        className="mb-6"
        actions={
          <Button variant="outline" size="sm" onClick={() => setHistory(true)}>
            History
          </Button>
        }
      />
      {!canEdit ? <Notice role="status">You can view this policy but not change it.</Notice> : null}

      {stale ? (
        <Notice tone="pending" role="alert">
          <p>The policy was changed by someone else. Reload to see the latest version (your unsaved edits will be lost).</p>
          <Button size="sm" className="mt-2" onClick={reload}>
            Reload policy
          </Button>
        </Notice>
      ) : err && !isStale(err) ? (
        <Notice tone="danger" role="alert">
          <p>{errorMessage(err)}</p>
          {issues.general.length ? (
            <ul className="list-disc pl-5">
              {issues.general.map((m, i) => (
                <li key={i}>{m}</li>
              ))}
            </ul>
          ) : null}
        </Notice>
      ) : null}
      {notice ? (
        <Notice tone="accent" role="status">
          {notice}
        </Notice>
      ) : null}

      <Section
        id="policy-rules"
        title="Rules"
        actions={
          canEdit ? (
            <Button size="sm" onClick={() => setEditing({ index: null, rule: newRule() })}>
              Add rule
            </Button>
          ) : null
        }
      >
        {rules.length === 0 ? (
          <EmptyState
            title="No rules yet"
            when="Add one when a request keeps being blocked, or when you want a say before something risky. Until then requests follow the default rule."
            ask={canEdit ? undefined : "Ask a workspace owner to add one."}
          >
            A rule says what agents may do on the internet: allow it, block it or ask you first.
          </EmptyState>
        ) : (
          <ol className="space-y-3">
            {rules.map((r, i) => (
              <li key={r.logical_id ?? `new-${i}`} className="rounded-card border border-line bg-surface p-4 text-sm text-ink shadow-card">
                <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
                  <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                    <span className="font-medium">{r.name}</span>{" "}
                    <Badge tone={r.action === "allow" ? "accent" : "danger"}>{r.action}</Badge>
                    {r.require_approval ? <Badge tone="pending">requires approval</Badge> : null}
                    {r.rate_limit ? (
                      <span className="text-ink-2">
                        {r.rate_limit.count}/{r.rate_limit.per}
                      </span>
                    ) : null}
                    {r.enabled === false ? <span className="text-muted">(disabled)</span> : null}
                  </div>
                  {canEdit ? (
                    <div className="flex flex-wrap gap-1">
                      <Button size="sm" variant="ghost" aria-label={`Move ${r.name} up`} disabled={i === 0} onClick={() => edit({ ...state, rules: moveRule(rules, i, i - 1) })}>
                        Up
                      </Button>
                      <Button size="sm" variant="ghost" aria-label={`Move ${r.name} down`} disabled={i === rules.length - 1} onClick={() => edit({ ...state, rules: moveRule(rules, i, i + 1) })}>
                        Down
                      </Button>
                      <Button size="sm" variant="outline" aria-label={`Edit ${r.name}`} onClick={() => setEditing({ index: i, rule: r })}>
                        Edit
                      </Button>
                      <Button size="sm" variant="danger" aria-label={`Delete ${r.name}`} onClick={() => edit({ ...state, rules: rules.filter((_, j) => j !== i) })}>
                        Delete
                      </Button>
                    </div>
                  ) : null}
                </div>
                {r.description ? <p className="mt-1 text-ink-2">{r.description}</p> : null}
                {appliesToAllAgents(r) ? (
                  <p data-testid="all-agents-note" className="mt-1 text-xs font-medium text-pending-ink">
                    ⚠ {allAgentsText(byoEnabled)}
                  </p>
                ) : null}
                <p className="mt-1 text-xs text-muted">
                  {(r.targets ?? []).length
                    ? (r.targets ?? []).map((t) => (t.type === "secret" ? `secret ${secretName(t.id)}` : describeTarget(t))).join("; ")
                    : "all targets"}
                </p>
                {issues.rules[i] ? (
                  <ul role="alert" className="mt-1 list-disc pl-5 text-xs text-danger-ink">
                    {issues.rules[i].map((m, k) => (
                      <li key={k}>{m}</li>
                    ))}
                  </ul>
                ) : null}
              </li>
            ))}
          </ol>
        )}
      </Section>

      <section aria-label="Defaults" className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
        <h2 className="font-display text-base font-semibold text-ink">Default rule</h2>
        <label className="block max-w-md text-sm">
          <span className="mb-1.5 block font-medium text-ink">Default rule</span>
          <select
            aria-label="Default rule"
            aria-describedby="default-rule-llm-note"
            className={inputClass}
            disabled={!canEdit}
            value={state.default_action}
            onChange={(e) => edit({ ...state, default_action: e.target.value as DraftState["default_action"] })}
          >
            <option value="none">No default (defer)</option>
            <option value="allow">Allow</option>
            <option value="block">Block</option>
          </select>
          {/* RFE-023 (D-66): the Default Rule skips governed LLM hosts; 07 §Why the Default Rule skips LLM hosts. */}
          <span
            id="default-rule-llm-note"
            data-testid="default-rule-llm-note"
            className="mt-1.5 block text-xs text-muted"
            title="Agents call their model with an Aurora-injected key on every turn, so a default of Block would stop every agent. LLM traffic is controlled by Governance instead. An explicit block rule that names an LLM host still applies."
          >
            Does not affect LLM endpoints — see{" "}
            <Link href={`/o/${orgId}/governance`} className="font-medium text-link underline">
              Governance
            </Link>
            . An explicit block rule that names an LLM host still applies.
          </span>
        </label>
        {workspace ? (
          <>
            <label className="block max-w-md text-sm">
              <span className="mb-1.5 block font-medium text-ink">Uncredentialed requests</span>
              <select
                aria-label="Uncredentialed default"
                className={inputClass}
                disabled={!canEdit}
                value={state.uncredentialed_default}
                onChange={(e) => edit({ ...state, uncredentialed_default: e.target.value as "none" | "block" })}
              >
                <option value="none">Follow the organization</option>
                <option value="block">Block unless allow-listed</option>
              </select>
              <span className="text-xs text-muted">A workspace can only tighten the organization policy.</span>
            </label>
            <label className="block max-w-md text-sm">
              <span className="mb-1.5 block font-medium text-ink">Browser tunnels (open lane)</span>
              <select
                aria-label="Open lane posture"
                className={inputClass}
                disabled={!canEdit}
                value={state.open_lane_posture}
                onChange={(e) => edit({ ...state, open_lane_posture: e.target.value as DraftState["open_lane_posture"] })}
              >
                <option value="enabled">Enabled</option>
                <option value="allowlist_only">Allow-list only</option>
                <option value="disabled">Disabled</option>
              </select>
              <span className="text-xs text-muted">
                Tunnels cannot be inspected or given credentials; allow-list only limits them to the allow-list. A workspace can only tighten the organization and instance setting.
              </span>
              {eff.data && eff.data.egress.open_lane.value !== "enabled" ? (
                <span className="block text-xs text-pending-ink" data-testid="open-lane-enforced">
                  Enforced by {eff.data.egress.open_lane.source === "instance" ? "instance" : "organization"}:{" "}
                  {eff.data.egress.open_lane.value}
                </span>
              ) : null}
            </label>
            <div className="space-y-2">
              <h3 className="text-sm font-medium text-ink">Allow-list</h3>
              <ul className="space-y-2">
                {state.uncredentialed_allowlist.map((e, i) => (
                  <li key={i} className="flex flex-wrap gap-2">
                    <input aria-label="Allow-list host" className={inputClass} disabled={!canEdit} placeholder="host" value={e.host} onChange={(ev) => edit({ ...state, uncredentialed_allowlist: state.uncredentialed_allowlist.map((x, j) => (j === i ? { ...x, host: ev.target.value } : x)) })} />
                    <input aria-label="Allow-list path" className={inputClass} disabled={!canEdit} placeholder="path (optional)" value={e.path ?? ""} onChange={(ev) => edit({ ...state, uncredentialed_allowlist: state.uncredentialed_allowlist.map((x, j) => (j === i ? { ...x, path: ev.target.value } : x)) })} />
                    {canEdit ? (
                      <Button type="button" variant="ghost" size="sm" onClick={() => edit({ ...state, uncredentialed_allowlist: state.uncredentialed_allowlist.filter((_, j) => j !== i) })}>
                        Remove
                      </Button>
                    ) : null}
                  </li>
                ))}
              </ul>
              {canEdit ? (
                <Button type="button" variant="outline" size="sm" onClick={() => edit({ ...state, uncredentialed_allowlist: [...state.uncredentialed_allowlist, { host: "", path: null }] })}>
                  Add allow-list entry
                </Button>
              ) : null}
            </div>
          </>
        ) : null}
      </section>

      {confirmBroad ? (
        <Notice tone="pending" role="alertdialog" aria-label="Confirm broad rules" className="space-y-2">
          <p className="font-medium">{allAgentsText(byoEnabled)}:</p>
          <ul className="list-disc pl-5">
            {broad.map((r, i) => (
              <li key={r.logical_id ?? `b-${i}`}>{r.name}</li>
            ))}
          </ul>
          <p>These rules select credentials for every agent of this scope. Apply anyway?</p>
          <div className="flex gap-2">
            <Button
              disabled={busy}
              onClick={() => {
                setConfirmBroad(false);
                void save(true);
              }}
            >
              Apply anyway
            </Button>
            <Button variant="outline" onClick={() => setConfirmBroad(false)}>
              Cancel
            </Button>
          </div>
        </Notice>
      ) : null}

      {canEdit ? (
        <div className="flex flex-wrap gap-2">
          <Button
            disabled={busy || (!dirty && !serverDraftDiffers(doc))}
            onClick={() => (broad.length > 0 ? setConfirmBroad(true) : void save(true))}
          >
            Apply changes
          </Button>
          <Button variant="outline" disabled={busy || !dirty} onClick={() => void save(false)}>
            Save draft
          </Button>
          <Button variant="ghost" disabled={busy} onClick={() => void discard()}>
            Discard draft
          </Button>
        </div>
      ) : null}

      <Section id="policy-warnings" title="Warnings">
        {warnings.isError ? <ErrorState error={warnings.error} /> : null}
        {warnings.data && warnings.data.items.length === 0 ? <p className="text-sm text-ink-2">No warnings.</p> : null}
        <ul className="space-y-2 text-sm">
          {warnings.data?.items.map((w, i) => (
            <li key={`${w.rule_logical_id}-${i}`}>
              <Notice tone="pending">
                <code className="font-mono">{w.code}</code> — {w.message}
              </Notice>
            </li>
          ))}
        </ul>
      </Section>

      {workspace ? <SimulatePanel wsId={scope.id} agents={agents} users={users} currentUserId={currentUserId} /> : null}

      {editing ? (
        <RuleEditorDialog
          key={editing.rule.logical_id ?? "new"}
          open
          initial={editing.rule}
          data={picker}
          onClose={() => setEditing(null)}
          onSave={(r) => {
            const idx = editing.index;
            edit({ ...state, rules: idx === null ? [...rules, r] : rules.map((x, j) => (j === idx ? r : x)) });
            setEditing(null);
          }}
        />
      ) : null}
      {history ? <History scope={scope} canEdit={canEdit} onClose={() => setHistory(false)} /> : null}
    </div>
  );
}

/** True when the saved draft already differs from the published generation. */
function serverDraftDiffers(doc: PolicyDocument): boolean {
  const strip = (g: PolicyDocument["draft"]) =>
    JSON.stringify(draftFromGeneration(g), (k, v) => (k === "logical_id" ? undefined : v));
  return strip(doc.draft) !== strip(doc.published);
}
