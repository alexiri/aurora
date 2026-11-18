"use client";

import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import type { components } from "@/lib/api/schema";
import { droppedRoleWarning, ROLES, roleLabel, roleWarning, subagentWarning, type Role, ROLE_LABEL } from "@/lib/model-roles";

type DefaultModel = components["schemas"]["DefaultModel"];

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
 * default endpoint's allowed models; readiness (owners only) supplies the endpoint's own roles and the warnings.
 */
export function ModelRolesFields({
  harnessId,
  harnessName,
  roleless,
  mapped = null,
  models,
  roles,
  onRoles,
  subagent,
  onSubagent,
  readiness,
  errors,
  disabled,
}: {
  harnessId: string | undefined;
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
  readiness: DefaultModel | null;
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
  const dropped = readiness?.roles_dropped ?? [];
  const warnings: string[] = [];
  for (const role of ROLES) {
    const v = readiness?.roles[role];
    // Readiness judges what is saved: an unsaved choice has no warning yet.
    const saved = v?.source === "agent" ? (v.model_id ?? "") : "";
    if (v?.warning && roles[role] === saved) warnings.push(roleWarning(role, v.warning, v.model_id));
  }
  for (const r of dropped) warnings.push(droppedRoleWarning(r));
  if (readiness?.subagent_warning && subagent === readiness.subagent_model) {
    warnings.push(subagentWarning(readiness.subagent_model, readiness.subagent_warning));
  }
  return (
    <div className="space-y-4" role="group" aria-labelledby="a-roles-title">
      <div className="space-y-1">
        <h3 id="a-roles-title" className="text-sm font-semibold text-ink">
          Model roles
        </h3>
        <p className="text-xs text-muted">
          The model each role stands for. The harness picks a role for subagents and background work; a role left empty
          follows the endpoint.
        </p>
      </div>
      {ROLES.map((role) => {
        const id = `a-role-${role}`;
        const view = readiness?.roles[role];
        const endpointModel = view && view.source === "endpoint" ? view.model_id : view?.source === "none" ? null : undefined;
        const empty =
          endpointModel === undefined
            ? "Use the endpoint's setting"
            : `Use the endpoint's setting (${endpointModel ?? "none"})`;
        // (C-1016) "Not used by <display name>" for whichever role the harness does not map.
        const unused = mapped !== null && !mapped.includes(role);
        return (
          <Field
            key={role}
            id={id}
            label={roleLabel(role, harnessId)}
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
              {ROLE_LABEL[r]}
            </option>
          ))}
        </select>
      </Field>
      {warnings.map((w) => (
        <Notice key={w} tone="pending" role="status" data-testid="role-warning">
          {w}
        </Notice>
      ))}
    </div>
  );
}
