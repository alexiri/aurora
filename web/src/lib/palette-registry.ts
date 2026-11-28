import type { VerbId } from "@/lib/palette-verb";
import { accountRoutes, adminRoutes, orgRoutes,TOPIC_LABEL, topicHref, wsRoutes, type WorkspaceTopic } from "@/lib/routes";

/**
 * (P5-REFINE-19, D-406) What the command palette lists. Pure: the component gathers the viewer's data and passes it in,
 * so a test can say what a role sees at an altitude. **Nouns** (places: workspaces, organizations, agents, topics, an
 * agent's pages, the person's account pages) are for everyone. **Verbs** exist only with "Show more controls" on and
 * only where the viewer's role allows them; a verb never acts: it goes to the page that owns the button and opens the
 * button's own dialog (`lib/palette-verb.ts`).
 */

export type PaletteGroup = "Workspaces" | "Places" | "Agents" | "Pages" | "Agent pages" | "Account" | "Actions";
export const PALETTE_GROUP_ORDER: PaletteGroup[] = ["Actions", "Workspaces", "Places", "Agents", "Pages", "Agent pages", "Account"];

export interface PaletteItem {
  id: string;
  group: PaletteGroup;
  label: string;
  /** Second line (the organization of a workspace, the page an action opens). */
  hint?: string;
  /** Extra text the item is found by, besides its label (a workspace by its organization's name). */
  also?: string;
  href?: string;
  /** A verb: after going to `href`, open this verb's dialog (nothing for "Keyboard shortcuts"). */
  verb?: VerbId | "shortcuts";
  /** `org/ws` or `org`: the key `readRecent` orders by. */
  recentKey?: string;
}

export interface PaletteAgent {
  id: string;
  name: string;
  handle: string;
  paused: boolean;
  byo: boolean;
}

export interface PaletteContext {
  /** "Show more controls" is on: verbs are listed. */
  verbs: boolean;
  /** Where the viewer is, from the address. */
  orgSlug?: string;
  wsSlug?: string;
  /** The agent whose page is open (a handle). */
  agentHandle?: string;
  /** The organizations and the workspaces this person can enter; `also` is a second name a workspace is found by. */
  memberships: { slug: string; name: string; role?: string; workspaces: { slug: string; name: string; also?: string }[] }[];
  topics: WorkspaceTopic[];
  agents: PaletteAgent[];
  /** The open agent's pages, from `availableSections` (what the navigation lists). */
  agentPages: { id: string; label: string }[];
  /** The viewer's role in the open workspace. */
  role?: "owner" | "approver" | "member";
  instanceAdmin: boolean;
  ssh: boolean;
}

const link = (id: string, group: PaletteGroup, label: string, href: string, extra: Partial<PaletteItem> = {}): PaletteItem => ({ id, group, label, href, ...extra });

export function paletteItems(c: PaletteContext): PaletteItem[] {
  const out: PaletteItem[] = [];
  const ws = c.orgSlug && c.wsSlug ? wsRoutes(c.orgSlug, c.wsSlug) : null;

  for (const m of c.memberships) {
    for (const w of m.workspaces) {
      out.push(link(`ws:${m.slug}/${w.slug}`, "Workspaces", w.name, wsRoutes(m.slug, w.slug).home(), { hint: m.name, also: [m.name, w.also].filter(Boolean).join(" "), recentKey: `${m.slug}/${w.slug}` }));
    }
  }
  // (D-508(e), (f)) The other places, only for those who may enter them: Manage <org> for owners, admins and instance
  // admins (never a member), and Administration for instance admins.
  for (const m of c.memberships) {
    if (!c.instanceAdmin && m.role !== "owner" && m.role !== "admin") continue;
    out.push(link(`org:${m.slug}`, "Places", `Manage ${m.name}`, orgRoutes(m.slug).home(), { also: m.name, recentKey: m.slug }));
  }
  if (c.instanceAdmin) out.push(link("admin", "Places", "Administration", adminRoutes.home(), { also: "Settings for everyone on this Aurora" }));
  if (ws) {
    for (const a of c.agents) out.push(link(`agent:${a.id}`, "Agents", a.name, ws.agent(a.handle), { hint: `@${a.handle}`, also: a.handle }));
    for (const t of c.topics) {
      if (t === "agents") continue;
      out.push(link(`topic:${t}`, "Pages", TOPIC_LABEL[t], topicHref("workspace", t, c.orgSlug!, c.wsSlug!)));
    }
    out.push(link("topic:agents", "Pages", TOPIC_LABEL.agents, ws.agents()));
    if (c.agentHandle) {
      for (const p of c.agentPages) {
        out.push(link(`page:${p.id}`, "Agent pages", p.label, ws.agent(c.agentHandle, { tab: p.id === "chat" ? undefined : p.id }), { hint: c.agents.find((a) => a.handle === c.agentHandle)?.name }));
      }
    }
  }
  out.push(link("account:profile", "Account", "Your account", accountRoutes.profile));
  out.push(link("account:security", "Account", "Security", accountRoutes.security));
  out.push(link("account:notifications", "Account", "Notifications", accountRoutes.notifications));
  if (c.ssh) out.push(link("account:ssh", "Account", "SSH keys", accountRoutes.ssh));
  out.push(link("account:api-keys", "Account", "API keys", accountRoutes.apiKeys));
  out.push(link("account:channels", "Account", "Chat platforms", accountRoutes.chatPlatforms));

  if (c.verbs) out.unshift(...paletteVerbs(c));
  return out;
}

