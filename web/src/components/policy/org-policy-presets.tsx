"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { ErrorNotice } from "@/components/ui/notice";
import {
  formFromDocument,
  formToDocument,
  useEffectiveGovernance,
  useGovernanceDoc,
  useInstanceGovernance,
  usePutGovernance,
} from "@/lib/api/governance";
import { draftFromGeneration, usePolicy, usePublish, usePutDraft, toDraftRequest, type PolicyScope } from "@/lib/api/policy";
import type { Org } from "@/lib/api/queries";
import { pendingChanges } from "@/lib/internet-access";
import {
  applyOrgPreset,
  detectOrgPreset,
  ORG_ASK_RULE_NAME,
  orgFloors,
  orgPresetForbidden,
  PRESET_IDS,
  PRESETS,
  withAskRule,
  type OrgPart,
  type PresetId,
} from "@/lib/policy-presets";

/**
 * (P5-REFINE-18 #2, D-410, D-427) "How much may agents do here?" on an organization's AI models and Internet access pages. The
 * same three cards as Administration's, setting the organization's own postures. **Balanced** on Internet access also adds an
 * organization rule that asks before write actions (the existing Ask rule shape, "Ask before changing data"), published at
 * once when the policy has nothing else waiting. A card looser than what the instance admin set is disabled with the reason.
 */
export function OrgPolicyPresets({ org, part }: { org: Org; part: OrgPart }) {
  const eff = useEffectiveGovernance(org.id);
  const doc = useGovernanceDoc(org.id, true);
  const instance = useInstanceGovernance(true);
  const put = usePutGovernance(org.id);
  const scope: PolicyScope = { kind: "organization", id: org.id };
  const policy = usePolicy(scope, part === "internet");
  const putDraft = usePutDraft(scope);
  const publish = usePublish(scope);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  if (eff.isPending || doc.isPending || instance.isPending || (part === "internet" && policy.isPending)) return <Loading />;
  if (eff.isError) return <ErrorState error={eff.error} onRetry={() => eff.refetch()} />;
  if (doc.isError) return <ErrorState error={doc.error} onRetry={() => doc.refetch()} />;
  if (part === "internet" && policy.isError) return <ErrorState error={policy.error} onRetry={() => policy.refetch()} />;
  const form = formFromDocument(doc.data?.document, eff.data, instance.data);
  const floors = orgFloors(eff.data, form);
  const published = policy.data ? draftFromGeneration(policy.data.published) : null;
  const draft = policy.data ? draftFromGeneration(policy.data.draft) : null;
  const askOn = (published?.rules ?? []).some((r) => r.name === ORG_ASK_RULE_NAME);
  const current = detectOrgPreset(form, floors, part, askOn);
  const base: PresetId = current ?? PRESET_IDS.find((id) => orgPresetForbidden(id, floors, part, form) === null) ?? "open";

  async function choose(id: PresetId) {
    setError(null);
    setBusy(true);
    try {
      await put.mutateAsync({ document: formToDocument(applyOrgPreset(id, form, part)), revision: doc.data?.revision ?? 0 });
      if (part === "internet" && draft && published) {
        const wantAsk = id === "balanced";
        if (wantAsk !== (draft.rules.some((r) => r.name === ORG_ASK_RULE_NAME))) {
          // Nothing else is waiting in the draft: the rule is applied at once. Otherwise it joins the draft for the page\x27s bar.
          const nothingWaiting = pendingChanges(published, draft) === 0;
          const rev = policy.data?.draft.draft_revision ?? 0;
          const saved = await putDraft.mutateAsync(toDraftRequest({ ...draft, rules: withAskRule(draft.rules, wantAsk) }, rev, "organization"));
          if (nothingWaiting) await publish.mutateAsync(saved.draft_revision ?? rev);
        }
      }
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section aria-labelledby={`org-presets-${part}-h`} className="space-y-3 rounded-card border border-line bg-surface p-4" data-testid="policy-presets">
      <div className="flex flex-wrap items-center gap-2">
        <h2 id={`org-presets-${part}-h`} className="font-display text-base font-semibold text-ink">
          How much may agents do here?
        </h2>
        <Badge tone={current === null ? "neutral" : "accent"} data-testid="preset-state">
          {current === null ? `Custom · based on ${PRESETS[base].label}` : PRESETS[current].label}
        </Badge>
      </div>
      <ChoiceCards<PresetId | "">
        legend="How much agents may do"
        legendHidden
        value={current ?? ""}
        disabled={busy || put.isPending}
        onChange={(v) => v && void choose(v)}
        options={PRESET_IDS.map((id) => {
          const why = orgPresetForbidden(id, floors, part, form);
          return {
            value: id,
            label: PRESETS[id].label,
            description: orgSentence(id, part, why),
            disabled: why !== null,
          };
        })}
      />
      {error ? <ErrorNotice error={error} /> : null}
    </section>
  );
}

/** The card\x27s sentence at organization level: what it sets here, and why it cannot be chosen when the instance forbids it. */
function orgSentence(id: PresetId, part: OrgPart, why: string | null): string {
  const text =
    id === "open"
      ? PRESETS.open.sentence
      : id === "balanced"
        ? part === "internet"
          ? "Agents reach any website but ask before changing data on sites they have no account for."
          : "Only the organization\x27s AI keys, and agents run on Aurora."
        : PRESETS.locked_down.sentence;
  return why ? `${text} ${why}.` : text;
}
