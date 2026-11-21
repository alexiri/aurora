"use client";

import Link from "next/link";

import { formatWhen } from "@/lib/format-date";
import { useState, type ReactNode } from "react";

import { CommonChanges } from "@/components/internet/common-changes";
import { usePolicyEditor } from "@/components/internet/use-policy-editor";
import { RuleEditorDialog, type Option, type PickerData } from "@/components/policy/rule-editor";
import { SimulatePanel } from "@/components/policy/simulate-panel";
import { ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { SaveBar, useSaveBar } from "@/components/ui/save-bar";
import { Avatar } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { MainAction } from "@/components/ui/main-action";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ShieldIcon } from "@/components/ui/icons";
import { ScopeLadder, type LadderBelow } from "@/components/ui/scope-ladder";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { WhoCan } from "@/components/ui/who-can";
import { errorMessage } from "@/lib/api/errors";
import type { EffectiveGovernance } from "@/lib/api/governance";
import { useGroups } from "@/lib/api/groups";
import {
  allAgentsText,
  appliesToAllAgents,
  describeTarget,
  moveRule,
  newRule,
  useInternetAccess,
  usePolicyGenerations,
  useRollback,
  type DraftState,
  type PolicyScope,
  type PublishedInternetAccess,
} from "@/lib/api/policy";
import type { Agent } from "@/lib/api/queries";
import { useSecrets } from "@/lib/api/secrets";
import { useWhoCan } from "@/lib/api/who-can";
import { internetAccessOf, internetSummary } from "@/lib/summary/internet";
import { limitsFromAbove, ruleIsPending } from "@/lib/internet-access";
import { orgRoutes, wsRoutes } from "@/lib/routes";
import { whoCanFromContacts, type WhoCanValue } from "@/lib/who-can";
import { personName } from "@/lib/workspace-name";

/** The organization's own limits (the governance document): its controls and one apply for them (C-989). */
export interface GovernanceEditor {
  /** The controls: posture, browser connections, allow-list. */
  controls: ReactNode;
  dirty: boolean;
  /** Sends the document; `true` when it is applied (the editor shows its own error otherwise). */
  apply: () => Promise<boolean>;
  discard: () => void;
}

/** (FIX-164) The published rules as text, for a member: what each does and to which websites, no controls. */
function ReadOnlyRules({ rules }: { rules: PublishedInternetAccess["rules"] }) {
  if (rules.length === 0) {
    return (
      <p className="rounded-card border border-dashed border-line-strong bg-surface p-6 text-sm text-ink-2">
        No rules yet. Requests follow the limits and defaults below.
      </p>
    );
  }
  return (
    <ol className="space-y-3" aria-label="Your rules">
      {rules.map((r) => (
        <li key={r.logical_id} className="rounded-card border border-line bg-surface p-4 text-sm text-ink shadow-card">
          <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
            <Badge tone={r.action === "allow" && !r.require_approval ? "accent" : r.action === "allow" ? "neutral" : "danger"}>
              {r.action === "block" ? "Blocks" : r.require_approval ? "Asks" : "Allows"}
            </Badge>
            <span className="font-medium">{r.name}</span>
            {r.enabled === false ? <span className="text-muted">(turned off)</span> : null}
          </div>
          {r.description ? <p className="mt-1 text-ink-2">{r.description}</p> : null}
          <p className="mt-1 break-words font-mono text-xs text-muted">
            {r.targets.length ? r.targets.map((t) => (t.type === "secret" ? "a stored token" : describeTarget(t))).join("; ") : "all targets"}
          </p>
        </li>
      ))}
    </ol>
  );
}

