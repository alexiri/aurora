"use client";

import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { Field, inputClass } from "@/components/ui/field";
import {
  byoOptionsFrom,
  issuesUnder,
  splitList,
  type ByoForm,
  type ByoPosture,
  type EffectiveGovernance,
  type GovForm,
  type GovIssues,
} from "@/lib/api/governance";

const POSTURE_LABEL: Record<ByoPosture, string> = {
  enabled: "Enabled: any member who can use the workspace may create BYO agents",
  owners_only: "Owners only: workspace owners create BYO agents and regenerate their tokens",
  disabled: "Disabled: no BYO agents; existing tokens are refused until it is loosened",
};

const errorText = (m: string[]) => (m.length ? m.join(" ") : undefined);

/**
 * (phase 2, RFE-025) The org's BYO agent controls: posture (never below the instance floor),
 * default source CIDR list (inside the instance default) and the require-context default.
 */
export function ByoPanel({
  form,
  patch,
  issues,
  readOnly,
  eff,
}: {
  form: GovForm;
  patch: (p: Partial<GovForm>) => void;
  issues: GovIssues;
  readOnly: boolean;
  eff: EffectiveGovernance;
}) {
  const b = form.byo;
  const set = (p: Partial<ByoForm>) => patch({ byo: { ...b, ...p } });
  const options = byoOptionsFrom(b.floor);
  const postureLocked = b.instanceOff || b.floor === "disabled";
  const effective = eff.byo;
  const cidrCount = splitList(b.sourceCidrs).length;

  return (
    <div className="space-y-4">
      {b.instanceOff ? (
        <Card role="status" className="border-danger-ink">
          <p className="text-sm font-medium text-ink">
            BYO agents are disabled by the instance operator <Badge tone="danger">enforced by instance</Badge>
          </p>
          <p className="mt-1 text-sm text-ink-2">
            Creating BYO agents is refused and every existing BYO token is refused, whatever is set here.
          </p>
        </Card>
      ) : null}

      <Card className="space-y-4">
        <h2 className="text-base font-semibold text-ink">BYO agents</h2>
        <p className="text-sm text-ink-2">
          Effective posture:{" "}
          <Badge tone={effective.posture.value === "disabled" ? "danger" : "accent"}>{effective.posture.value}</Badge>{" "}
          <span className="text-muted">set by {effective.posture.source}</span>
        </p>
        <Field
          id="byo-posture"
          label="Who may create BYO agents"
          hint={`Options below the instance floor (${b.floor}) are not offered. Workspaces can tighten further.${postureLocked ? " Enforced by instance." : ""}`}
          error={errorText(issuesUnder(issues, "/byo/posture"))}
        >
          <select
            id="byo-posture"
            className={inputClass}
            value={b.posture}
            disabled={readOnly || postureLocked}
            onChange={(e) => set({ posture: e.target.value as ByoForm["posture"] })}
          >
            <option value="">Inherit the instance floor ({b.floor})</option>
            {options.map((p) => (
              <option key={p} value={p}>
                {POSTURE_LABEL[p]}
              </option>
            ))}
          </select>
        </Field>
      </Card>

      <Card className="space-y-4">
        <h2 className="text-base font-semibold text-ink">Where BYO tokens may be used from</h2>
        <Field
          id="byo-cidrs"
          label="Default source CIDR list"
          hint={
            b.instanceCidrs.length
              ? `One CIDR per line, at most 32. Every entry must lie inside the instance default: ${b.instanceCidrs.join(", ")}.`
              : "One CIDR per line, at most 32; /0 is not allowed. Empty = unrestricted (agents may narrow it)."
          }
          error={errorText(issuesUnder(issues, "/byo/source_cidrs"))}
        >
          <textarea
            id="byo-cidrs"
            rows={4}
            className={`${inputClass} font-mono`}
            value={b.sourceCidrs}
            disabled={readOnly || b.instanceOff}
            placeholder="203.0.113.0/24"
            onChange={(e) => set({ sourceCidrs: e.target.value })}
          />
        </Field>
        <p className="text-xs text-muted">
          {cidrCount > 0 ? `${cidrCount} of 32 entries. ` : ""}
          The address is the TLS peer of the proxy connection (or the PROXY protocol address from a
          configured trusted load balancer), never a request header. A list limits network position,
          not identity.
        </p>
        <label className="flex items-start gap-2 text-sm text-ink">
          <input
            type="checkbox"
            className="mt-0.5 size-4 shrink-0 accent-accent"
            checked={b.requireContext || b.requireContextLocked}
            disabled={readOnly || b.requireContextLocked || b.instanceOff}
            onChange={(e) => set({ requireContext: e.target.checked })}
          />
          <span>
            Require context credentials
            {b.requireContextLocked ? (
              <>
                {" "}
                <Badge>enforced by instance</Badge>
              </>
            ) : null}
            <span className="block text-xs text-muted">
              Requests that would inject a credential must come from a labelled run (
              <code>aurora run</code>), never from the plain agent token. Individual agents can also
              turn this on.
            </span>
          </span>
        </label>
      </Card>
    </div>
  );
}
