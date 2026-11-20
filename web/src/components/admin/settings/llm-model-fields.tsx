"use client";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { ROLE_LABEL, ROLES, type Role } from "@/lib/model-roles";

export interface InfoRowDraft {
  k: number;
  id: string;
  ctx: string;
  caps: string[];
}

/** The capabilities `model_info` accepts (C-875). */
export const CAPABILITIES: { id: string; label: string }[] = [
  { id: "effort", label: "Effort" },
  { id: "thinking", label: "Thinking" },
  { id: "adaptive_thinking", label: "Adaptive thinking" },
  { id: "interleaved_thinking", label: "Interleaved thinking" },
];

/**
 * (phase 5, P5-MODELS-3, C-875) **Model roles** of an endpoint: the strongest, everyday and fastest model (D-426), each one of the endpoint's
 * models or free text. A non-Anthropic endpoint without roles gets the notice that agents will ask for Claude names.
 */
export function ModelRolesFieldset({
  provider,
  roles,
  onChange,
  offered,
  errors,
  serverError,
}: {
  provider: string;
  roles: Record<Role, string>;
  onChange: (r: Record<Role, string>) => void;
  /** The endpoint's models as typed (models, allowed models, default model). */
  offered: string[];
  /** Client-side check per role. */
  errors: Partial<Record<Role, string>>;
  /** The server's `value_invalid` for `model_roles`. */
  serverError?: string;
}) {
  const none = ROLES.every((r) => !roles[r].trim());
  return (
    <fieldset className="space-y-3 rounded-control border border-line p-3">
      <legend className="px-1 text-sm font-medium text-ink">Model roles</legend>
      <p className="text-xs text-muted">
        The model each role stands for on this endpoint. Pick one of its models or type an id. Agents can override a role.
      </p>
      <datalist id="ep-role-models">
        {offered.map((m) => (
          <option key={m} value={m} />
        ))}
      </datalist>
      <div className="grid gap-3 sm:grid-cols-3">
        {ROLES.map((r) => (
          <Field key={r} id={`ep-role-${r}`} label={ROLE_LABEL[r]} error={errors[r]}>
            <input
              id={`ep-role-${r}`}
              list="ep-role-models"
              spellCheck={false}
              autoComplete="off"
              className={`${inputClass} font-mono`}
              value={roles[r]}
              onChange={(e) => onChange({ ...roles, [r]: e.target.value })}
            />
          </Field>
        ))}
      </div>
      {serverError ? (
        <p role="alert" className="text-xs text-danger-ink">
          {serverError}
        </p>
      ) : null}
      {provider !== "anthropic" && none ? (
        <Notice tone="pending" role="status" data-testid="no-roles-notice">
          Agents on this endpoint will ask for Claude model names for subagents and background work. Set the roles.
        </Notice>
      ) : null}
    </fieldset>
  );
}

/** (P5-MODELS-3, C-875) **Model details**: a keyed list, model id → context window and capabilities. */
export function ModelDetailsFieldset({
  rows,
  onChange,
  errors,
  serverError,
  max,
}: {
  rows: InfoRowDraft[];
  onChange: (rows: InfoRowDraft[]) => void;
  errors: Record<number, { id?: string; ctx?: string }>;
  serverError?: string;
  max: number;
}) {
  const set = (k: number, patch: Partial<InfoRowDraft>) => onChange(rows.map((r) => (r.k === k ? { ...r, ...patch } : r)));
  const next = rows.reduce((m, r) => Math.max(m, r.k + 1), 0);
  return (
    <fieldset className="space-y-3 rounded-control border border-line p-3">
      <legend className="px-1 text-sm font-medium text-ink">Model details</legend>
      <p className="text-xs text-muted">For models the harness does not recognise, such as non-Claude models behind LiteLLM.</p>
      {rows.length === 0 ? <p className="text-xs text-muted">No model has details yet.</p> : null}
      <ul className="space-y-3" aria-label="Model details">
        {rows.map((r, i) => (
          <li key={r.k} className="space-y-3 rounded-control border border-line p-3">
            <div className="grid gap-3 sm:grid-cols-2">
              <Field id={`ep-info-id-${r.k}`} label={`Model id ${i + 1}`} error={errors[r.k]?.id}>
                <input
                  id={`ep-info-id-${r.k}`}
                  spellCheck={false}
                  className={`${inputClass} font-mono`}
                  value={r.id}
                  onChange={(e) => set(r.k, { id: e.target.value })}
                />
              </Field>
              <Field id={`ep-info-ctx-${r.k}`} label={`Context window ${i + 1}`} error={errors[r.k]?.ctx} hint="Tokens. Optional.">
                <input
                  id={`ep-info-ctx-${r.k}`}
                  inputMode="numeric"
                  className={inputClass}
                  value={r.ctx}
                  onChange={(e) => set(r.k, { ctx: e.target.value })}
                />
              </Field>
            </div>
            <div role="group" aria-label={`Capabilities of model ${i + 1}`} className="flex flex-wrap gap-x-5 gap-y-2">
              {CAPABILITIES.map((c) => (
                <CheckboxField
                  key={c.id}
                  id={`ep-info-${r.k}-${c.id}`}
                  label={c.label}
                  checked={r.caps.includes(c.id)}
                  onChange={(on) => set(r.k, { caps: on ? [...r.caps, c.id] : r.caps.filter((x) => x !== c.id) })}
                />
              ))}
            </div>
            <Button type="button" size="sm" variant="outline" aria-label={`Remove details of model ${i + 1}`} onClick={() => onChange(rows.filter((x) => x.k !== r.k))}>
              Remove
            </Button>
          </li>
        ))}
      </ul>
      {serverError ? (
        <p role="alert" className="text-xs text-danger-ink">
          {serverError}
        </p>
      ) : null}
      <Button type="button" size="sm" variant="outline" disabled={rows.length >= max} onClick={() => onChange([...rows, { k: next, id: "", ctx: "", caps: [] }])}>
        Add model details
      </Button>
    </fieldset>
  );
}
