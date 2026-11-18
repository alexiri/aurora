import { errorMessage } from "@/lib/api/errors";
import { ApiError } from "@/lib/api/client";
import type { Level } from "@/lib/api/accounts";
import type { AppDetail, ConnectMethod, ConnectPlan, ConnectRequest, SetupMethod } from "@/lib/api/apps";
import { appIsCustomOnly, buildConnectFields } from "@/lib/api/apps";
import { LEVEL_ORDER } from "@/lib/accounts";
import { safeHref } from "@/components/ui/markdown";

/**
 * The pure part of the connect journey (C-914, `14 §3`): which steps a person sees, the choices each offers, the words
 * of the Done step and of a failure, and the body of the one request that makes the account and the access.
 */

export type StepId = "app" | "agent" | "account" | "access" | "signin" | "done";

export const STEP_LABEL: Record<StepId, string> = {
  app: "App",
  agent: "Agent",
  account: "Account",
  access: "Access",
  signin: "Sign in",
  done: "Done",
};

/** What decides the steps. Everything here is known before the step it affects is reached. */
export interface JourneyShape {
  /** The app was given (a deep link, a Connect button, the Done step's other app): no App step. */
  appGiven: boolean;
  /** A fixed agent (the agent's own page): no Agent step. */
  agentFixed: boolean;
  /** The workspace has no agent to choose from: no Agent step (the account is only made). */
  noAgents: boolean;
  /** "Another website or API" was chosen. */
  website: boolean;
  /** An account of this app that the person may give exists. */
  hasExisting: boolean;
  /** The person chose "Use <name>" on the Account step. */
  useExisting: boolean;
  /** There is a choice to make: the app can change things (websites, AI keys and read-only apps have none). */
  hasChoice: boolean;
  /** No agent will be given access (the Agent step was left empty): the Access step is skipped. */
  noAgentChosen: boolean;
}

/** The steps that apply, in order; the Done step is always last. */
export function stepsOf(s: JourneyShape): StepId[] {
  const out: StepId[] = [];
  if (!s.appGiven) out.push("app");
  if (!s.agentFixed && !s.noAgents) out.push("agent");
  if (!s.website && s.hasExisting) out.push("account");
  if (!s.website && s.hasChoice && !s.noAgentChosen) out.push("access");
  if (!(s.hasExisting && s.useExisting && !s.website)) out.push("signin");
  out.push("done");
  return out;
}

export function nextStep(s: JourneyShape, from: StepId): StepId {
  const steps = stepsOf(s);
  const i = steps.indexOf(from);
  return steps[Math.min(i + 1, steps.length - 1)] ?? "done";
}

export function prevStep(s: JourneyShape, from: StepId): StepId | null {
  const steps = stepsOf(s);
  const i = steps.indexOf(from);
  return i > 0 ? steps[i - 1]! : null;
}

/** The three choices an app account offers a new access (`14 §2`): none for an app that cannot change anything. */
export function levelsForApp(app: Pick<AppDetail, "tools" | "app_restrictions">): Level[] {
  if (!app.tools.some((t) => t.group === "write")) return [];
  return appIsCustomOnly(app) ? ["read", "ask"] : [...LEVEL_ORDER];
}

/** The Done sentences of `14 §3`, one per access. `level` is null for a website or an app with no choice. */
export function doneSentence({ agent, app, level, agentChosen }: { agent: string; app: string; level: Level | null; agentChosen: boolean }): string {
  if (!agentChosen) return `${app} is connected. No agent can use it yet.`;
  switch (level) {
    case "read":
      return `${agent} can now read ${app}.`;
    case "ask":
      return `${agent} can now read ${app}, and asks you before each change.`;
    case "act":
      return `${agent} can now read and change ${app} without asking.`;
    default:
      return `${agent} can now use ${app}.`;
  }
}

/** The permissions to tick on the app's token page for the chosen access: "repo (read), read:org". */
export function tickList(method: Pick<ConnectMethod, "scopes" | "scope_presets">, level: Level | null): string[] {
  const ids = level === "read" || level === null ? method.scope_presets.read : method.scope_presets.write;
  return ids.map((id) => method.scopes.find((s) => s.scope === id)?.name ?? id);
}

/** The scopes the request asks for at the chosen access (`scope_presets`, C-956); an Advanced edit wins. */
export function scopesFor(method: Pick<ConnectMethod, "scopes" | "scope_presets">, level: Level | null, override?: string[] | null): string[] {
  if (override) return override;
  return level === "read" || level === null ? method.scope_presets.read : method.scope_presets.write;
}

/** The suggested method of the plan, if it is one that works (C-955). */
export function suggestedMethod(plan: ConnectPlan | undefined, app: Pick<AppDetail, "connect">): { method: ConnectMethod; setup: SetupMethod } | null {
  const id = plan?.suggested_method;
  if (!plan || !id) return null;
  const setup = plan.methods.find((m) => m.id === id);
  const method = app.connect.find((m) => m.id === id);
  return setup && method && setup.ready ? { method, setup } : null;
}

