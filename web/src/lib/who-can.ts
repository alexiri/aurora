import { ApiError } from "@/lib/api/client";
import type { components } from "@/lib/api/schema";

/** (C-981) The server's answer to "who can fix this?": the action, the role, up to three names, a path when the viewer can. */
export type WhoCanValue = components["schemas"]["WhoCan"];
export type WhoCanRole = components["schemas"]["WhoCanRole"];
/** `contacts` of a workspace or an organization (C-981): the people to ask where nothing was refused. */
export type Contacts = components["schemas"]["Contacts"];

/** The role phrases are fixed (`14 §Who can fix this`). Instance admins are never named. */
export const ROLE_PHRASE: Record<WhoCanRole, string> = {
  workspace_owner: "workspace owners",
  workspace_approver: "workspace owners and approvers",
  org_admin: "organization owners and admins",
  org_owner: "organization owners",
  instance_admin: "an Aurora administrator",
};

/**
 * What an action key reads as after "can": `Only workspace owners can <phrase>.` The server sends a snake_case key
 * (`view_this` and `change_this` when a refusal does not know better); a key the dashboard has no words for reads
 * as its own words rather than failing.
 */
export const ACTION_PHRASE: Record<string, string> = {
  view_this: "open this page",
  change_this: "change this",
  add_account: "add an account",
  add_accounts: "add an account",
  connect_app: "connect an app",
  connect_chat: "connect a chat platform",
  give_access: "give agents access",
  add_rule: "add a rule",
  add_schedule: "set up a schedule",
  add_skill: "add a skill",
  add_agent: "add an agent",
  invite_people: "invite people",
  add_people: "add people",
  set_up: "set this up",
  add_instance: "add the app's instance again",
};

export function actionPhrase(action: string): string {
  return ACTION_PHRASE[action] ?? action.replace(/_/g, " ");
}

/** `A`, `A or B`, `A, B or C`. */
export function orList(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} or ${names[names.length - 1]}`;
}

const capital = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

/** "Only workspace owners can add an account." */
export function onlySentence(w: Pick<WhoCanValue, "role" | "action">): string {
  return `Only ${ROLE_PHRASE[w.role]} can ${actionPhrase(w.action)}.`;
}

/**
 * The people to ask. With names and `withRole`: "Ask Ada Lovelace or Alan Turing (organization owners and admins)."
 * (the role is said nowhere else); without `withRole` (a sentence before it already named the role): "Ask Bob Builder."
 * An instance admin is never named: "An Aurora administrator can set this up." With no names for any other role the
 * role itself is asked: "Ask workspace owners."
 */
export function askSentence(w: Pick<WhoCanValue, "role" | "people">, withRole = true): string {
  if (w.role === "instance_admin") return "An Aurora administrator can set this up.";
  const names = w.people.map((p) => p.name).filter(Boolean);
  if (names.length === 0) return `Ask ${ROLE_PHRASE[w.role]}.`;
  return `Ask ${orList(names)}${withRole ? ` (${ROLE_PHRASE[w.role]})` : ""}.`;
}

/** The link text for a viewer who can do it themselves: "Add an account". */
export function actionLabel(action: string): string {
  return capital(actionPhrase(action));
}

/**
 * A `WhoCan` from the `contacts` of the place the viewer is in, for somewhere nothing was refused (an empty state, a
 * page header). The viewer cannot act here, or the caller would show the action instead.
 */
export function whoCanFromContacts(role: WhoCanRole, contacts: Contacts | null | undefined, action: string): WhoCanValue {
  const people =
    role === "instance_admin"
      ? []
      : role === "org_admin"
        ? (contacts?.admins ?? contacts?.owners ?? [])
        : (contacts?.owners ?? []);
  return { action, role, people: people.slice(0, 3), you_can: false };
}

/** A refusal made on the page, carrying `who_can` like the server's 403 (C-915): never a bare `{ code }`. */
export function forbiddenError(whoCan: WhoCanValue): ApiError {
  return new ApiError("forbidden", "You do not have permission to do that.", { who_can: whoCan }, 403);
}

/** The `who_can` a refusal carries (`details.who_can`), if it has a well-formed one. */
export function whoCanOf(err: unknown): WhoCanValue | null {
  const details = err instanceof ApiError ? err.details : typeof err === "object" && err !== null ? (err as { details?: unknown }).details : null;
  if (typeof details !== "object" || details === null) return null;
  const w = (details as { who_can?: unknown }).who_can;
  if (typeof w !== "object" || w === null) return null;
  const { role, action, people } = w as Partial<WhoCanValue>;
  if (typeof role !== "string" || !(role in ROLE_PHRASE) || typeof action !== "string" || !Array.isArray(people)) return null;
  return w as WhoCanValue;
}

/** Who may open an organization page that allows `roles`: owners and admins together, or owners alone. */
export function orgRoleFor(allow: ReadonlyArray<string>): WhoCanRole {
  return allow.includes("admin") ? "org_admin" : "org_owner";
}