/** (org-14) "History of internet rules": a page section, one row per published version, who changed what, Roll back. */
function HistorySection({ scope, description }: { scope: PolicyScope; description: string }) {
  const gens = usePolicyGenerations(scope, true);
  const rollback = useRollback(scope);
  // Rollback replaces the draft: it carries the draft revision this list showed (409 otherwise).
  const draftRevision = gens.data?.items.find((g) => g.status === "draft")?.draft_revision ?? undefined;
  const [target, setTarget] = useState<number | null>(null);
  const rows = gens.data?.items.filter((g) => g.status !== "draft") ?? [];
  return (
    <Section id="internet-history" title="History of internet rules" description={description}>
      {gens.isPending ? <Loading /> : null}
      {gens.isError ? <ErrorState error={gens.error} /> : null}
      {rollback.isError ? <ErrorState error={rollback.error} /> : null}
      {rows.length > 0 ? (
        <ul className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface shadow-card">
          {rows.map((g) => (
            <li key={g.id} className="flex flex-wrap items-center justify-between gap-3 p-4 text-sm text-ink">
              <span className="flex min-w-0 items-center gap-3">
                {g.actor || g.published_by ? (
                  <Avatar name={personName((g.actor ?? g.published_by)?.display_name)} tintKey={g.actor?.display_name ?? g.id} size={32} />
                ) : (
                  // No one to show (the seeded first version, or a person who has left): a neutral tile, never a "?" avatar.
                  <span aria-hidden="true" className="inline-flex size-8 shrink-0 items-center justify-center rounded-full bg-subtle text-ink-2">
                    <ShieldIcon width={16} height={16} />
                  </span>
                )}
                <span className="min-w-0">
                  {/* (org-14, C-1087) Who changed what, in a sentence from the generation's change list. */}
                  <span className="block break-words font-medium">{historySentence(g)}</span>
                  <span className="block text-xs text-ink-2">
                    {g.published_at ? formatWhen(g.published_at) : formatWhen(g.created_at)}
                    {g.status === "published" ? " · current" : ""}
                  </span>
                </span>
              </span>
              {g.status === "published" ? (
                <Badge tone="accent">Current</Badge>
              ) : (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={rollback.isPending || draftRevision === undefined}
                  aria-label={`Roll back to version ${g.number}`}
                  onClick={() => setTarget(g.number)}
                >
                  Roll back to this
                </Button>
              )}
            </li>
          ))}
        </ul>
      ) : null}
      <ConfirmDialog
        open={target !== null}
        onOpenChange={(o) => {
          if (!o) setTarget(null);
        }}
        title="Roll back to this version?"
        description="Rules added since stop applying now."
        confirmLabel="Roll back"
        busy={rollback.isPending}
        onConfirm={() => {
          if (target === null || draftRevision === undefined) return;
          rollback.mutate({ generation: target, expectRevision: draftRevision }, { onSuccess: () => setTarget(null) });
        }}
      />
    </Section>
  );
}

const CHANGE_VERB = { added: "added", removed: "removed", edited: "edited" } as const;

/**
 * (org-14, C-1087) Who changed what: "Alice added ‘Block tracker’", "Alice added ‘A’ and removed ‘B’", "Alice changed 3
 * rules". A version with no change list (the first, or one that changed only the defaults) reads "Alice published version 4".
 */
export function historySentence(g: {
  number: number;
  published_by?: { display_name: string } | null;
  actor?: { display_name: string } | null;
  changes?: { kind: "added" | "removed" | "edited"; rule_name: string }[];
  more?: number;
}): string {
  const changes = g.changes ?? [];
  const total = changes.length + (g.more ?? 0);
  // The seeded first version has no author and no change list: it is the starting rules, not "Someone".
  if (g.number === 1 && !g.actor && !g.published_by && total === 0) return "The starting rules, version 1";
  const author = g.actor ?? g.published_by;
  const who = author ? personName(author.display_name) : "Someone";
  if (total === 0) return `${who} published version ${g.number}`;
  if (total > 2) return `${who} changed ${total} rules`;
  const [a, b] = changes;
  const first = `${CHANGE_VERB[a!.kind]} ‘${a!.rule_name}’`;
  if (!b) return `${who} ${first}`;
  return b.kind === a!.kind ? `${who} ${first} and ‘${b.rule_name}’` : `${who} ${first} and ${CHANGE_VERB[b.kind]} ‘${b.rule_name}’`;
}

const LANE_OPTIONS: { value: DraftState["open_lane_posture"]; label: string }[] = [
  { value: "enabled", label: "Let the organization decide" },
  { value: "allowlist_only", label: "Only listed websites" },
  { value: "disabled", label: "Turn off" },
];

