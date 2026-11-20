"use client";

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button, buttonVariants } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { useCopy } from "@/lib/use-copy";
import { cn } from "@/lib/utils";
import {
  agentSnippets,
  probeGatewayCa,
  useContainerConfig,
  useRegenerateToken,
} from "@/lib/api/agents";
import { expiryText, lastUsedText } from "@/lib/api/agent-tokens";
import type { Agent } from "@/lib/api/queries";

import { ByoSettings } from "./byo-settings";
import { AgentRevealDialog, type Revealed } from "./reveal";
import { TokenBadges } from "./token-badges";

function Snippet({ label, text }: { label: string; text: string }) {
  const { state, copy } = useCopy();
  return (
    <div className="space-y-2">
      <p className="text-sm font-medium text-ink">{label}</p>
      <pre className="overflow-auto rounded-control border border-line bg-subtle p-3 font-mono text-xs text-ink">{text}</pre>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="outline" size="sm" onClick={() => copy(text)}>
          Copy {label}
        </Button>
        {state !== "idle" ? (
          <span role="status" className={state === "copied" ? "text-sm text-accent-ink" : "text-sm text-danger-ink"}>
            {state === "copied" ? "Copied" : "Your browser blocked copying; select the text instead."}
          </span>
        ) : null}
      </div>
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
      setRevealed({ token: res.token, config: res.container_config, agentName: agent.name, expiresAt: res.expires_at });
      regen.reset();
    } catch {
      // surfaced via regen.error
    }
  }

  return (
    <div className="max-w-3xl space-y-8">
      <ByoSettings agent={agent} wsId={wsId} canOwn={canOwn} />
      <Section
        id="tok"
        title="Agent token"
        description="Regenerating revokes the current token and any context credentials."
      >
        {agent.byo?.token ? (
          <div data-testid="token-info" className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-ink-2">
            <span className="font-mono text-ink">{agent.byo.token.display_prefix}…</span>
            <span>{expiryText(agent.byo.token.expires_at)}</span>
            <span>{lastUsedText(agent.byo.token.last_used_at)}</span>
            <TokenBadges token={agent.byo.token} />
          </div>
        ) : (
          <p data-testid="token-info" className="text-sm text-ink-2">
            This agent has no live token. Regenerate one to connect it.
          </p>
        )}
        {regenBlocked ? (
          <Notice role="status">{regenBlocked}</Notice>
        ) : (
          <div>
            <Button variant="outline" onClick={() => setConfirming(true)}>
              Regenerate token
            </Button>
          </div>
        )}
        <ConfirmDialog
          open={confirming}
          onOpenChange={(o) => {
            setConfirming(o);
            if (!o) regen.reset();
          }}
          title="Regenerate the agent token?"
          description={`The old token for @${agent.handle} stops working now. Whatever uses it stops until you give it the new token, which is shown once.`}
          confirmLabel="Regenerate token"
          danger
          busy={regen.isPending}
          onConfirm={() => void regenerate()}
        >
          <ErrorNotice error={regen.error} />
        </ConfirmDialog>
        {regen.error && !confirming ? (
          <ErrorNotice error={regen.error} />
        ) : null}
      </Section>

      <Section id="cfg" title="Container config" description="How to connect this agent's computer to the gateway.">
        {config.isPending ? <Loading /> : null}
        {config.isError ? <ErrorState error={config.error} onRetry={() => config.refetch()} /> : null}
        {config.data ? (
          <pre data-testid="container-config" className="max-h-64 overflow-auto rounded-control border border-line bg-subtle p-3 font-mono text-xs text-ink">
            {JSON.stringify(config.data, null, 2)}
          </pre>
        ) : null}
        <Snippet label="aurora login" text={snippets.login} />
        <Snippet label="aurora run" text={snippets.run} />
        {snippets.env ? <Snippet label="env exports" text={snippets.env} /> : null}
        {config.data?.notes?.map((n, i) => (
          <Notice key={i} tone="pending" role="note">
            {n}
          </Notice>
        ))}
        <p className="text-sm text-ink-2">
          Replace <code>&lt;AGENT_TOKEN&gt;</code> with your agent token
          {agent.byo?.effective_require_context ? (
            <>
              {" "}(or <code>&lt;CONTEXT_TOKEN&gt;</code> with a credential minted by <code>aurora run</code>)
            </>
          ) : null}
          .
        </p>
      </Section>

      <Section id="ca" title="Gateway CA certificate" description="Trust this certificate so the proxy can inspect TLS traffic.">
        {ca.isError ? (
          <ErrorNotice error={ca.error} />
        ) : (
          <div>
            <a
              className={cn(buttonVariants({ variant: "outline", size: "sm" }))}
              href="/v1/gateway/ca.pem"
              download="aurora-gateway-ca.pem"
            >
              Download CA certificate
            </a>
          </div>
        )}
      </Section>

      <AgentRevealDialog revealed={revealed} onClose={() => setRevealed(null)} />
    </div>
  );
}
