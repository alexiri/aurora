import type { AltitudeId } from "@/lib/altitude-registry";

/**
 * The agent page's five sections (D-200, docs/14 §Agent page (phase 5)) and the one mapping from a page id to its
 * section. The URL keeps `?tab=<page>` with the page ids the page has always had, so every deep link opens the page it
 * opened before; the section is derived from the page.
 */

export type AgentPageId =
  | "chat"
  | "instructions"
  | "memory"
  | "skills"
  | "evals"
  | "automations"
  | "schedules"
  | "webhooks"
  | "channels"
  | "peers"
  | "computer"
  | "terminal"
  | "files"
  | "previews"
  | "ssh"
  | "connect"
  | "access";

/** (P5-REFINE-11, D-408) `settings` is the Settings page behind the labelled Settings button; it replaces Behaviour and Access. */
export type SectionId = "chat" | "triggers" | "computer" | "settings";

/** Every page id, for `useTabParam` (unknown `?tab=` values fall back). */
export const AGENT_PAGE_IDS: readonly AgentPageId[] = [
  "chat",
  "instructions",
  "memory",
  "skills",
  "evals",
  "automations",
  "schedules",
  "webhooks",
  "channels",
  "peers",
  "computer",
  "terminal",
  "files",
  "previews",
  "ssh",
  "connect",
  "access",
];

/**
 * (P5-REFINE-14, D-405) Schedules, Webhooks and Channels are one page, **Automations** (`?tab=automations`). The old ids keep
 * working: each opens Automations with its kind as the filter, so every deep link opens what it opened before.
 */
export type AutomationFilter = "all" | "schedules" | "webhooks" | "chats";
export const LEGACY_AUTOMATION_TAB: Partial<Record<AgentPageId, AutomationFilter>> = { schedules: "schedules", webhooks: "webhooks", channels: "chats" };

export const PAGE_LABEL: Record<AgentPageId, string> = {
  chat: "Chat",
  instructions: "Instructions",
  memory: "Memory",
  skills: "Skills",
  evals: "Tests",
  automations: "Automations",
  schedules: "Schedules",
  webhooks: "Webhooks",
  channels: "Channels",
  peers: "Other agents",
  // (P5-REFINE-15, D-408) The Computer's sub-nav reads Files · Terminal · Connect · Previews · Details.
  computer: "Details",
  terminal: "Terminal",
  files: "Files",
  previews: "Previews",
  ssh: "Connect",
  connect: "Computer: Connect",
  access: "Apps & access",
};

/** Sections in order, each with its pages in order. */
export const SECTIONS: readonly { id: SectionId; label: string; pages: readonly AgentPageId[] }[] = [
  { id: "chat", label: "Chat", pages: ["chat"] },
  { id: "triggers", label: "Automations", pages: ["automations"] },
  // (P5-REFINE-15) Files first, then the Expert pages, then Details (the old Overview body); a BYO agent's `connect` stays alone.
  { id: "computer", label: "Computer", pages: ["files", "terminal", "ssh", "previews", "computer", "connect"] },
  // (D-408) One page with a sub-nav; Pause and Delete sit in its danger zone. `peers` (Other agents) moved here from Automations
  // with its page id kept (P5-REFINE-12 renames and fills it).
  { id: "settings", label: "Settings", pages: ["instructions", "access", "memory", "skills", "evals", "peers"] },
];

/** The one-line lead under a Settings page's name (the old menu's descriptions). */
export const SETTINGS_LEAD: Partial<Record<AgentPageId, string>> = {
  instructions: "Name, instructions, model",
  access: "Apps, accounts and the AI key it uses",
  memory: "What it remembers",
  skills: "What it knows how to do",
  evals: "Checks that tell you whether it still does the job",
  peers: "Which other agents it may ask",
};

/** Page id → its section (the one mapping). */
export const SECTION_OF: Record<AgentPageId, SectionId> = {
  ...(Object.fromEntries(SECTIONS.flatMap((s) => s.pages.map((p) => [p, s.id] as const))) as Record<AgentPageId, SectionId>),
  // The old ids open Automations (see `resolvePage`).
  schedules: "triggers",
  webhooks: "triggers",
  channels: "triggers",
};

export interface PageAvailability {
  /** A BYO agent has only Instructions, Computer: Connect and Access, and no Chat. */
  byo: boolean;
  /** The instance runs previews and the organization switched them on (P4-PRV-1). */
  previews: boolean;
  /** The instance's `terminal.enabled` (P4-TERM). */
  terminal: boolean;
  /** (P4-SSH-1, C-795) The instance's `features.ssh`; absent means off. */
  ssh?: boolean;
}

