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
  | "schedules"
  | "webhooks"
  | "channels"
  | "peers"
  | "computer"
  | "terminal"
  | "previews"
  | "connect"
  | "access";

export type SectionId = "chat" | "behaviour" | "triggers" | "computer" | "access";

/** Every page id, for `useTabParam` (unknown `?tab=` values fall back). */
export const AGENT_PAGE_IDS: readonly AgentPageId[] = [
  "chat",
  "instructions",
  "memory",
  "skills",
  "schedules",
  "webhooks",
  "channels",
  "peers",
  "computer",
  "terminal",
  "previews",
  "connect",
  "access",
];

export const PAGE_LABEL: Record<AgentPageId, string> = {
  chat: "Chat",
  instructions: "Instructions",
  memory: "Memory",
  skills: "Skills",
  schedules: "Schedules",
  webhooks: "Webhooks",
  channels: "Channels",
  peers: "Peers",
  computer: "Computer",
  terminal: "Terminal",
  previews: "Previews",
  connect: "Computer: Connect",
  access: "Access",
};

/** Sections in order, each with its pages in order. */
export const SECTIONS: readonly { id: SectionId; label: string; pages: readonly AgentPageId[] }[] = [
  { id: "chat", label: "Chat", pages: ["chat"] },
  { id: "behaviour", label: "Behaviour", pages: ["instructions", "memory", "skills"] },
  { id: "triggers", label: "Triggers", pages: ["schedules", "webhooks", "channels", "peers"] },
  { id: "computer", label: "Computer", pages: ["computer", "terminal", "previews", "connect"] },
  { id: "access", label: "Access", pages: ["access"] },
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
}

export function isPageAvailable(page: AgentPageId, av: PageAvailability): boolean {
  if (av.byo) return page === "instructions" || page === "connect" || page === "access";
  if (page === "connect") return false;
  if (page === "terminal") return av.terminal;
  if (page === "previews") return av.previews;
  return true;
}

export interface AvailableSection {
  id: SectionId;
  label: string;
  pages: { id: AgentPageId; label: string }[];
}

/** The sections that have at least one available page, in order, with their available pages. */
export function availableSections(av: PageAvailability): AvailableSection[] {
  return SECTIONS.map((s) => ({
    id: s.id,
    label: s.label,
    pages: s.pages.filter((p) => isPageAvailable(p, av)).map((id) => ({ id, label: PAGE_LABEL[id] })),
  })).filter((s) => s.pages.length > 0);
}

/** A hosted agent opens on Chat, a BYO agent on Instructions. */
export const defaultPage = (av: PageAvailability): AgentPageId => (av.byo ? "instructions" : "chat");

/** The page to show for a requested one: itself when available, otherwise the default. */
export function resolvePage(requested: string | null | undefined, av: PageAvailability): AgentPageId {
  const id = AGENT_PAGE_IDS.find((p) => p === requested);
  return id && isPageAvailable(id, av) ? id : defaultPage(av);
}
