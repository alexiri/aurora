"use client";

import Link from "next/link";
import { useState, type ReactNode } from "react";

import { ByoPanel } from "@/components/governance/byo-panel";
import { Errors } from "@/components/governance/controls";
import { ModelsTab } from "@/components/governance/models-tab";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { ScopeLadder, type LadderAbove } from "@/components/ui/scope-ladder";
import { errorMessage } from "@/lib/api/errors";
import {
  formFromDocument,
  formFromEffective,
  formToDocument,
  governanceIssues,
  isStale,
  maxAgeDaysProblem,
  useEffectiveGovernance,
  useGovernanceDoc,
  useInstanceGovernance,
  usePutGovernance,
  type EffectiveGovernance,
  type GovForm,
  type GovIssues,
} from "@/lib/api/governance";
import { useWorkspaces, type Org } from "@/lib/api/queries";
import { orgRoutes, wsRoutes } from "@/lib/routes";
import { whoCanFromContacts } from "@/lib/who-can";

/** What the form hands its body: the editable form, a patch function, the server's field errors and whether to lock it. */
interface FormCtx {
  form: GovForm;
  patch: (p: Partial<GovForm>) => void;
  issues: GovIssues;
  readOnly: boolean;
  eff: EffectiveGovernance;
}

/**
 * The organization's governance document behind one form with one Save and Discard changes, for the page or tab that
 * hosts a part of it (AI models: the model rules; Agents › BYO: the BYO and token keys). Members read the effective
 * values and get no buttons. The part that is not shown goes back unchanged (the document is saved whole).
 */
function GovernanceForm({
  org,
  canEdit,
  saved: savedText,
  children,
}: {
  org: Org;
  canEdit: boolean;
  /** Said after a save. */
  saved: string;
  children: (ctx: FormCtx) => ReactNode;
}) {
  const [saved, setSaved] = useState(false);
  const eff = useEffectiveGovernance(org.id);
  const doc = useGovernanceDoc(org.id, canEdit);
  const instance = useInstanceGovernance(canEdit);
  if (eff.isPending || (canEdit && (doc.isPending || instance.isPending))) return <Loading />;
  if (eff.isError) return <ErrorState error={eff.error} onRetry={() => eff.refetch()} />;
  if (canEdit && doc.isError) return <ErrorState error={doc.error} onRetry={() => doc.refetch()} />;
  return (
    <FormBody
      key={canEdit ? doc.data?.revision : "ro"}
      org={org}
      eff={eff.data!}
      revision={doc.data?.revision ?? 0}
      initial={canEdit ? formFromDocument(doc.data?.document, eff.data!, instance.data) : formFromEffective(eff.data!)}
      canEdit={canEdit}
      saved={saved}
      setSaved={setSaved}
      savedText={savedText}
      onReload={() => void Promise.all([doc.refetch(), eff.refetch()])}
    >
      {children}
    </FormBody>
  );
}

