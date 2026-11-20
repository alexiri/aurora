"use client";

import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { ROLES, ROLE_NAME, type Role } from "@/lib/model-roles";

export type RoleDraft = Record<Role, string>;
export const EMPTY_ROLES: RoleDraft = { large: "", medium: "", small: "" };

export interface RoleFieldErrors {
  /** Whole-group errors (`model_role_endpoint_mismatch`, a field the server did not name). */
  group?: string;
  roles: Partial<Record<Role, string>>;
  subagent?: string;
}

/**
 * The **Model roles** group and **Subagents use** of the Instructions tab (D-302, D-303, C-876). `models` are the
 * default endpoint's allowed models. (P5-REFINE-12) Roles are named for what they do, an unset role reads "Same as the
 * default", and no endpoint detail shows here.
 */
export function ModelRolesFields({
  harnessName,
  roleless,
  mapped = null,
  models,
  roles,
  onRoles,
  subagent,
  onSubagent,
  errors,
  disabled,
}: {
  harnessName: string;
  /** The harness maps no roles (`GET /v1/harnesses` `model_roles = []`). */
  roleless: boolean;
  /** The roles the harness maps (`model_roles`); `null` while unknown. A role it does not map says so (C-1016). */
  mapped?: readonly string[] | null;
  models: { model_id: string; display_name?: string | null }[];
  roles: RoleDraft;
  onRoles: (r: RoleDraft) => void;
  subagent: string;
  onSubagent: (s: string) => void;
  errors: RoleFieldErrors;
  disabled: boolean;
}) {
  if (roleless) {
    return (
      <Notice role="status" data-testid="roles-not-used">
        Not used by {harnessName}.
      </Notice>
    );
  }
  // (P5-POLISH-6, D-385) The role warnings are not on this page; the endpoint shows them (P5-POLISH-10, C-1034).
  return (
    <div className="space-y-4" role="group" aria-labelledby="a-roles-title">
      <div className="space-y-1">
        <h3 id="a-roles-title" className="text-sm font-semibold text-ink">
          Models for different jobs
        </h3>
        <p className="text-xs text-muted">
          The agent picks a model for subagents and background work. Anything left on “Same as the default” uses the
          default model.
        </p>
      </div>
      {ROLES.map((role) => {
        const id = `a-role-${role}`;
        // (P5-REFINE-12, behaviour-5) Never "Use the endpoint's setting (none)": an unset role follows the default.
        const empty = "Same as the default";
        // (C-1016) "Not used by <display name>" for whichever role the harness does not map.
        const unused = mapped !== null && !mapped.includes(role);
        return (
          <Field
            key={role}
            id={id}
            label={ROLE_NAME[role]}
            error={errors.roles[role]}
            hint={unused ? `Not used by ${harnessName}.` : undefined}
          >
            <select id={id} className={inputClass} value={roles[role]} disabled={disabled || unused} onChange={(e) => onRoles({ ...roles, [role]: e.target.value })}>
              <option value="">{empty}</option>
              {roles[role] && !models.some((m) => m.model_id === roles[role]) ? <option value={roles[role]}>{roles[role]}</option> : null}
              {models.map((m) => (
                <option key={m.model_id} value={m.model_id}>
                  {m.display_name ?? m.model_id}
                </option>
              ))}
            </select>
          </Field>
        );
      })}
      {errors.group ? (
        <Notice tone="danger" role="alert">
          {errors.group}
        </Notice>
      ) : null}
      <Field id="a-subagent" label="Subagents use" error={errors.subagent} hint="The model of subagents whose definition names none.">
        <select id="a-subagent" className={inputClass} value={subagent} disabled={disabled} onChange={(e) => onSubagent(e.target.value)}>
          <option value="inherit">Same as the default</option>
          {ROLES.filter((r) => mapped === null || mapped.includes(r)).map((r) => (
            <option key={r} value={r}>
              {ROLE_NAME[r]}
            </option>
          ))}
        </select>
      </Field>
    </div>
  );
}