export function isPageAvailable(page: AgentPageId, av: PageAvailability): boolean {
  if (av.byo) return page === "instructions" || page === "connect" || page === "access";
  if (page === "connect") return false;
  // (P5-FILES-2, D-316(b)) Files exists exactly where the Terminal does: no gate of its own.
  if (page === "terminal" || page === "files") return av.terminal;
  if (page === "previews") return av.previews;
  // (P4-SSH-1) SSH: hosted agents on an instance with `ssh.enabled`; independent of the terminal's switch.
  if (page === "ssh") return av.ssh === true;
  return true;
}

export interface AvailableSection {
  id: SectionId;
  label: string;
  pages: { id: AgentPageId; label: string }[];
}

/**
 * (P5-REFINE-3, D-406) The altitude of the pages that are not Use: Memory and Skills are Set up, the rest of this table
 * Expert (registry ids in `lib/altitude-registry.ts`). Altitude hides a page from the navigation only: a direct
 * `?tab=` still opens it (`resolvePage` does not look at it).
 */
export const PAGE_ALTITUDE: Partial<Record<AgentPageId, { id: AltitudeId; level: "setup" | "expert" }>> = {
  memory: { id: "agent.memory", level: "setup" },
  skills: { id: "agent.skills", level: "setup" },
  evals: { id: "agent.tests", level: "expert" },
  peers: { id: "agent.otheragents", level: "expert" },
  terminal: { id: "agent.terminal", level: "expert" },
  ssh: { id: "agent.ssh", level: "expert" },
  previews: { id: "agent.previews", level: "expert" },
  connect: { id: "agent.connect", level: "expert" },
};

/** Does the viewer's altitude list this page in the navigation? `allows` comes from `useAltitude()`. */
export type PageAllows = (level: "setup" | "expert") => boolean;

function listed(page: AgentPageId, allows: PageAllows | undefined): boolean {
  const a = PAGE_ALTITUDE[page];
  return !a || !allows || allows(a.level);
}

/** How many available pages the altitude leaves out of the navigation (the page footer counts them). */
export function hiddenPageCount(av: PageAvailability, allows: PageAllows): number {
  return AGENT_PAGE_IDS.filter((p) => isPageAvailable(p, av) && !listed(p, allows)).length;
}

/**
 * The sections that have at least one available page, in order, with their available pages. With `allows`, pages above
 * the viewer's altitude are left out of the navigation, except `current` (the page that is open, e.g. by a direct link).
 */
export function availableSections(av: PageAvailability, allows?: PageAllows, current?: AgentPageId): AvailableSection[] {
  return SECTIONS.map((s) => ({
    id: s.id,
    label: s.label,
    pages: s.pages
      .filter((p) => isPageAvailable(p, av) && (p === current || listed(p, allows)))
      .map((id) => ({ id, label: pageLabel(id, av) })),
  })).filter((s) => s.pages.length > 0);
}

/** The page's name in the sub-nav: Details, or Overview when the instance has no Files (terminal off), where it is the state card. */
export function pageLabel(id: AgentPageId, av: PageAvailability): string {
  return id === "computer" && !av.terminal ? "Overview" : PAGE_LABEL[id];
}

/** (P5-REFINE-15) Where the Computer section opens: Files when they exist, else Overview. Other sections open on their first page. */
export function sectionLanding(section: SectionId, pages: readonly { id: AgentPageId }[]): AgentPageId | undefined {
  if (section === "computer") return pages.find((p) => p.id === "files")?.id ?? pages.find((p) => p.id === "computer")?.id ?? pages[0]?.id;
  return pages[0]?.id;
}

/** A hosted agent opens on Chat, a BYO agent on Instructions. */
export const defaultPage = (av: PageAvailability): AgentPageId => (av.byo ? "instructions" : "chat");

/** The page to show for a requested one: itself when available, otherwise the default. */
export function resolvePage(requested: string | null | undefined, av: PageAvailability): AgentPageId {
  const found = AGENT_PAGE_IDS.find((p) => p === requested);
  // (P5-REFINE-14) The old Schedules / Webhooks / Channels ids open the one Automations page.
  const id = found && LEGACY_AUTOMATION_TAB[found] ? "automations" : found;
  return id && isPageAvailable(id, av) ? id : defaultPage(av);
}
