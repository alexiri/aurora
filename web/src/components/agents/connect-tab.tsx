"use client";

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import {
  agentSnippets,
  probeGatewayCa,
  useContainerConfig,
  useRegenerateToken,
} from "@/lib/api/agents";
import { errorMessage } from "@/lib/api/errors";
import type { Agent } from "@/lib/api/queries";

import { ByoSettings } from "./byo-settings";
import { AgentRevealDialog, type Revealed } from "./reveal";

function Snippet({ label, text }: { label: string; text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="space-y-1">
      <p className="text-sm font-medium">{label}</p>
      <pre className="overflow-auto rounded-md bg-neutral-100 p-3 text-xs">{text}</pre>
      <Button
        variant="outline"
        size="sm"
        onClick={() => void navigator.clipboard?.writeText(text).then(() => setCopied(true))}
      >
        Copy {label}
      </Button>
      {copied ? (
        <span role="status" className="ml-2 text-sm text-green-700">
          Copied
        </span>
      ) : null}
    </div>
  );
}

export function ConnectTab({ agent, wsId, canOwn = false }: { agent: Agent; wsId?: string; canOwn?: boolean }) {
  const config = useContainerConfig(agent.id, true);
  const regen = useRegenerateToken(agent.id);
  const ca = useQuery({ queryKey: ["gateway-ca"] as const, queryFn: probeGatewayCa, retry: false });
  const [confirming, setConfirming] = useState(false);
  const [revealed, setRevealed] = useState<Revealed | null>(null);
  const snippets = agentSnippets(agent.handle, config.data);
  // (RFE-025) Regeneration follows the creation rule: hidden for non-owners under `owners_only`.
  const regenBlocked =
    agent.byo?.posture === "disabled"
      ? `BYO is disabled by ${agent.byo.posture_level}; tokens cannot be regenerated until it is loosened.`
      : agent.byo?.posture === "owners_only" && !canOwn
        ? "Under the owners-only BYO posture only workspace owners can regenerate this token."
        : null;

  async function regenerate() {
    try {
      const res = await regen.mutateAsync();
      setConfirming(false);
      setRevealed({ token: res.token, config: res.container_config, agentName: agent.name });
      regen.reset();
    } catch {
      // surfaced via regen.error
    }
  }

  return (
    <div className="max-w-3xl space-y-6">
      <ByoSettings agent={agent} wsId={wsId} canOwn={canOwn} />
      <section className="space-y-2" aria-labelledby="tok-h">
        <h2 id="tok-h" className="text-lg font-medium">
          Agent token
        </h2>
        <p className="text-sm text-neutral-600">
          Regenerating revokes the current token and any context credentials.
        </p>
        {regenBlocked ? (
          <p role="status" className="text-sm text-neutral-600">
            {regenBlocked}
          </p>
        ) : confirming ? (
          <div role="alertdialog" aria-label="Confirm token regeneration" className="space-y-2 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm">
            <p>Regenerate the token for @{agent.handle}? The previous token stops working immediately.</p>
            <div className="flex gap-2">
              <Button onClick={() => void regenerate()} disabled={regen.isPending}>
                {regen.isPending ? "Regenerating…" : "Yes, regenerate"}
              </Button>
              <Button variant="outline" onClick={() => setConfirming(false)}>
                Cancel
              </Button>
            </div>
          </div>
        ) : (
          <Button variant="outline" onClick={() => setConfirming(true)}>
            Regenerate token
          </Button>
        )}
        {regen.error ? (
          <p role="alert" className="text-sm text-red-700">
            {errorMessage(regen.error)}
          </p>
        ) : null}
      </section>

      <section className="space-y-3" aria-labelledby="cfg-h">
        <h2 id="cfg-h" className="text-lg font-medium">
          Container config
        </h2>
        {config.isPending ? <Loading /> : null}
        {config.isError ? <ErrorState error={config.error} onRetry={() => config.refetch()} /> : null}
        {config.data ? (
          <pre data-testid="container-config" className="max-h-64 overflow-auto rounded-md bg-neutral-100 p-3 text-xs">
            {JSON.stringify(config.data, null, 2)}
          </pre>
        ) : null}
        <Snippet label="aurora login" text={snippets.login} />
        <Snippet label="aurora run" text={snippets.run} />
        {snippets.env ? <Snippet label="env exports" text={snippets.env} /> : null}
        {config.data?.notes?.map((n, i) => (
          <p key={i} role="note" className="rounded-md bg-amber-50 p-3 text-sm text-amber-900">
            {n}
          </p>
        ))}
        <p className="text-sm text-neutral-600">
          Replace <code>&lt;AGENT_TOKEN&gt;</code> with your agent token
          {agent.byo?.effective_require_context ? (
            <>
              {" "}(or <code>&lt;CONTEXT_TOKEN&gt;</code> with a credential minted by <code>aurora run</code>)
            </>
          ) : null}
          .
        </p>
      </section>

      <section className="space-y-2" aria-labelledby="ca-h">
        <h2 id="ca-h" className="text-lg font-medium">
          Gateway CA certificate
        </h2>
        {ca.isError ? (
          <p role="alert" className="text-sm text-red-700">
            {errorMessage(ca.error)}
          </p>
        ) : (
          <a className="text-sm underline" href="/v1/gateway/ca.pem" download="aurora-gateway-ca.pem">
            Download CA certificate
          </a>
        )}
      </section>

      <AgentRevealDialog revealed={revealed} onClose={() => setRevealed(null)} />
    </div>
  );
}