/** The verbs the viewer's role allows. An instance admin acts as an owner (the same rule the rest of the app uses). */
export function paletteVerbs(c: PaletteContext): PaletteItem[] {
  const out: PaletteItem[] = [];
  const role = c.role ?? (c.instanceAdmin ? "owner" : undefined);
  const owner = role === "owner";
  const canApprove = role === "owner" || role === "approver";
  const ws = c.orgSlug && c.wsSlug ? wsRoutes(c.orgSlug, c.wsSlug) : null;
  const verb = (id: string, label: string, href: string, v?: PaletteItem["verb"], hint?: string): void => {
    out.push({ id: `verb:${id}`, group: "Actions", label, href, verb: v, hint });
  };
  if (ws) {
    for (const a of c.agents) {
      if (!a.byo) verb(`chat:${a.id}`, `New chat with ${a.name}`, ws.agent(a.handle), "new-chat", "Opens a new conversation");
      if (owner) {
        verb(`app:${a.id}`, `Give ${a.name} an app`, ws.agent(a.handle, { tab: "access" }), undefined, "Apps & access");
        if (!a.byo) {
          verb(`schedule:${a.id}`, `New schedule for ${a.name}`, ws.agent(a.handle, { tab: "schedules" }), "new-schedule", "Automations › Schedules");
          verb(`webhook:${a.id}`, `New webhook for ${a.name}`, ws.agent(a.handle, { tab: "webhooks" }), "new-webhook", "Automations › Webhooks");
          verb(`export:${a.id}`, `Export ${a.name}`, ws.agent(a.handle, { tab: "manage" }), "export-agent", "A zip of what it knows and does");
          verb(`import:${a.id}`, `Import into ${a.name}`, ws.agent(a.handle, { tab: "manage" }), "import-agent", "From an Aurora export (.zip)");
        }
        // (D-447) Pause opens the button's confirm; Resume goes to Manage, where the button is.
        if (a.paused) verb(`resume:${a.id}`, `Resume ${a.name}`, ws.agent(a.handle, { tab: "manage" }), undefined, "Settings › Manage");
        else verb(`pause:${a.id}`, `Pause ${a.name}`, ws.agent(a.handle, { tab: "manage" }), "pause-agent", "Asks first");
      }
    }
    if (canApprove) verb("approvals", "Open Approvals", ws.approvals());
    if (owner) {
      verb("people", "Add people", ws.people({ add: 1 }));
      verb("new-agent", "New agent", ws.agents(), "new-agent");
    }
  }
  verb("shortcuts", "Keyboard shortcuts", "", "shortcuts", "Every shortcut");
  return out;
}

function words(s: string): string[] {
  return s.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

/** 0 = no match; higher is better. Case-insensitive; a word-start beats a substring; the whole-label start beats both. */
export function matchScore(item: Pick<PaletteItem, "label" | "also">, query: string): number {
  const q = query.trim().toLowerCase();
  if (!q) return 1;
  const label = item.label.toLowerCase();
  if (label.startsWith(q)) return 4;
  const qs = words(q);
  const ws = words(label);
  if (qs.length > 0 && qs.every((t) => ws.some((w) => w.startsWith(t)))) return 3;
  if (label.includes(q)) return 2;
  const also = item.also?.toLowerCase() ?? "";
  if (also && (also.includes(q) || (qs.length > 0 && qs.every((t) => words(also).some((w) => w.startsWith(t)))))) return 1;
  return 0;
}

/**
 * The items that match, best first; ties and an empty query go by recency (`recent` is `readRecent`'s newest-first list),
 * then by the registry's own order (verbs, workspaces, …). Stable.
 */
export function matchItems(items: PaletteItem[], query: string, recent: string[]): PaletteItem[] {
  const rank = (i: PaletteItem) => {
    const at = i.recentKey ? recent.findIndex((k) => k === i.recentKey || k.startsWith(`${i.recentKey}/`)) : -1;
    return at < 0 ? Number.MAX_SAFE_INTEGER : at;
  };
  const scored = items.map((item, order) => ({ item, order, score: matchScore(item, query) })).filter((x) => x.score > 0);
  scored.sort((a, b) => {
    // Group by group (so a heading is never repeated), then the best match, then the most recently used, then the registry's order.
    const g = PALETTE_GROUP_ORDER.indexOf(a.item.group) - PALETTE_GROUP_ORDER.indexOf(b.item.group);
    return g || b.score - a.score || rank(a.item) - rank(b.item) || a.order - b.order;
  });
  return scored.map((x) => x.item);
}
