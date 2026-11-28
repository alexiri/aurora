"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { useModelReadiness, type KeyReadiness } from "@/lib/api/credential-grants";
import { useOrgMemberOnly } from "@/lib/nav/org-member";
import type { components } from "@/lib/api/schema";
import { droppedRoleWarning, ROLE_LABEL, ROLE_SOURCE_COPY, ROLES, roleWarning, subagentWarning, type Role } from "@/lib/model-roles";

type DefaultModel = components["schemas"]["DefaultModel"];

/** Why a granted key is not usable, in words, and the fix (`08 §Model readiness`). */
export const REASON_COPY: Record<string, { text: string; fix: string; where?: "keys" | "governance" }> = {
  not_active: { text: "The key is not active (a pending proposal or a rejected one).", fix: "Accept it or replace the value.", where: "keys" },
  endpoint_unknown: { text: "The key's endpoint is no longer configured.", fix: "Ask your operator, or add a key on a configured endpoint.", where: "keys" },
  provider_disabled: { text: "The endpoint's provider is disabled on this instance.", fix: "Ask your operator to enable it." },
  scope_not_permitted: { text: "Your organization does not permit this key scope on the endpoint.", fix: "An organization admin can change this.", where: "governance" },
  origin_unverified: { text: "The endpoint requires a verified account and this key is not verified.", fix: "Re-verify the key.", where: "keys" },
  origin_not_compliant: { text: "The key is verified, but its account is not an allowed one.", fix: "Use a key from an allowed account.", where: "keys" },
  no_shared_dialect: { text: "The endpoint serves no dialect this agent's harness speaks.", fix: "Ask your operator to add a dialect, or use another harness." },
};

/** (FIX-170, C-1026) Readiness warnings of the default selection, one line each. */
export const READINESS_WARNING_COPY: Record<string, string> = {
  default_model_not_served: "The endpoint's default model is not in your key's model list, so it is skipped.",
  override_not_served: "This agent's model is not in your key's model list. It is still used.",
};

const SOURCE_COPY: Record<string, string> = {
  override: "the model chosen for this agent",
  endpoint_default: "the endpoint's default model",
  provider_default: "the provider's default model",
  key_listing: "the first model your key serves",
  allow_list: "the first model in the allow-list",
  static_list: "the first model in the endpoint's list",
};

function KeyRow({ k, base, orgBase }: { k: KeyReadiness; base: string; orgBase: string }) {
  const reason = k.reason ? (REASON_COPY[k.reason] ?? { text: k.reason, fix: "" }) : null;
  // (D-508(e)) A member doesn't see Manage: a fix that lives there is said, not linked.
  const member = useOrgMemberOnly(decodeURIComponent(orgBase.split("/")[2] ?? ""));
  const ofOrg = reason?.where === "governance" || (reason?.where === "keys" && k.scope === "organization");
  const keysHref = `${k.scope === "organization" ? orgBase : base}/apps?tab=accounts&kind=ai-key`;
  const target = reason?.where === "governance" ? `${orgBase}/ai-models` : reason?.where === "keys" ? keysHref : null;
  const href = member && ofOrg ? null : target;
  return (
    <ListRow actions={<Badge tone={k.usable ? "accent" : "danger"}>{k.usable ? "Ready" : "Not usable"}</Badge>}>
      <div className="min-w-0 space-y-0.5 text-sm">
        <p className="flex flex-wrap items-center gap-x-2">
          <span className="break-words font-medium text-ink">{k.name}</span>
          <span className="text-xs text-muted">
            {k.scope} · {k.provider} · {k.endpoint_id}
          </span>
        </p>
        {reason ? (
          <p className="text-xs text-danger-ink">
            {reason.text} {href ? (
              <Link href={href} className="font-semibold underline">
                {reason.fix}
              </Link>
            ) : (
              reason.fix
            )}
          </p>
        ) : null}
      </div>
    </ListRow>
  );
}

