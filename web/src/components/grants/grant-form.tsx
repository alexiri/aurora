"use client";

import { Badge } from "@/components/ui/badge";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Segmented } from "@/components/ui/segmented";
import {
  ASK_TIMEOUT_MAX,
  ASK_TIMEOUT_MIN,
  GROUPS,
  counts,
  groupOf,
  groupTools,
  grantValueProblem,
  setGroup,
  timeoutProblem,
  type Decision,
  type GrantErrors,
  type GrantValue,
  type Tool,
} from "@/lib/grant-editor";

import { PermissionGroups, type PermissionGroup, type PermissionRow } from "./permission-groups";

const DECISIONS = [
  { value: "allow", label: "Allow" },
  { value: "ask", label: "Ask" },
  { value: "block", label: "Block" },
] as const;

type Mode = GrantValue["mode"];

/**
 * The one editor of "what may an agent do through this connection" (FIX-147, D-233): Full access or Custom (Allow /
 * Ask / Block per tool, grouped Read and Write, group-wide setters, inert tools, the ask timeout). Controlled and
 * stateless: it fetches nothing and saves nothing. The Access tab's dialog and "Give agents access" both render it;
 * nowhere else lists an app's tools.
 */
export function GrantForm({
  appName,
  tools,
  inert = [],
  unavailable = [],
  customOnly,
  blockUntooled,
  value,
  onChange,
  errors = { tools: {} },
  disabled = false,
  idPrefix = "ge",
}: {
  appName: string;
  /** The tools that apply to this connection (`toolsForMethod` already applied). */
  tools: Tool[];
  /** Tool ids the grant names that the catalogue no longer has. */
  inert?: string[];
  /** (FIX-136, D-241) Tools the connection's token cannot serve (`splitByScopes`): listed collapsed, never selectable. */
  unavailable?: Tool[];
  /** The app allows only custom grants (`app_restrictions.custom_only`, or learned from a 422). */
  customOnly: boolean;
  blockUntooled: boolean;
  value: GrantValue;
  onChange: (next: GrantValue) => void;
  errors?: GrantErrors;
  disabled?: boolean;
  /** Prefix of the timeout field's id (two forms may be on one page). */
  idPrefix?: string;
}) {
  const effectiveMode: Mode = customOnly ? "custom" : value.mode;
  const anyAsk = counts(value.decisions).ask > 0;
  const timeoutHint = anyAsk ? timeoutProblem(value.timeout) : undefined;
  const problem = grantValueProblem(value, customOnly);

  const setDecision = (id: string, d: Decision) => onChange({ ...value, decisions: { ...value.decisions, [id]: d } });

  const groups: PermissionGroup[] = GROUPS.map((g) => ({
    id: g.value,
    label: g.label,
    rows: groupTools(tools, g.value).map(
      (t): PermissionRow => ({
        id: t.id,
        name: t.name,
        badges: t.wildcard ? <Badge>Umbrella</Badge> : undefined,
        detail: t.wildcard
          ? `Covers the whole ${groupOf(t)} group of ${t.host === "*" ? "the app" : t.host}.`
          : `${t.methods.join(", ")} ${t.host}${t.path}`,
        error: errors.tools[t.id] || undefined,
      }),
    ),
  }));

  return (
    <div className="space-y-4">
      {customOnly ? (
        <Notice tone="pending" role="status">
          This app only supports custom access: choose the tools the agent may use.
        </Notice>
      ) : (
        <div className="space-y-1.5">
          <p className="text-sm font-medium text-ink">Access to {appName}</p>
          <Segmented<Mode>
            label="Access mode"
            value={value.mode}
            disabled={disabled}
            onChange={(m) => onChange({ ...value, mode: m })}
            options={[
              { value: "full", label: "Full access" },
              { value: "custom", label: "Custom" },
            ]}
          />
          {value.mode === "full" ? (
            <p className="text-xs text-muted">Every tool of the app, including tools added by later updates.</p>
          ) : null}
        </div>
      )}
      {blockUntooled ? <Notice role="status">Requests that are not in the tool list are always blocked.</Notice> : null}

      {effectiveMode === "custom" ? (
        <div className="space-y-4">
          <p className="text-xs text-muted">
            Allow runs the tool, Ask holds it for an approval, Block refuses it. A tool you do not list stays blocked.
          </p>
          <PermissionGroups<Decision>
            groups={groups}
            options={[...DECISIONS]}
            values={value.decisions}
            fallback="block"
            disabled={disabled}
            onChange={setDecision}
            onChangeGroup={(g, d) => onChange({ ...value, decisions: setGroup(value.decisions, tools, g as "read" | "write", d) })}
            sectionLabel={(g) => `${g.label} tools`}
            groupControlLabel={(g) => `Set all ${g.label.toLowerCase()} tools`}
            rowControlLabel={(r) => `Access for ${r.name}`}
          />
          {inert.length > 0 ? (
            <section aria-label="Inert tools" className="space-y-1.5">
              <h3 className="text-sm font-semibold text-ink">Inert tools</h3>
              <p className="text-xs text-muted">
                An app update removed these tools. They grant nothing; set them to Block to drop them from this grant.
              </p>
              <ul className="divide-y divide-line rounded-control border border-line">
                {inert.map((id) => (
                  <li key={id} className="space-y-1.5 px-3 py-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <code className="min-w-0 break-all text-sm text-ink">{id}</code>
                      <Badge tone="pending">Inert</Badge>
                    </div>
                    {errors.tools[id] ? (
                      <p role="alert" className="text-xs text-danger-ink">
                        {errors.tools[id]}
                      </p>
                    ) : null}
                    <Segmented<Decision>
                      label={`Access for ${id}`}
                      value={value.decisions[id] ?? "block"}
                      options={[...DECISIONS]}
                      onChange={(d) => setDecision(id, d)}
                      disabled={disabled}
                    />
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          {anyAsk ? (
            <Field
              id={`${idPrefix}-timeout`}
              label="Approval timeout (seconds)"
              hint={`Empty = the instance default (5 min). From ${ASK_TIMEOUT_MIN} to ${ASK_TIMEOUT_MAX} s, at most the instance maximum (1 h unless raised).`}
              error={errors.timeout ?? timeoutHint}
            >
              <input
                id={`${idPrefix}-timeout`}
                inputMode="numeric"
                placeholder="default"
                className={inputClass + " w-32"}
                value={value.timeout}
                onChange={(e) => onChange({ ...value, timeout: e.target.value })}
                disabled={disabled}
              />
            </Field>
          ) : null}
        </div>
      ) : null}

      {unavailable.length > 0 ? (
        <details className="rounded-control border border-line px-3 py-2">
          <summary className="cursor-pointer text-sm font-medium text-ink">
            Not available with this connection&apos;s token ({unavailable.length})
          </summary>
          <p className="mt-1.5 text-xs text-muted">
            These tools need a scope the token does not have, so they cannot be granted. Reconnect with a token that has it, or
            edit the token&apos;s scopes on the connection if it does.
          </p>
          <ul className="mt-1.5 divide-y divide-line">
            {unavailable.map((t) => (
              <li key={t.id} className="space-y-0.5 py-1.5">
                <span className="text-sm text-ink">{t.name}</span>
                <span className="block break-words text-xs text-muted">Needs {(t.scopes ?? []).join(" or ")}</span>
              </li>
            ))}
          </ul>
        </details>
      ) : null}

      {problem === "empty" ? (
        <p role="status" className="text-xs text-muted">
          Allow or ask at least one tool to save.
        </p>
      ) : null}
    </div>
  );
}
