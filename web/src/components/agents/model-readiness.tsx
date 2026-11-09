"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { useModelReadiness, type KeyReadiness } from "@/lib/api/credential-grants";

/** Why a granted key is not usable, in words, and the fix (`08 §Model readiness`). */
export const REASON_COPY: Record<string, { text: string; fix: string; where?: "keys" | "governance" }> = {
  not_active: { text: "The key is not active (a pending proposal or a rejected one).", fix: "Accept it or replace the value.", where: "keys" },
  endpoint_unknown: { text: "The key's endpoint is no longer configured.", fix: "Ask your operator, or add a key on a configured endpoint.", where: "keys" },
  provider_disabled: { text: "The endpoint's provider is disabled on this instance.", fix: "Ask your operator to enable it." },
  scope_not_permitted: { text: "Governance does not permit this key scope on the endpoint.", fix: "An organization admin can change this.", where: "governance" },
  origin_unverified: { text: "The endpoint requires a verified account and this key is not verified.", fix: "Re-verify the key.", where: "keys" },
  origin_not_compliant: { text: "The key is verified, but its account is not an allowed one.", fix: "Use a key from an allowed account.", where: "keys" },
  no_shared_dialect: { text: "The endpoint serves no dialect this agent's harness speaks.", fix: "Ask your operator to add a dialect, or use another harness." },
};

const SOURCE_COPY: Record<string, string> = {
  override: "the model chosen for this agent",
  endpoint_default: "the endpoint's default model",
  provider_default: "the provider's default model",
  allow_list: "the first model in the allow-list",
  static_list: "the first model in the endpoint's list",
};

function KeyRow({ k, base, orgBase }: { k: KeyReadiness; base: string; orgBase: string }) {
  const reason = k.reason ? (REASON_COPY[k.reason] ?? { text: k.reason, fix: "" }) : null;
  const keysHref = `${k.scope === "organization" ? orgBase : base}/credentials?tab=llm`;
  const href = reason?.where === "governance" ? `${orgBase}/governance` : reason?.where === "keys" ? keysHref : null;
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
            <Notice role="status">No LLM key is granted to this agent. Grant one below, or add a key under Credentials.</Notice>
          ) : (
            <ItemList aria-label="Granted LLM keys">
              {readiness.data.keys.map((k) => (
                <KeyRow key={k.secret_id} k={k} base={base} orgBase={orgBase} />
              ))}
            </ItemList>
          )}
          {readiness.data.override_dropped ? (
            <Notice tone="pending" role="status">
              The model chosen for this agent is not usable with these keys, so the next turn will ignore it.
            </Notice>
          ) : null}
          {readiness.data.keys.length > 0 ? (
            d ? (
              <p className="text-sm text-ink-2" data-testid="next-model">
                The next turn uses <span className="font-mono text-ink">{d.model_id}</span> on{" "}
                <span className="font-mono text-ink">{d.endpoint_id}</span> ({SOURCE_COPY[d.source] ?? d.source}).
              </p>
            ) : (
              <Notice tone="pending" role="status">
                No model chosen —{" "}
                <Link href={`${pathname}?tab=instructions`} className="font-semibold underline">
                  choose one on Instructions
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
