/**
 * (P5-UX-9, C-932, `14 §Agents, accounts and access` §4) The words of an app's setup page: one sentence per way to
 * connect with its state chip, the headline, the picker label of an OAuth client, and which actions the viewer's
 * `can` lets the page offer. Pure functions over the setup status of C-931, so the table is unit-tested without a page.
 */
import type { AppSetup } from "@/lib/api/apps";
import type { components } from "@/lib/api/schema";

type Method = components["schemas"]["SetupMethod"];
type Need = components["schemas"]["SetupNeed"];
type Can = components["schemas"]["SetupCan"];

/** The page is read for a workspace owner (`workspace`) or for an organization owner or admin (`organization`). */
export type SetupView = "workspace" | "organization";

/** `ready`, `needs_setup` (the viewer can do it here) and `admin_only` (someone above the viewer has to). */
export type SetupChip = "ready" | "needs_setup" | "admin_only";

export const CHIP_LABEL: Record<SetupChip, string> = {
  ready: "Ready",
  needs_setup: "Needs setup",
  admin_only: "Admin only",
};

/** Where an OAuth client may be set up, for the sentence "none is set up for …". */
function placesPhrase(view: SetupView, orgName?: string): string {
  const org = orgName ? orgName : "the organization";
  return view === "workspace" ? `this workspace, ${org} or Aurora` : `${org} or Aurora`;
}

/** A label that starts a word ("Personal access token") reads after "with a/an"; a name or acronym ("GitHub App", "API key") stays as written. */
function withLabel(label: string): string {
  const first = label.split(/\s+/)[0] ?? "";
  if (/^(sign in|log in)/i.test(label)) return label;
  // An acronym ("API key", "SSH key") keeps its capitals and takes the article its first letter's name asks for.
  if (/^[A-Z]{2,5}$/.test(first)) return `${"AEFHILMNORSX".includes(first[0]!) ? "an" : "a"} ${label}`;
  const proper = /[A-Z]/.test(first.slice(1));
  if (proper) return label;
  const lower = label.charAt(0).toLowerCase() + label.slice(1);
  return `${/^[aeiou]/i.test(lower) ? "an" : "a"} ${lower}`;
}

/** Can the viewer fix this need themselves on this page? */
export function needIsFixable(need: Need, view: SetupView, can: Can): boolean {
  switch (need) {
    case "oauth_client":
      return view === "workspace" ? can.register_workspace_client : can.register_org_client;
    case "instance":
      return can.register_instance;
    case "admin_material":
      return false;
  }
}

/** The state chip of a way to connect: ready; needing something the viewer can set up here; or needing someone above them. */
export function methodChip(m: Pick<Method, "ready" | "needs">, view: SetupView, can: Can): SetupChip {
  if (m.ready) return "ready";
  if (m.needs.length > 0 && m.needs.every((n) => needIsFixable(n, view, can))) return "needs_setup";
  return "admin_only";
}

/** The one sentence of a way to connect (§4). */
export function methodSentence(
  appName: string,
  m: Pick<Method, "label" | "kind" | "ready" | "needs">,
  view: SetupView,
  opts: { orgName?: string; available?: boolean } = {},
): string {
  const { orgName, available = true } = opts;
  if (m.ready) {
    if (m.kind === "oauth2") return `People can ${/^sign in/i.test(m.label) ? m.label.charAt(0).toLowerCase() + m.label.slice(1) : `connect ${appName} with ${m.label}`}.`;
    return `People can connect ${appName} with ${withLabel(m.label)}.`;
  }
  const parts: string[] = [];
  if (m.needs.includes("oauth_client")) parts.push(`needs an OAuth client: none is set up for ${placesPhrase(view, orgName)}`);
  if (m.needs.includes("instance")) parts.push(`needs a self-hosted ${appName} server to be registered first`);
  if (m.needs.includes("admin_material"))
    parts.push(`needs details from whoever runs your ${appName} organization, such as an app ID and a private key`);
  if (parts.length === 0)
    return available ? `${m.label} is not ready.` : `${m.label} is switched off: ${appName} is not available${orgName ? ` in ${orgName}` : " here"}.`;
  return `${m.label} ${parts.join(", and ")}.`;
}

/** The sentence under the title: what works today and what does not yet. */
export function setupHeadline(appName: string, methods: Pick<Method, "label" | "audience" | "ready">[], view: SetupView): string {
  const people = methods.filter((m) => m.audience === "person");
  const ready = people.filter((m) => m.ready);
  const waiting = people.filter((m) => !m.ready);
  const where = view === "workspace" ? "People here" : "People";
  if (people.length === 0) return `${appName} has no way for people to connect it yet.`;
  const readyPart =
    ready.length === 0
      ? `${where} cannot connect ${appName} yet.`
      : `${where} can connect ${appName} with ${listOr(ready.map((m) => withLabel(m.label)))} today.`;
  if (waiting.length === 0) return readyPart;
  return `${readyPart} ${listAnd(waiting.map((m) => m.label))} ${waiting.length === 1 ? "needs" : "need"} setup first.`;
}

function listOr(items: string[]): string {
  return items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} or ${items[items.length - 1]}`;
}
function listAnd(items: string[]): string {
  return items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * (C-946) A client is named by the ways of connecting it serves: "GitHub — for Sign in with GitHub". With several
 * methods they are joined with " and "; a client no method uses (or an old API) falls back to "GitHub".
 */
export function clientLabel(appName: string, usedBy: { label: string }[] | undefined): string {
  const labels = (usedBy ?? []).map((m) => m.label);
  return labels.length > 0 ? `${appName} — for ${listAnd(labels)}` : appName;
}

/** What the page offers the viewer, from the server's `can` (the dashboard only decides what to show). */
export interface SetupActions {
  /** **Connect** (the journey, for the workspace or, in the organization view, for the whole organization). */
  connect: boolean;
  /** Register, edit and remove an OAuth client at the viewer's level. */
  registerClient: boolean;
  /** Register a self-hosted app (the Self-hosted apps tab with `?package=`). */
  registerInstance: boolean;
  /** Change which apps the organization allows. */
  setAvailability: boolean;
}

export function setupActions(view: SetupView, setup: Pick<AppSetup, "can" | "available">): SetupActions {
  const { can } = setup;
  return {
    connect: setup.available && (view === "workspace" ? can.connect_workspace : can.connect_org),
    registerClient: view === "workspace" ? can.register_workspace_client : can.register_org_client,
    // The server's `register_instance` is already false for an app that cannot run on a server people host.
    registerInstance: view === "organization" && can.register_instance,
    setAvailability: view === "organization" && can.set_availability,
  };
}

/** The sentence of the "Available in <org>" row and the header chip. */
export function availabilityWords(available: boolean, orgName?: string): string {
  return available ? `Available${orgName ? ` in ${orgName}` : ""}` : `Not available${orgName ? ` in ${orgName}` : ""}`;
}