export interface SubmitInput {
  workspaceId: string;
  method: ConnectMethod;
  fields: Record<string, string>;
  /** The agents given access in the same request; none = only the account is made. */
  agentIds: string[];
  /** The access; null where there is no choice (it is "read" on the wire, which allows every read the app has). */
  level: Level | null;
  /** The app can change things: without an agent the account is made read-only. */
  hasWriteTools: boolean;
  /** The Advanced edits. */
  scopesOverride?: string[] | null;
  instanceId?: string;
  clientId?: string;
  accessLevel?: string;
  /** A pasted token's scopes the person ticked by hand (FIX-136), as in the old form. */
  scopes?: string[] | null;
  /** (P5-UX-11, C-936) The organization mode: the account is for the whole organization and `level` is its limit. */
  org?: { orgId: string };
}

/** The steps of the organization's journey (C-936): no Agent step; the Access step sets the account's limit. */
export type OrgStepId = "app" | "account" | "access" | "signin" | "done";

export const ORG_STEP_LABEL: Record<OrgStepId, string> = { app: "App", account: "Account", access: "Limit", signin: "Sign in", done: "Done" };

export function orgStepsOf(s: { appGiven: boolean; hasExisting: boolean; useExisting: boolean; hasChoice: boolean }): OrgStepId[] {
  const out: OrgStepId[] = [];
  if (!s.appGiven) out.push("app");
  if (s.hasExisting) out.push("account");
  if (!(s.hasExisting && s.useExisting) && s.hasChoice) out.push("access");
  if (!(s.hasExisting && s.useExisting)) out.push("signin");
  out.push("done");
  return out;
}

/** The Done sentence of the organization's journey (`14 §3`). */
export function orgDoneSentence(app: string, org: string): string {
  return `${app} is connected for ${org}. No agent can use it yet.`;
}

/** `POST /v1/apps/{app}/connect` (C-957): the method, the fields, the scopes of the chosen level and the grant. */
export function buildJourneyConnect(i: SubmitInput): ConnectRequest {
  const fields = buildConnectFields(i.method.fields, i.fields);
  const scopes = i.scopes ?? (i.method.scopes.length > 0 ? scopesFor(i.method, i.level, i.scopesOverride) : []);
  if (i.org) {
    return {
      scope: "organization",
      org_id: i.org.orgId,
      method_id: i.method.id,
      ...(Object.keys(fields).length ? { fields } : {}),
      ...(i.clientId ? { oauth_client_id: i.clientId } : {}),
      ...(i.instanceId ? { instance_id: i.instanceId } : {}),
      ...(i.accessLevel ? { access_level: i.accessLevel } : {}),
      ...(scopes.length > 0 ? { scopes } : {}),
      ...(i.hasWriteTools ? { limit: i.level ?? "read" } : {}),
    };
  }
  return {
    scope: "workspace",
    workspace_id: i.workspaceId,
    method_id: i.method.id,
    ...(Object.keys(fields).length ? { fields } : {}),
    ...(i.clientId ? { oauth_client_id: i.clientId } : {}),
    ...(i.instanceId ? { instance_id: i.instanceId } : {}),
    ...(i.accessLevel ? { access_level: i.accessLevel } : {}),
    ...(scopes.length > 0 ? { scopes } : {}),
    ...(i.agentIds.length > 0 ? { grant: { agent_ids: i.agentIds, level: i.level ?? "read" } } : i.hasWriteTools ? { access: "read" } : {}),
  };
}

/** C-958 in the journey's words: names the app, and for an unreachable one its host. */
export function connectFailure(err: unknown, app: string): string {
  if (err instanceof ApiError && err.code === "connect_probe_failed") {
    const d = (err.details ?? {}) as { cause?: unknown; host?: unknown; upstream_status?: unknown };
    if (d.cause === "rejected") return `${app} did not accept this token. Check that it is complete and has not expired.`;
    if (d.cause === "unreachable") return `Aurora could not reach ${app}${typeof d.host === "string" ? ` (${d.host})` : ""}.`;
    if (d.cause === "unexpected") return `${app} answered with an unexpected error${d.upstream_status != null ? ` (${String(d.upstream_status)})` : ""}.`;
  }
  return errorMessage(err);
}

/** The host of what a person typed as a website: "https://api.example.com/v1" or "api.example.com" → "api.example.com". */
export function hostOf(input: string): string {
  const t = input.trim();
  if (!t) return "";
  try {
    return new URL(t.includes("://") ? t : `https://${t}`).host;
  } catch {
    return t;
  }
}

/** The app's token page as a button's `href`: the app's note link, else the first field's help link. */
export function tokenPage(app: Pick<AppDetail, "connect_note">, method: Pick<ConnectMethod, "fields">): string | null {
  const url = app.connect_note?.url ?? method.fields.find((f) => f.help_url)?.help_url ?? null;
  return url ? safeHref(url) : null;
}