/** (C-926) The defaults a level keeps in its policy draft; the workspace has four, the organization one. */
function PolicyDefaults({ state, edit, workspace, orgSlug }: { state: DraftState; edit: (s: DraftState) => void; workspace: boolean; orgSlug: string }) {
  const [host, setHost] = useState("");
  const [path, setPath] = useState("");
  const list = state.uncredentialed_allowlist;
  return (
    <div className="space-y-4">
      <div className="max-w-md">
        {/* RFE-023 (D-66): the default rule skips governed LLM hosts; 07 §Why the Default Rule skips LLM hosts. */}
        <Field
          id="default-rule"
          label="Default rule"
          hint={
            <span
              data-testid="default-rule-llm-note"
              title="Agents call their model with an Aurora-injected key on every turn, so a default of Block would stop every agent. AI model traffic is controlled by the AI models rules instead. An explicit block rule that names an AI host still applies."
            >
              Does not affect AI endpoints. See{" "}
              <Link href={orgRoutes(orgSlug).aiModels()} className="font-medium text-link underline">
                AI models
              </Link>
              . An explicit block rule that names an AI host still applies.
            </span>
          }
        >
          <Select
            id="default-rule"
            value={state.default_action}
            onChange={(v) => edit({ ...state, default_action: v as DraftState["default_action"] })}
            options={[
              { value: "none", label: workspace ? "Let the organization decide" : "None: only rules decide" },
              { value: "allow", label: "Allow" },
              { value: "block", label: "Block" },
            ]}
          />
        </Field>
      </div>
      {workspace ? (
        <>
          <div className="max-w-md">
            <Field id="uncredentialed-default" label="Requests without an account" hint="Websites on the allow-list stay reachable. A workspace can only restrict further what the organization sets.">
              <Select id="uncredentialed-default" value={state.uncredentialed_default} onChange={(v) => edit({ ...state, uncredentialed_default: v as DraftState["uncredentialed_default"] })} options={[{ value: "none", label: "Let the organization decide" }, { value: "block", label: "Block" }]} />
            </Field>
          </div>
          <div className="max-w-md">
            <Field id="open-lane-posture" label="Browser connections (not inspected)" hint="Aurora can't see inside these. A workspace can only restrict further what the organization and an Aurora administrator set.">
              <Select id="open-lane-posture" value={state.open_lane_posture} onChange={(v) => edit({ ...state, open_lane_posture: v as DraftState["open_lane_posture"] })} options={LANE_OPTIONS.map((o) => ({ value: o.value, label: o.label }))} />
            </Field>
          </div>
          <section aria-label="Allow-list" className="space-y-2">
            <h3 className="text-sm font-medium text-ink">Allow-list</h3>
            {list.length === 0 ? <p className="text-sm text-ink-2">No websites listed.</p> : null}
            <ul className="space-y-1.5">
              {list.map((e, i) => (
                <li key={`${e.host}${e.path ?? ""}-${i}`} className="flex flex-wrap items-center gap-2 text-sm text-ink">
                  <span className="font-mono">
                    {e.host}
                    {e.path ?? ""}
                  </span>
                  <Button type="button" variant="ghost" size="sm" aria-label={`Remove ${e.host}`} onClick={() => edit({ ...state, uncredentialed_allowlist: list.filter((_, j) => j !== i) })}>
                    Remove
                  </Button>
                </li>
              ))}
            </ul>
            <form
              className="flex flex-wrap gap-2"
              onSubmit={(ev) => {
                ev.preventDefault();
                if (!host.trim()) return;
                edit({ ...state, uncredentialed_allowlist: [...list, { host: host.trim(), path: path.trim() || null }] });
                setHost("");
                setPath("");
              }}
            >
              <input aria-label="Website" className={`${inputClass} min-w-0 flex-1 basis-48`} placeholder="Website, e.g. docs.example.org" value={host} onChange={(e) => setHost(e.target.value)} />
              <input aria-label="Path (optional)" className={`${inputClass} min-w-0 flex-1 basis-48`} placeholder="Path (optional)" value={path} onChange={(e) => setPath(e.target.value)} />
              <Button type="submit" variant="outline" size="sm">
                Add website
              </Button>
            </form>
          </section>
        </>
      ) : null}
    </div>
  );
}