function FormBody({
  org,
  eff,
  revision,
  initial,
  canEdit,
  saved,
  setSaved,
  savedText,
  onReload,
  children,
}: {
  org: Org;
  eff: EffectiveGovernance;
  revision: number;
  initial: GovForm;
  canEdit: boolean;
  saved: boolean;
  setSaved: (v: boolean) => void;
  savedText: string;
  onReload: () => void;
  children: (ctx: FormCtx) => ReactNode;
}) {
  const [form, setForm] = useState<GovForm>(initial);
  // The stored form: the page is keyed on the document's revision, so a save starts a fresh form from what is now stored.
  const [baseline] = useState(JSON.stringify(initial));
  const put = usePutGovernance(org.id);
  const issues = governanceIssues(put.error);
  const dirty = JSON.stringify(form) !== baseline;
  const patch = (p: Partial<GovForm>) => {
    setSaved(false);
    setForm((f) => ({ ...f, ...p }));
  };
  // The save re-keys this form (a new revision), so "Saved" lives in the parent and is told by the promise, not a callback.
  const save = () => {
    put.mutateAsync({ document: formToDocument(form), revision }).then(
      () => setSaved(true),
      () => undefined,
    );
  };
  return (
    <div className="space-y-4">
      {children({ form, patch, issues, readOnly: !canEdit, eff })}
      {canEdit ? (
        <div className="space-y-3 border-t border-line pt-4">
          {isStale(put.error) ? (
            <Notice tone="pending" role="alert">
              These rules were changed by someone else. Reload to see the latest version.{" "}
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
          {saved ? <Notice tone="accent" role="status">{savedText}</Notice> : null}
          <FormActions>
            <Button
              size="lg"
              onClick={save}
              disabled={put.isPending || !dirty || !!maxAgeDaysProblem(form.byo.maxAgeDays, form.byo.instanceMaxAgeDays)}
            >
              {put.isPending ? "Saving…" : "Save"}
            </Button>
            <Button
              size="lg"
              variant="outline"
              disabled={put.isPending || !dirty}
              onClick={() => {
                put.reset();
                setSaved(false);
                setForm(JSON.parse(baseline) as GovForm);
              }}
            >
              Discard changes
            </Button>
          </FormActions>
        </div>
      ) : null}
    </div>
  );
}

const POSTURE_WORDS: Record<string, string> = {
  open: "Agents may also use their own AI credentials",
  sanctioned_only: "Only keys an administrator approved",
};
const BYO_WORDS: Record<string, string> = {
  enabled: "Members of a workspace may run agents outside Aurora",
  owners_only: "Only workspace owners may run agents outside Aurora",
  disabled: "Nobody may run agents outside Aurora",
};
const SCOPE_WORDS: Record<string, string> = { organization: "keys for the whole organization", workspace: "workspace keys" };

function listWords(names: string[]): string {
  return names.length <= 2 ? names.join(" and ") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** (C-966, C-963) What Administration sets for AI models, one row each; nothing the organization chose itself. */
export function modelLimitsFromAbove(eff: EffectiveGovernance): LadderAbove[] {
  const who = whoCanFromContacts("instance_admin", undefined, "change_this");
  const rows: LadderAbove[] = [];
  if (eff.llm.posture.source === "instance") {
    rows.push({ label: "Which AI keys agents may use", value_display: POSTURE_WORDS[eff.llm.posture.value] ?? eff.llm.posture.value, source: "instance", who_can: who });
  }
  const set = eff.llm.endpoints.filter((e) => e.permitted.source === "instance");
  if (set.length > 0) {
    const yes = set.filter((e) => e.permitted.value).map((e) => e.display_name);
    const no = set.filter((e) => !e.permitted.value).map((e) => e.display_name);
    rows.push({
      label: "AI endpoints organizations may use",
      value_display: [yes.length > 0 ? `${listWords(yes)} allowed` : "", no.length > 0 ? `${listWords(no)} not allowed` : ""].filter(Boolean).join("; "),
      source: "instance",
      who_can: who,
    });
  }
  for (const e of eff.llm.endpoints) {
    if (e.key_scopes.source === "instance") {
      const scopes = e.key_scopes.value.map((s) => SCOPE_WORDS[s] ?? s);
      rows.push({ label: `Who may add ${e.display_name} keys`, value_display: scopes.length > 0 ? `Only ${scopes.join(" and ")}` : "Nobody", source: "instance", who_can: who });
    }
    if (e.allowed_models.source === "instance") {
      const models = e.allowed_models.value;
      rows.push({ label: `Models allowed on ${e.display_name}`, value_display: models.length > 0 ? models.join(", ") : "Any model", source: "instance", who_can: who });
    }
  }
  return rows;
}

/** (C-967, C-963) What Administration sets for agents outside Aurora. */
export function byoLimitsFromAbove(eff: EffectiveGovernance): LadderAbove[] {
  const who = whoCanFromContacts("instance_admin", undefined, "change_this");
  const rows: LadderAbove[] = [];
  if (eff.byo.posture.source === "instance") {
    rows.push({ label: "Who may run agents outside Aurora", value_display: BYO_WORDS[eff.byo.posture.value] ?? eff.byo.posture.value, source: "instance", who_can: who });
  }
  const cidrs = eff.byo.source_cidrs;
  if (cidrs.source === "instance" && (cidrs.value ?? []).length > 0) {
    rows.push({ label: "Where their tokens may be used from", value_display: (cidrs.value ?? []).join(", "), source: "instance", who_can: who });
  }
  if (eff.byo.require_context_credentials.source === "instance" && eff.byo.require_context_credentials.value) {
    rows.push({ label: "Context credentials", value_display: "Required: requests that carry an account's credential must come from a labelled run", source: "instance", who_can: who });
  }
  const age = eff.agent_tokens.max_age_days;
  if (age.source === "instance" && age.value != null) {
    rows.push({ label: "Longest a token lasts", value_display: `Up to ${age.value} days`, source: "instance", who_can: who });
  }
  return rows;
}

const REASONS: Record<string, string> = {
  mismatch: "belongs to an account these rules do not allow",
  unverifiable: "its origin could not be checked",
  not_permitted: "its endpoint or scope is no longer permitted",
  pending: "is waiting for a check",
};

/**
 * (C-966) "<n> AI keys no longer follow these rules", linking to the Accounts lists filtered to AI keys: the
 * organization's, and each workspace's that holds one. Nothing to say when all follow.
 */
export function KeysNotice({ eff, org }: { eff: EffectiveGovernance; org: Org }) {
  const workspaces = useWorkspaces(org.id);
  const bad = eff.non_compliant_secrets;
  if (bad.length === 0) {
    return (
      <Notice tone="accent" role="status">
        All AI keys follow these rules.
      </Notice>
    );
  }
  const slugOf = new Map((workspaces.data?.items ?? []).map((w) => [w.id, w]));
  const orgKeys = bad.filter((s) => !s.workspace_id);
  const byWorkspace = new Map<string, typeof bad>();
  for (const s of bad) if (s.workspace_id) byWorkspace.set(s.workspace_id, [...(byWorkspace.get(s.workspace_id) ?? []), s]);
  return (
    <Notice tone="pending" role="status" className="space-y-2" data-testid="keys-notice">
      <p className="font-medium text-ink">
        {bad.length} AI {bad.length === 1 ? "key no longer follows" : "keys no longer follow"} these rules
      </p>
      <p>Agents can&apos;t use {bad.length === 1 ? "it" : "them"} until {bad.length === 1 ? "it does" : "they do"}.</p>
      <ul className="space-y-1 text-sm">
        {orgKeys.length > 0 ? (
          <li>
            <Link className="font-medium text-link underline" href={orgRoutes(org.slug).accounts({ kind: "ai-key" })}>
              Show them in {org.name}&apos;s Accounts
            </Link>{" "}
            <span className="text-ink-2">({orgKeys.map((s) => `${s.name} ${REASONS[s.reason] ?? s.reason}`).join("; ")})</span>
          </li>
        ) : null}
        {[...byWorkspace.entries()].map(([id, keys]) => {
          const ws = slugOf.get(id);
          return (
            <li key={id}>
              {ws ? (
                <Link className="font-medium text-link underline" href={wsRoutes(org.slug, ws.slug).accounts({ kind: "ai-key" })}>
                  Show them in {ws.name}&apos;s Accounts
                </Link>
              ) : (
                <span className="font-medium text-ink">A workspace</span>
              )}{" "}
              <span className="text-ink-2">({keys.map((s) => `${s.name} ${REASONS[s.reason] ?? s.reason}`).join("; ")})</span>
            </li>
          );
        })}
      </ul>
    </Notice>
  );
}

/**
 * (P5-IA-3, C-966) The organization's AI models page: the notice for keys that no longer comply, Administration's
 * limits as ladder rows, then the organization's model rules. Members read it.
 */
export function OrgAiModels({ org }: { org: Org }) {
  const canEdit = org.my_role === "owner" || org.my_role === "admin";
  return (
    <GovernanceForm org={org} canEdit={canEdit} saved="Saved. Existing keys were re-checked against the new rules.">
      {({ form, patch, issues, readOnly, eff }) => (
        <div className="space-y-5">
          <KeysNotice eff={eff} org={org} />
          <ScopeLadder
            above={modelLimitsFromAbove(eff)}
            here={
              <div className="space-y-5">
                <h3 className="text-sm font-semibold text-ink">Rules for {org.name}</h3>
                <ModelsTab form={form} patch={patch} issues={issues} readOnly={readOnly} />
              </div>
            }
          />
        </div>
      )}
    </GovernanceForm>
  );
}

/** (P5-IA-3, C-967) Agents › BYO of an organization: Administration's limits above, the organization's own below. */
export function OrgByo({ org }: { org: Org }) {
  const canEdit = org.my_role === "owner" || org.my_role === "admin";
  return (
    <GovernanceForm org={org} canEdit={canEdit} saved="Saved.">
      {({ form, patch, issues, readOnly, eff }) => (
        <ScopeLadder
          above={byoLimitsFromAbove(eff)}
          here={<ByoPanel form={form} patch={patch} issues={issues} readOnly={readOnly} eff={eff} orgId={org.id} />}
        />
      )}
    </GovernanceForm>
  );
}
