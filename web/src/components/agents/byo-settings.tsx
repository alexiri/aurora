"use client";

import { useState, type FormEvent } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { usePutAgentByo } from "@/lib/api/agents";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { splitList } from "@/lib/api/governance";
import type { Agent } from "@/lib/api/queries";

/**
 * (phase 2, RFE-025) A BYO agent's governance: effective posture, source CIDR list and the
 * require-context flag. Editing needs `can_own`; the posture itself is set on the workspace and
 * organization (tighten-only), so this card only reports it.
 */
export function ByoSettings({ agent, wsId, canOwn }: { agent: Agent; wsId: string | undefined; canOwn: boolean }) {
  const byo = agent.byo;
  const put = usePutAgentByo(wsId, agent.id);
  const [cidrs, setCidrs] = useState((byo?.source_cidrs ?? []).join("\n"));
  const [requireContext, setRequireContext] = useState(byo?.require_context ?? false);
  const [saved, setSaved] = useState(false);
  if (!byo) return null;
  const list = splitList(cidrs);
  const fe = put.error ? fieldErrors(put.error) : {};
  const disabled = byo.posture === "disabled";

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    put.mutate(
      { source_cidrs: list.length ? list : null, require_context: requireContext },
      { onSuccess: () => setSaved(true) },
    );
  }

  return (
    <Card className="space-y-4" aria-labelledby="byo-h">
      <h2 id="byo-h" className="font-display text-base font-semibold text-ink">
        Access controls
      </h2>
      {disabled ? (
        <Notice tone="danger" role="status">
          BYO disabled by {byo.posture_level}: this agent&apos;s token is refused and live tunnels were closed.
          Nothing was deleted; loosening the setting restores it without a new token.
        </Notice>
      ) : (
        <p className="text-sm text-ink-2">
          Posture <Badge tone="accent">{byo.posture}</Badge>{" "}
          <span className="text-muted">set by {byo.posture_level}</span>
        </p>
      )}
      <dl className="grid gap-1 text-sm sm:grid-cols-[12rem_1fr]">
        <dt className="text-muted">Effective source list</dt>
        <dd data-testid="effective-cidrs">
          {byo.effective_source_cidrs ? (
            <>
              <span className="font-mono">{byo.effective_source_cidrs.join(", ")}</span>{" "}
              <span className="text-muted">from {byo.effective_source_level}</span>
            </>
          ) : (
            "Unrestricted"
          )}
        </dd>
        <dt className="text-muted">Context credentials</dt>
        <dd>
          {byo.effective_require_context ? (
            <>
              Required <span className="text-muted">by {byo.effective_require_context_level}</span>
            </>
          ) : (
            "Optional"
          )}
        </dd>
      </dl>
      <form onSubmit={submit} noValidate className="space-y-3">
        <fieldset disabled={!canOwn || put.isPending} className="space-y-3">
          <Field
            id="byo-agent-cidrs"
            label="Source CIDR list for this agent"
            hint="One CIDR per line, at most 32, inside the organization or instance default. Empty = inherit. The address is the TLS peer (or a trusted load balancer's PROXY address), never a header."
            error={put.error ? (Object.values(fe)[0] ?? errorMessage(put.error)) : undefined}
          >
            <textarea
              id="byo-agent-cidrs"
              rows={3}
              className={`${inputClass} font-mono`}
              value={cidrs}
              placeholder="203.0.113.0/24"
              onChange={(e) => setCidrs(e.target.value)}
            />
          </Field>
          <CheckboxField
            id="byo-require-context"
            label="Require context credentials"
            hint={
              <>
                Requests that need a credential must come from a labelled run (<code>aurora run</code>).
              </>
            }
            checked={requireContext || byo.effective_require_context_level === "instance" || byo.effective_require_context_level === "org"}
            disabled={byo.effective_require_context_level === "instance" || byo.effective_require_context_level === "org"}
            onChange={setRequireContext}
          />
        </fieldset>
        {!canOwn ? <p className="text-xs text-muted">Only workspace owners can change these settings.</p> : null}
        <FormActions>
          {canOwn ? (
            <Button type="submit" disabled={put.isPending}>
              {put.isPending ? "Saving…" : "Save access controls"}
            </Button>
          ) : null}
          {saved ? (
            <span role="status" className="text-sm text-accent-ink">
              Saved. Live connections were re-checked.
            </span>
          ) : null}
        </FormActions>
      </form>
    </Card>
  );
}