/** "Looser options are not available: Administration requires …" when the instance sets a limit this level cannot loosen. */
function minimumNote(eff: EffectiveGovernance): string | null {
  const parts: string[] = [];
  if (eff.egress.posture.source === "instance" && eff.egress.posture.value === "credentialed_only") parts.push("agents to reach only listed websites");
  if (eff.egress.open_lane.source === "instance" && eff.egress.open_lane.value !== "enabled") {
    parts.push(eff.egress.open_lane.value === "disabled" ? "browser connections to be turned off" : "browser connections only for listed websites");
  }
  return parts.length ? `Looser options are not available: Administration requires ${parts.join(" and ")}.` : null;
}

export interface InternetLevelProps {
  level: "workspace" | "org";
  scope: PolicyScope;
  orgId: string;
  orgSlug: string;
  /** The organization's name, for the page's sentence. */
  orgName?: string;
  wsSlug?: string;
  /** Owners and admins of the level; others see the limits only. */
  canEdit: boolean;
  eff: EffectiveGovernance;
  /** Who can change this level, for a viewer who cannot; and who changes the organization's limits. */
  whoCanHere: WhoCanValue;
  whoCanOrg: WhoCanValue;
  agents?: Agent[];
  users?: Option[];
  currentUserId?: string;
  governance?: GovernanceEditor;
  below?: LadderBelow;
  /** The "Tightened below" part of the organization: a heading and a link to show all. */
  belowNote?: ReactNode;
  /** The caller draws the page heading itself (the preset card goes under it, never above it); the editable view then skips it. */
  hideHeader?: boolean;
}

/**
 * (P5-IA-2, C-965) Internet access at a workspace or an organization: the summary, the line about apps, Common changes,
 * the rules people wrote, the limits (above, here, below) and Check what would happen, with **one** Apply bar (C-989).
 */
