/**
 * Model roles in words (P5-MODELS-3, D-302, D-303; C-875, C-876, C-878): the role vocabulary, the alias `claude-code`
 * maps each role to, the warnings readiness reports, and the client-side check of a model id. The server stays
 * authoritative for every one of them.
 */

export const ROLES = ["large", "medium", "small"] as const;
export type Role = (typeof ROLES)[number];

export const ROLE_LABEL: Record<Role, string> = { large: "Large", medium: "Medium", small: "Small" };

/** The alias each role maps to in `claude-code` (`08 §Model roles`). */
export const CLAUDE_CODE_ALIAS: Record<Role, string> = { large: "opus", medium: "sonnet", small: "haiku" };

/** `MODEL_ID_PATTERN` (C-875): `^[A-Za-z0-9._:/@-]{1,200}$`. */
export const MODEL_ID_PATTERN = /^[A-Za-z0-9._:/@-]{1,200}$/;

export const isRole = (v: string): v is Role => (ROLES as readonly string[]).includes(v);

/** `Large` or, for `claude-code`, `Large (opus)`. */
export function roleLabel(role: Role, harnessId: string | undefined): string {
  return harnessId === "claude-code" ? `${ROLE_LABEL[role]} (${CLAUDE_CODE_ALIAS[role]})` : ROLE_LABEL[role];
}

/** A role's readiness warning (`role_unset` | `role_not_allowed`) in words. */
export function roleWarning(role: Role, warning: string, model?: string | null): string {
  const name = ROLE_LABEL[role];
  if (warning === "role_not_allowed") {
    return `${name}${model ? ` (${model})` : ""} is not allowed for your organization, so it counts as not set.`;
  }
  return `${name} is not set. On this endpoint, subagents and background requests may ask for a model it does not serve.`;
}

/** `subagent_warning` in words. */
export function subagentWarning(subagent: string, warning: string): string {
  const name = isRole(subagent) ? ROLE_LABEL[subagent] : subagent;
  return warning === "subagent_role_unset"
    ? `Subagents use ${name}, which is not set, so they may ask for a model this endpoint does not serve.`
    : warning;
}

/** A role the agent set that is no longer usable and is left out of the next turn. */
export function droppedRoleWarning(role: string): string {
  const name = isRole(role) ? ROLE_LABEL[role] : role;
  return `The ${name} model set for this agent is no longer usable, so the next turn leaves it out. Choose another.`;
}

/** The field a 422 `model_id_invalid` / `llm_model_not_allowed` names (`details.field`): a role, `subagent_model` or the model. */
export function errorTarget(field: unknown): { role?: Role; subagent?: true; model?: true } {
  const f = typeof field === "string" ? field : "";
  const role = ROLES.find((r) => f === r || f.endsWith(`.${r}`) || f.endsWith(`/${r}`));
  if (role) return { role };
  if (f.includes("subagent")) return { subagent: true };
  return { model: true };
}

/** Where a resolved role comes from (`RoleView.source`). */
export const ROLE_SOURCE_COPY: Record<string, string> = {
  agent: "set on this agent",
  endpoint: "from the endpoint",
  none: "not set",
};
