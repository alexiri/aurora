import type { AdminTopic, Level, OrgTopic, WorkspaceTopic } from "@/lib/routes";

/** Who is looking: what decides which topics a level lists (C-961, `14` "Who sees what"). */
export interface ViewerRoles {
  instanceAdmin: boolean;
  /** The viewer's role in the organization, when the level has one. */
  org?: "owner" | "admin" | "member";
  /** The viewer's role in the workspace, when the level has one. */
  workspace?: "owner" | "approver" | "member";
  /** The workspace is a personal workspace (`personal_of` set): its owner starts at Use (D-406). */
  personalWorkspace?: boolean;
}

/**
 * (P5-REFINE-3, D-406) How much depth a control has. Use: everyone who may use the thing. Set up: the default for
 * owners and admins, and for anyone who turns on "Show more controls". Expert: only with that switch on (and instance
 * admins inside Administration, where it is on by default). Altitude hides navigation and depth, never authorization.
 */
export type Altitude = "use" | "setup" | "expert";

const RANK: Record<Altitude, number> = { use: 0, setup: 1, expert: 2 };

/**
 * The altitude a person starts at, from their role (`14` §Altitudes): instance admin, organization owner or admin and
 * the owner of a team workspace → `setup` (an instance admin inside Administration → `expert`); the owner of a personal
 * workspace, approver, member and organization member → `use`. One line decides the personal-workspace case.
 */
export function viewerFloor(roles: ViewerRoles, inAdmin = false): Altitude {
  if (roles.instanceAdmin) return inAdmin ? "expert" : "setup";
  if (roles.org === "owner" || roles.org === "admin") return "setup";
  if (roles.workspace === "owner" && !roles.personalWorkspace) return "setup";
  return "use";
}

/** Is a control of altitude `item` shown at this floor, with the person's switch? Role caps are the caller's (`allowed`). */
export function altitudeAllows(item: Altitude, floor: Altitude, more: boolean): boolean {
  return more || RANK[item] <= RANK[floor];
}

/** The altitude a topic list is filtered by; leave it out to list every topic the role may see. */
export interface AltitudeView {
  floor: Altitude;
  more: boolean;
}

// (P5-REFINE-18 #1) In the rail's order: Overview · policy for everyone · this installation.
const ADMIN_TOPICS: AdminTopic[] = ["overview", "internet-access", "agents", "apps", "chat-platforms", "people", "data", "platform", "ai-models", "set-in-helm", "settings"];
// (P5-REFINE-17, rec 8) The Manage <org> sub-nav: Overview first (the organization's own address), then the topics.
const ORG_TOPICS: OrgTopic[] = ["overview", "agents", "people", "apps", "chat-platforms", "ai-models", "internet-access", "data", "settings"];
const ORG_MEMBER_TOPICS: OrgTopic[] = ["overview", "ai-models", "internet-access"];

export function visibleTopics(level: "admin", roles: ViewerRoles, alt?: AltitudeView): AdminTopic[];
export function visibleTopics(level: "org", roles: ViewerRoles, alt?: AltitudeView): OrgTopic[];
export function visibleTopics(level: "workspace", roles: ViewerRoles, alt?: AltitudeView): WorkspaceTopic[];
export function visibleTopics(level: Level, roles: ViewerRoles, alt?: AltitudeView): string[] {
  // Administration lists every topic to an instance admin: their floor there is Expert.
  if (level === "admin") return roles.instanceAdmin ? ADMIN_TOPICS : [];
  const setup = !alt || altitudeAllows("setup", alt.floor, alt.more);
  if (level === "org") {
    // An instance admin acting in an organization sees its pages exactly as its owner would.
    const role = roles.org ?? (roles.instanceAdmin ? "owner" : undefined);
    const list = role === "owner" || role === "admin" ? ORG_TOPICS : role === "member" ? ORG_MEMBER_TOPICS : [];
    // At Use an organization member lists Overview only; AI models and Internet access need Set up.
    return setup ? list : list.filter((t) => t === "overview");
  }
  const role = roles.workspace ?? (roles.instanceAdmin ? "owner" : undefined);
  if (!role) return [];
  const canApprove = role === "owner" || role === "approver";
  const all: WorkspaceTopic[] = [
    "overview",
    "agents",
    "skills",
    "apps",
    "internet-access",
    ...(canApprove ? (["approvals"] as const) : []),
    "activity",
    ...(role === "owner" ? (["people", "settings"] as const) : []),
  ];
  // At Use: Overview, Agents and Approvals (where the role may approve).
  return setup ? all : all.filter((t) => t === "overview" || t === "agents" || t === "approvals");
}