/** (phase 5, P5-MODELS-3, C-878) The resolved roles with their source, the subagent default, the warnings and the dropped roles. */
function RolesView({ d, instructionsHref }: { d: DefaultModel; instructionsHref: string }) {
  const notes: string[] = [];
  for (const role of ROLES) {
    const v = d.roles[role];
    if (v.warning) notes.push(roleWarning(role, v.warning, v.model_id));
  }
  for (const r of d.roles_dropped) notes.push(droppedRoleWarning(r));
  if (d.subagent_warning) notes.push(subagentWarning(d.subagent_model, d.subagent_warning));
  return (
    <div className="space-y-2" data-testid="model-roles">
      <ItemList aria-label="Model roles">
        {ROLES.map((role) => {
          const v = d.roles[role];
          return (
            <ListRow key={role}>
              <p className="flex flex-wrap items-center gap-x-2 text-sm">
                <span className="font-medium text-ink">{ROLE_LABEL[role]}</span>
                {v.model_id ? <span className="break-all font-mono text-ink">{v.model_id}</span> : <span className="text-muted">none</span>}
                <span className="text-xs text-muted">{ROLE_SOURCE_COPY[v.source] ?? v.source}</span>
              </p>
            </ListRow>
          );
        })}
      </ItemList>
      <p className="text-sm text-ink-2" data-testid="subagent-model">
        Subagents use {d.subagent_model === "inherit" ? "the same model as the default" : ROLE_LABEL[d.subagent_model as Role] ?? d.subagent_model}.
      </p>
      {notes.map((n) => (
        <Notice key={n} tone="pending" role="status" data-testid="role-warning">
          {n}{" "}
          <Link href={instructionsHref} className="font-semibold underline">
            Change the roles on Instructions
          </Link>
          .
        </Notice>
      ))}
    </div>
  );
}

/** Hosted agents, owners only (C-500): each granted LLM key as Ready or its reason with the fix, and the model the next turn uses. */
export function ModelReadinessSection({ agentId, base }: { agentId: string; base: string }) {
  const readiness = useModelReadiness(agentId);
  const pathname = usePathname() ?? "";
  const orgBase = base.split("/w/")[0];
  const d = readiness.data?.default ?? null;
  return (
    <Section id="model-readiness" title="Model readiness" description="Which granted LLM keys this agent can use, and the model its next turn will use.">
      {readiness.isPending ? <Loading /> : null}
      {readiness.isError ? <ErrorState error={readiness.error} onRetry={() => readiness.refetch()} /> : null}
      {readiness.data ? (
        <div className="space-y-3">
          {readiness.data.keys.length === 0 ? (
            <Notice role="status">This agent has no AI key. Give it one below, or add a key under Apps & accounts.</Notice>
          ) : (
            <ItemList aria-label="AI keys this agent uses">
              {readiness.data.keys.map((k) => (
                <KeyRow key={k.secret_id} k={k} base={base} orgBase={orgBase} />
              ))}
            </ItemList>
          )}
          {(readiness.data.warnings ?? []).map((w) => (
            <Notice key={w} tone="pending" role="status" data-testid="readiness-warning">
              {READINESS_WARNING_COPY[w] ?? w}
            </Notice>
          ))}
          {readiness.data.override_dropped ? (
            <Notice tone="pending" role="status">
              The model chosen for this agent is not usable with these keys, so the next turn will ignore it.
            </Notice>
          ) : null}
          {readiness.data.keys.length > 0 ? (
            d ? (
              <>
                <p className="text-sm text-ink-2" data-testid="next-model">
                  The next turn uses <span className="font-mono text-ink">{d.model_id}</span> on{" "}
                  <span className="font-mono text-ink">{d.endpoint_id}</span> ({SOURCE_COPY[d.source] ?? d.source}).
                </p>
                <RolesView d={d} instructionsHref={`${pathname}?tab=instructions`} />
              </>
            ) : (
              <Notice tone="pending" role="status">
                No model chosen, and Aurora could not pick one: the key&apos;s model list could not be read, or it has no allowed chat model.{" "}
                <Link href={`${pathname}?tab=instructions#a-model`} className="font-semibold underline">
                  Choose one on Instructions
                </Link>
                .
              </Notice>
            )
          ) : null}
        </div>
      ) : null}
    </Section>
  );
}