export function InternetLevel(p: InternetLevelProps) {
  const workspace = p.level === "workspace";
  const ed = usePolicyEditor(p.scope, p.canEdit);
  // A member of the workspace reads the published rules from the member route (FIX-164); owners read the full document.
  const memberRead = useInternetAccess(workspace ? p.scope.id : undefined, workspace && !p.canEdit);
  const addRule = useWhoCan(workspace ? { workspaceId: p.scope.id } : { orgId: p.orgId }, workspace ? "workspace_owner" : "org_admin", "add_rule");
  const groupsQ = useGroups(p.orgId);
  const wsSecrets = useSecrets(workspace ? { kind: "workspace", id: p.scope.id } : { kind: "organization", id: p.orgId }, p.canEdit);
  const orgSecrets = useSecrets({ kind: "organization", id: p.orgId }, p.canEdit && workspace);
  const [editing, setEditing] = useState<{ index: number | null; rule: ReturnType<typeof newRule> } | null>(null);
  const [simulating, setSimulating] = useState(false);
  const [confirmBroad, setConfirmBroad] = useState(false);
  const [partial, setPartial] = useState<string | null>(null);
  const bar = useSaveBar();
  const gov = p.governance;

  const who = { org: p.whoCanOrg, instance: whoCanFromContacts("instance_admin", undefined, "change_this") };
  const limits = limitsFromAbove(p.level, p.eff, who);
  const note = minimumNote(p.eff);

  const header = (description: ReactNode, actions?: ReactNode) => <PageHeader title="Internet access" description={description} className="mb-2" actions={actions} />;

  if (!p.canEdit) {
    // (FIX-164, D-365) A workspace's members read its published rules: the summary and the rules as text, no controls.
    const memberSummary = memberRead.data
      ? internetSummary(
          internetAccessOf(memberRead.data, p.eff.egress.posture.value === "credentialed_only", p.eff.egress.baseline_allowlist.length + p.eff.egress.org_allowlist.length),
          "Agents here",
        )
      : null;
    return (
      <div className="max-w-page space-y-6">
        {header(
          memberSummary ? (
            <>
              <span data-testid="internet-summary" className="block font-display text-xl text-ink">
                {memberSummary}
              </span>
              <span className="mt-2 block">
                What agents may do in apps is set for each agent, on its{" "}
                <Link href={wsRoutes(p.orgSlug, p.wsSlug ?? "").agents()} className="font-medium text-link underline">
                  Apps &amp; access tab
                </Link>
                .
              </span>
            </>
          ) : (
            "Which websites agents can reach, and what asks or blocks."
          ),
        )}
        <Notice role="status">
          <WhoCan value={{ ...p.whoCanHere, action: "change_this" }} lead className="text-inherit" />
        </Notice>
        {workspace ? (
          <Section id="policy-rules" title="Your rules">
            {memberRead.isPending ? <Loading /> : null}
            {memberRead.isError ? <ErrorState error={memberRead.error} onRetry={() => void memberRead.refetch()} /> : null}
            {memberRead.data ? <ReadOnlyRules rules={memberRead.data.rules} /> : null}
          </Section>
        ) : null}
        {/* (FIX-206, D-444) No "Limits": a viewer who can change nothing here is not shown what is set above this level. */}
      </div>
    );
  }

  if (ed.policy.isPending || (!ed.state && !ed.policy.isError)) return <Loading />;
  if (ed.policy.isError) return <ErrorState error={ed.policy.error} onRetry={() => ed.policy.refetch()} />;
  const state = ed.state as DraftState;
  const published = ed.published as DraftState;
  const doc = ed.policy.data!;

  const allSecrets = [...(wsSecrets.data?.items ?? []), ...(workspace ? (orgSecrets.data?.items ?? []) : [])]
    .filter((s) => s.status === "active")
    .filter((s, i, a) => a.findIndex((x) => x.id === s.id) === i);
  const picker: PickerData = {
    agents: (p.agents ?? []).map((a) => ({ id: a.id, label: `${a.name} (@${a.handle})` })),
    users: p.users ?? [],
    groups: groupsQ.data?.items.map((g) => ({ id: g.id, label: `${g.name} (${g.member_count})` })),
    secrets: allSecrets.map((s) => ({ id: s.id, label: `${s.name} (${s.scope})` })),
  };
  const secretName = (id: string) => allSecrets.find((s) => s.id === id)?.name ?? id;
  const rules = state.rules;
  const broad = rules.filter(appliesToAllAgents);
  const byoEnabled = p.eff.byo?.posture.value === "enabled";

  const summary = internetSummary(internetAccessOf(doc.published, p.eff.egress.posture.value === "credentialed_only", p.eff.egress.baseline_allowlist.length + p.eff.egress.org_allowlist.length),
    "Agents here",
  );
  const count = ed.pending + (gov?.dirty ? 1 : 0);

  async function apply() {
    setPartial(null);
    bar.clear();
    let limitsApplied = false;
    if (gov?.dirty) {
      if (!(await gov.apply())) return;
      limitsApplied = true;
    }
    if (ed.pending > 0) {
      if (!(await ed.apply())) {
        if (limitsApplied) setPartial("The limits were applied. The rules and defaults were not; they are still pending.");
        return;
      }
    }
    bar.flash();
  }

  async function discard() {
    setPartial(null);
    bar.clear();
    gov?.discard();
    if (ed.pending > 0) await ed.discard();
  }

  return (
    <div className="max-w-page space-y-8">
      {p.hideHeader ? null : header(
        workspace ? (
          <>
            <span data-testid="internet-summary" className="block font-display text-xl text-ink">
              {summary}
            </span>
            <span className="mt-2 block">
              What agents may do in apps is set for each agent, on its{" "}
              <Link href={wsRoutes(p.orgSlug, p.wsSlug ?? "").agents()} className="font-medium text-link underline">
                Apps &amp; access tab
              </Link>
              .
            </span>
          </>
        ) : (
          // (P5-REFINE-17, org-8) The organization's page is two groups: Your rules and Set above you.
          <span data-testid="internet-summary">What agents in {p.orgName ?? "the organization"} may reach.</span>
        ),
      )}

      {ed.stale ? (
        <Notice tone="pending" role="alert">
          <p>The rules were changed by someone else. Reload to see the latest version (your edits will be lost).</p>
          <Button size="sm" className="mt-2" onClick={ed.reload}>
            Reload
          </Button>
        </Notice>
      ) : ed.error ? (
        <Notice tone="danger" role="alert">
          <p>{errorMessage(ed.error)}</p>
          {ed.issues.general.length ? (
            <ul className="list-disc pl-5">
              {ed.issues.general.map((m, i) => (
                <li key={i}>{m}</li>
              ))}
            </ul>
          ) : null}
        </Notice>
      ) : null}

      <CommonChanges scopeWords={workspace ? "workspace" : "organization"} onAdd={(r) => ed.edit({ ...state, rules: [...rules, r] })} />

      <Section
        id="policy-rules"
        title="Your rules"
        actions={
          <MainAction canAct whoCan={addRule}>
            <Button size="sm" variant="outline" onClick={() => setEditing({ index: null, rule: newRule() })}>
              Add rule
            </Button>
          </MainAction>
        }
      >
        {rules.length === 0 ? (
          <p className="rounded-card border border-dashed border-line-strong bg-surface p-6 text-sm text-ink-2">
            No rules yet. Until you add one, requests follow the limits and defaults below.
          </p>
        ) : (
          <ol className="space-y-3">
            {rules.map((r, i) => (
              <li key={r.logical_id ?? `new-${i}`} className="rounded-card border border-line bg-surface p-4 text-sm text-ink shadow-card">
                <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
                  <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                    <Badge tone={r.action === "allow" && !r.require_approval ? "accent" : r.action === "allow" ? "neutral" : "danger"}>
                      {r.action === "block" ? "Blocks" : r.require_approval ? "Asks" : "Allows"}
                    </Badge>
                    <span className="font-medium">{r.name}</span>
                    {r.rate_limit ? <span className="text-ink-2">{r.rate_limit.count}/{r.rate_limit.per}</span> : null}
                    {r.enabled === false ? <span className="text-muted">(turned off)</span> : null}
                    {ruleIsPending(r, published) ? <Badge>Not applied yet</Badge> : null}
                  </div>
                  <div className="flex flex-wrap gap-1">
                    <Button size="sm" variant="ghost" aria-label={`Move ${r.name} up`} disabled={i === 0} onClick={() => ed.edit({ ...state, rules: moveRule(rules, i, i - 1) })}>
                      Up
                    </Button>
                    <Button size="sm" variant="ghost" aria-label={`Move ${r.name} down`} disabled={i === rules.length - 1} onClick={() => ed.edit({ ...state, rules: moveRule(rules, i, i + 1) })}>
                      Down
                    </Button>
                    <Button size="sm" variant="outline" aria-label={`Edit ${r.name}`} onClick={() => setEditing({ index: i, rule: r })}>
                      Edit
                    </Button>
                    <Button size="sm" variant="danger" aria-label={`Delete ${r.name}`} onClick={() => ed.edit({ ...state, rules: rules.filter((_, j) => j !== i) })}>
                      Delete
                    </Button>
                  </div>
                </div>
                {r.description ? <p className="mt-1 text-ink-2">{r.description}</p> : null}
                {appliesToAllAgents(r) ? (
                  <p data-testid="all-agents-note" className="mt-1 text-xs font-medium text-pending-ink">
                    ⚠ {allAgentsText(byoEnabled)}
                  </p>
                ) : null}
                <p className="mt-1 font-mono text-xs text-muted">
                  {(r.targets ?? []).length ? (r.targets ?? []).map((t) => (t.type === "secret" ? `secret ${secretName(t.id)}` : describeTarget(t))).join("; ") : "all targets"}
                </p>
                {ed.issues.rules[i] ? (
                  <ul role="alert" className="mt-1 list-disc pl-5 text-xs text-danger-ink">
                    {ed.issues.rules[i].map((m, k) => (
                      <li key={k}>{m}</li>
                    ))}
                  </ul>
                ) : null}
              </li>
            ))}
          </ol>
        )}
      </Section>

      {ed.warnings.data && ed.warnings.data.items.length > 0 ? (
        <Section id="policy-warnings" title="Warnings">
          <ul className="space-y-2 text-sm">
            {ed.warnings.data.items.map((w, i) => (
              <li key={`${w.rule_logical_id}-${i}`}>
                <Notice tone="pending">{w.message}</Notice>
              </li>
            ))}
          </ul>
        </Section>
      ) : null}

      {workspace ? (
        <Section id="internet-limits" title="Limits and defaults">
          <ScopeLadder
            above={limits}
            minimumNote={note}
            collapsed
            here={
              <div className="space-y-5">
                {gov?.controls}
                <PolicyDefaults state={state} edit={ed.edit} workspace orgSlug={p.orgSlug} />
              </div>
            }
            below={p.below}
          />
          {p.belowNote}
        </Section>
      ) : (
        <>
          {/* (org-8) Set above you: one line each, with who set it. The allow-list shows once. */}
          <Section id="internet-above" title="Set above you">
            {limits.length === 0 ? (
              <p className="text-sm text-ink-2">Nothing above {p.orgName ?? "the organization"} limits what agents may reach.</p>
            ) : (
              <ul aria-label="Set above you" className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface text-sm shadow-card">
                {limits.map((l) => (
                  <li key={l.label} className="px-4 py-3">
                    <p className="font-medium text-ink">{l.label}</p>
                    <p className="text-ink-2">{l.value_display}</p>
                    <p className="text-xs text-muted">Set by {l.source === "instance" ? "the instance admin" : "your organization's admins"}</p>
                  </li>
                ))}
              </ul>
            )}
            {note ? <p className="text-sm text-ink-2">{note}</p> : null}
          </Section>
          <details className="rounded-card border border-line bg-surface shadow-card" data-testid="internet-more">
            <summary className="cursor-pointer px-4 py-3 font-display text-base font-semibold text-ink">More settings</summary>
            <div className="space-y-5 border-t border-line p-4">
              {gov?.controls}
              <PolicyDefaults state={state} edit={ed.edit} workspace={false} orgSlug={p.orgSlug} />
              {p.below ? <ScopeLadder above={[]} below={p.below} /> : null}
              {p.belowNote}
            </div>
          </details>
        </>
      )}

      {/* (P5-POLISH-7, F1) The simulator is a button that opens it in a dialog, not a form at the bottom of the page. */}
      {workspace ? (
        <Altitude id="policy.try" level="setup">
          <div className="flex flex-wrap items-center gap-3">
            <Button variant="outline" onClick={() => setSimulating(true)}>
              Try a request
            </Button>
            <span className="text-sm text-ink-2">Check what would happen: see which rule decides a request.</span>
          </div>
        </Altitude>
      ) : null}
      {workspace && simulating ? (
        <Dialog open onOpenChange={(o) => (o ? null : setSimulating(false))}>
          <DialogContent size="lg" title="Try a request">
            <SimulatePanel wsId={p.scope.id} agents={p.agents ?? []} users={p.users ?? []} currentUserId={p.currentUserId} onDone={() => setSimulating(false)} />
          </DialogContent>
        </Dialog>
      ) : null}

      {confirmBroad ? (
        <Notice tone="pending" role="alertdialog" aria-label="Confirm broad rules" className="space-y-2">
          <p className="font-medium">{allAgentsText(byoEnabled)}:</p>
          <ul className="list-disc pl-5">
            {broad.map((r, i) => (
              <li key={r.logical_id ?? `b-${i}`}>{r.name}</li>
            ))}
          </ul>
          <p>These rules select accounts for every agent here. Apply anyway?</p>
          <div className="flex gap-2">
            <Button
              disabled={ed.busy}
              onClick={() => {
                setConfirmBroad(false);
                void apply();
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

      <SaveBar
        dirty={count > 0}
        saving={ed.busy}
        saved={bar.saved}
        savedMessage="Saved. Changes apply to new requests."
        unsavedLabel={count === 1 ? "1 unsaved change" : `${count} unsaved changes`}
        onSave={() => (broad.length > 0 && ed.pending > 0 ? setConfirmBroad(true) : void apply())}
        onDiscard={() => void discard()}
      >
        {partial}
      </SaveBar>

      {editing ? (
        <RuleEditorDialog
          key={editing.rule.logical_id ?? "new"}
          open
          initial={editing.rule}
          data={picker}
          onClose={() => setEditing(null)}
          onSave={(r) => {
            const idx = editing.index;
            ed.edit({ ...state, rules: idx === null ? [...rules, r] : rules.map((x, j) => (j === idx ? r : x)) });
            setEditing(null);
          }}
        />
      ) : null}
      {p.canEdit ? (
        <HistorySection
          scope={p.scope}
          description={workspace ? "Each change to what agents here may reach." : `Each change to what agents in ${p.orgName ?? "the organization"} may reach.`}
        />
      ) : null}
    </div>
  );
}
