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

export type SectionId = "chat" | "behaviour" | "triggers" | "computer" | "access";

/** Every page id, for `useTabParam` (unknown `?tab=` values fall back). */
export const AGENT_PAGE_IDS: readonly AgentPageId[] = [
  "chat",
  "instructions",
  "memory",
  "skills",
  "evals",
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

export const PAGE_LABEL: Record<AgentPageId, string> = {
  chat: "Chat",
  instructions: "Instructions",
  memory: "Memory",
  skills: "Skills",
  evals: "Tests",
  schedules: "Schedules",
  webhooks: "Webhooks",
  channels: "Channels",
  peers: "Other agents",
  computer: "Computer",
  terminal: "Terminal",
  files: "Files",
  previews: "Previews",
  ssh: "SSH",
  connect: "Computer: Connect",
  access: "Apps & access",
};

/** Sections in order, each with its pages in order. */
export const SECTIONS: readonly { id: SectionId; label: string; pages: readonly AgentPageId[] }[] = [
  { id: "chat", label: "Chat", pages: ["chat"] },
  { id: "behaviour", label: "Instructions", pages: ["instructions", "memory", "skills", "evals"] },
  { id: "triggers", label: "Automations", pages: ["schedules", "webhooks", "channels", "peers"] },
  { id: "computer", label: "Computer", pages: ["computer", "terminal", "files", "previews", "ssh", "connect"] },
  { id: "access", label: "Apps & access", pages: ["access"] },
];

/** Page id → its section (the one mapping). */
export const SECTION_OF: Record<AgentPageId, SectionId> = Object.fromEntries(
  SECTIONS.flatMap((s) => s.pages.map((p) => [p, s.id] as const)),
) as Record<AgentPageId, SectionId>;

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
      .map((id) => ({ id, label: PAGE_LABEL[id] })),
  })).filter((s) => s.pages.length > 0);
}

/** A hosted agent opens on Chat, a BYO agent on Instructions. */
export const defaultPage = (av: PageAvailability): AgentPageId => (av.byo ? "instructions" : "chat");

/** The page to show for a requested one: itself when available, otherwise the default. */
export function resolvePage(requested: string | null | undefined, av: PageAvailability): AgentPageId {
  const id = AGENT_PAGE_IDS.find((p) => p === requested);
  return id && isPageAvailable(id, av) ? id : defaultPage(av);
}
