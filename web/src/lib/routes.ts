/**
 * Every internal dashboard path comes from here (C-960, P5-IA-1): one function per topic and tab at each level.
 * A test (`tests/routes.test.ts`) fails on a hard-coded old path anywhere else in `web/src`. The pages' old paths
 * live in `legacy-routes.ts`, which turns them into these.
 */

export type Level = "admin" | "org" | "workspace";
export type Query = Record<string, string | number | boolean | null | undefined>;

/** `path` + `?query` + `#hash`; empty, null and undefined query values are left out. */
export function url(path: string, query?: Query, hash?: string): string {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(query ?? {})) {
    if (v !== undefined && v !== null && v !== "") qs.set(k, String(v));
  }
  const q = qs.toString();
  return `${path}${q ? `?${q}` : ""}${hash ? `#${hash.replace(/^#/, "")}` : ""}`;
}

const seg = encodeURIComponent;

/** The tab ids of each topic page (`14 §Topics per level`). Order is the order of the tab bar. */
export const TABS = {
  // (P5-REFINE-18 #1) What agents may do: Internet access · AI keys and rules · Bring your own · Defaults · Agent templates (C-1114).
  adminAgentPolicy: ["internet-access", "ai-keys", "byo", "defaults", "templates"],
  adminApps: ["apps", "oauth-clients", "self-hosted", "advanced"],
  adminPeople: ["users", "organizations", "sign-in", "new-users"],
  adminData: ["retention", "storage", "audit", "usage"],
  // Health & runners: Health · Runners · Computers, then the installation's own settings pages.
  adminPlatform: ["health", "runners", "computers", "secret-stores", "email", "limits"],
  // (P5-REFINE-17, org-6) Agents · Bring your own · Skills · Templates (C-1115); Previews is a second section of Bring your own.
  orgAgents: ["agents", "byo", "skills", "templates"],
  orgApps: ["accounts", "apps", "oauth-clients", "self-hosted"],
  orgPeople: ["members", "groups", "directory-sync"],
  orgData: ["retention", "storage", "audit", "usage"],
  orgSettings: ["general", "api-keys", "danger"],
  wsActivity: ["requests", "usage"],
  wsApps: ["accounts", "apps"],
  wsSettings: ["general", "api-keys", "audit", "danger"],
} as const;

type TabOf<K extends keyof typeof TABS> = (typeof TABS)[K][number];

/** Administration (`/admin/…`). */
export const adminRoutes = {
  home: () => "/admin",
  /** (P5-REFINE-18 #1) What agents may do (`/admin/internet-access`), one page with four tabs. */
  agentPolicy: (tab?: TabOf<"adminAgentPolicy">, hash?: string) => url("/admin/internet-access", { tab }, hash),
  apps: (tab?: TabOf<"adminApps">, query?: Query, hash?: string) => url("/admin/apps", { tab, ...query }, hash),
  oauthClients: (query?: Query) => url("/admin/apps", { tab: "oauth-clients", ...query }),
  selfHosted: (query?: Query) => url("/admin/apps", { tab: "self-hosted", ...query }),
  /** The Advanced tab: `package-sources`, `publishers` or `host-conflicts`, or one package source by its id. */
  advanced: (anchor?: string) => url("/admin/apps", { tab: "advanced" }, anchor),
  package: (id: string) => `/admin/apps/packages/${seg(id)}`,
  /** AI endpoints: the endpoints only; the AI key rules moved to What agents may do › AI keys and rules. */
  aiModels: () => "/admin/ai-models",
  internetAccess: () => "/admin/internet-access",
  /** (P5-SKLIB-3, D-450(b)) The instance's skill libraries, a sub-page of What agents may do › Defaults. */
  skillLibraries: () => "/admin/skill-libraries",
  /** (C-1114) The review of one template version, a sub-page of What agents may do › Agent templates. */
  templateReview: (templateId: string, version: string) => `/admin/internet-access/templates/${seg(templateId)}/${seg(version)}`,
  people: (tab?: TabOf<"adminPeople">, hash?: string) => url("/admin/people", { tab }, hash),
  organizations: (hash?: string) => url("/admin/people", { tab: "organizations" }, hash),
  organization: (id: string) => `/admin/people/organizations/${seg(id)}`,
  signIn: (hash?: string) => url("/admin/people", { tab: "sign-in" }, hash),
  chatPlatforms: () => "/admin/chat-platforms",
  data: (tab?: TabOf<"adminData">) => url("/admin/data", { tab }),
  platform: (tab?: TabOf<"adminPlatform">, hash?: string) => url("/admin/platform", { tab }, hash),
  /** All settings (the index of every section). */
  settings: (hash?: string) => url("/admin/settings", undefined, hash),
  /** (P5-REFINE-18 #1) Every value the Helm values set; `key` jumps to one row. */
  setInHelm: (key?: string) => url("/admin/set-in-helm", undefined, key),
  /** The page of one `governance` key: AI key rules, egress and BYO/agent tokens each live in their own topic. */
  governanceSetting: (key: string) => {
    if (key.startsWith("governance.egress.")) return url("/admin/internet-access", { tab: "internet-access" }, key);
    if (key.startsWith("byo.") || key.startsWith("governance.agent_tokens.")) return url("/admin/internet-access", { tab: "byo" }, key);
    return url("/admin/internet-access", { tab: "ai-keys" }, key);
  },
  /** The topic page and tab that holds a settings section (C-971); All settings when the section has no topic yet. */
  sectionPage: (section: string, hash?: string) => {
    const target = SETTINGS_SECTION_PAGE[section.replace(/-/g, "_")];
    return target ? url(target.path, { tab: target.tab }, hash) : url("/admin/settings", undefined, hash);
  },
};

/** Settings sections → the topic page that renders them (`14` "Settings live in their topics"). */
const SETTINGS_SECTION_PAGE: Record<string, { path: string; tab?: string }> = {
  governance: { path: "/admin/internet-access", tab: "ai-keys" },
  llm: { path: "/admin/ai-models" },
  sign_in: { path: "/admin/people", tab: "sign-in" },
  scim: { path: "/admin/people", tab: "sign-in" },
  email: { path: "/admin/platform", tab: "email" },
  secrets: { path: "/admin/platform", tab: "secret-stores" },
  agents: { path: "/admin/internet-access", tab: "defaults" },
  approvals: { path: "/admin/internet-access", tab: "defaults" },
  terminal: { path: "/admin/platform", tab: "computers" },
  previews: { path: "/admin/platform", tab: "computers" },
  skills: { path: "/admin/internet-access", tab: "defaults" },
  retention: { path: "/admin/data", tab: "retention" },
  limits: { path: "/admin/platform", tab: "limits" },
  outgoing_webhooks: { path: "/admin/apps", tab: "advanced" },
};

/** Where else a section's keys are shown (`governance` splits three ways): the topic and the page of each part. */
const SETTINGS_SECTION_ALSO: Record<string, { topic: string; label: string; href: string }[]> = {
  governance: [
    { topic: "What agents may do", label: "Internet access", href: "/admin/internet-access?tab=internet-access" },
    { topic: "What agents may do", label: "Bring your own", href: "/admin/internet-access?tab=byo" },
  ],
};

/** The topic a settings section is grouped under in All settings, with its topic tab (C-971). */
export function sectionTopic(section: string): { topic: string; label: string } {
  const key = section.replace(/-/g, "_");
  const path = SETTINGS_SECTION_PAGE[key]?.path ?? "";
  const topic = Object.entries(ADMIN_RAIL_LABEL).find(([id]) => path === `/admin/${id}`);
  return { topic: topic?.[1] ?? "Other", label: topic?.[1] ?? "Other" };
}

/** Every page that shows part of a section: the main one first (All settings lists them all). */
export function sectionHomes(section: string): { topic: string; label: string; href: string }[] {
  const main = sectionTopic(section);
  return [{ topic: main.topic, label: main.label, href: adminRoutes.sectionPage(section) }, ...(SETTINGS_SECTION_ALSO[section.replace(/-/g, "_")] ?? [])];
}

/** An organization (`/o/<org>/…`). */
export function orgRoutes(org: string) {
  const base = `/o/${seg(org)}`;
  return {
    base,
    home: () => base,
    agents: (tab?: TabOf<"orgAgents">) => url(`${base}/agents`, { tab }),
    /** The manage page for a workspace an administrator is not in (FIX-235). */
    workspace: (ws: string) => `${base}/workspaces/${seg(ws)}`,
    skillLibraries: () => `${base}/agents/skills/libraries`,
    apps: (tab?: TabOf<"orgApps">, query?: Query, hash?: string) => url(`${base}/apps`, { tab, ...query }, hash),
    /** The organization's accounts; `kind` filters the list (`ai-key`, `app`, `website`). */
    accounts: (query?: Query, hash?: string) => url(`${base}/apps`, { tab: "accounts", ...query }, hash),
    /** The per-app setup page (P5-UX-9). */
    app: (app: string, query?: Query) => url(`${base}/apps/${seg(app)}`, query),
    aiModels: () => `${base}/ai-models`,
    internetAccess: () => `${base}/internet-access`,
    people: (tab?: TabOf<"orgPeople">, query?: Query) => url(`${base}/people`, { tab, ...query }),
    group: (id: string) => `${base}/people/groups/${seg(id)}`,
    chatPlatforms: () => `${base}/chat-platforms`,
    data: (tab?: TabOf<"orgData">) => url(`${base}/data`, { tab }),
    settings: (tab?: TabOf<"orgSettings">) => url(`${base}/settings`, { tab }),
  };
}

/** A workspace (`/o/<org>/w/<ws>/…`). */
export function wsRoutes(org: string, ws: string) {
  const base = `/o/${seg(org)}/w/${seg(ws)}`;
  return {
    base,
    home: () => base,
    welcome: () => `${base}/welcome`,
    agents: () => `${base}/agents`,
    /** An agent's page (an id or a handle), optionally on a tab. */
    agent: (handle: string, query?: Query) => url(`${base}/agents/${seg(handle)}`, query),
    /** (C-1116) The four-step page that creates an agent from a template. */
    newAgent: (templateId: string) => `${base}/agents/new/${seg(templateId)}`,
    terminal: (handle: string) => `${base}/agents/${seg(handle)}/terminal`,
    skills: () => `${base}/skills`,
    apps: (tab?: TabOf<"wsApps">, query?: Query, hash?: string) => url(`${base}/apps`, { tab, ...query }, hash),
    accounts: (query?: Query, hash?: string) => url(`${base}/apps`, { tab: "accounts", ...query }, hash),
    /** The Connect dialog (`connect=<app>`, on Apps) or the Reconnect dialog (`reconnect=<id>`, on Accounts) open (C-842). */
    connect: (app: string, reconnect?: string | null) =>
      url(`${base}/apps`, reconnect ? { tab: "accounts", reconnect } : { tab: "apps", connect: app }),
    app: (app: string, query?: Query) => url(`${base}/apps/${seg(app)}`, query),
    internetAccess: () => `${base}/internet-access`,
    approvals: (query?: Query) => url(`${base}/approvals`, query),
    activity: (query?: Query) => url(`${base}/activity`, query),
    /** (P5-USAGE-2, C-1004) Activity › Usage, optionally filtered to an agent (`agent=<id>`). */
    usage: (agent?: string) => url(`${base}/activity`, { tab: "usage", agent }),
    people: (query?: Query) => url(`${base}/people`, query),
    settings: (tab?: TabOf<"wsSettings">) => url(`${base}/settings`, { tab }),
  };
}

export const accountRoutes = {
  profile: "/account",
  security: "/account/security",
  apiKeys: "/account/api-keys",
  chatPlatforms: "/account/channels",
  /** Approval notifications (account-4): the browser opt-in, the e-mail switch and push devices. */
  notifications: "/account/notifications",
  /** SSH keys (P4-SSH-1): a rail entry on Account while `features.ssh` is on. */
  ssh: "/account/ssh",
  delete: "/account/delete",
} as const;

/** Topic ids per level, in the order of `14 §Topics per level`. */
export type AdminTopic =
  | "overview"
  | "apps"
  | "ai-models"
  | "internet-access"
  | "people"
  | "chat-platforms"
  | "data"
  | "platform"
  | "set-in-helm"
  | "settings";
export type OrgTopic = "overview" | "agents" | "apps" | "ai-models" | "internet-access" | "people" | "chat-platforms" | "data" | "settings";
export type WorkspaceTopic = "overview" | "agents" | "skills" | "apps" | "internet-access" | "approvals" | "activity" | "people" | "settings";

/** The name of each topic in the interface (the vocabulary of `14`). */
export const TOPIC_LABEL: Record<AdminTopic | OrgTopic | WorkspaceTopic, string> = {
  overview: "Overview",
  agents: "Agents",
  skills: "Skills",
  apps: "Apps & accounts",
  "ai-models": "AI models",
  "internet-access": "Internet access",
  approvals: "Approvals",
  activity: "Activity",
  people: "People",
  "chat-platforms": "Chat platforms",
  data: "Data",
  platform: "Platform",
  "set-in-helm": "Set in Helm",
  settings: "Settings",
};

/**
 * (P5-REFINE-18 #1, rec 9) Administration's rail in two halves: what an administrator decides for everyone, and what
 * belongs to this installation. Labels here are the rail's; the organization and workspace levels keep `TOPIC_LABEL`.
 * Route ids do not change.
 */
export const ADMIN_RAIL_LABEL: Record<AdminTopic, string> = {
  overview: "Overview",
  "internet-access": "What agents may do",
  apps: "Apps",
  "chat-platforms": "Chat platforms",
  people: "People & sign-in",
  data: "Data",
  platform: "Health & runners",
  "ai-models": "AI endpoints",
  "set-in-helm": "Set in Helm",
  settings: "All settings",
};

export const ADMIN_RAIL_GROUPS: { label: string | null; topics: AdminTopic[] }[] = [
  { label: null, topics: ["overview"] },
  { label: "Policy for everyone", topics: ["internet-access", "apps", "chat-platforms", "people", "data"] },
  { label: "This installation", topics: ["platform", "ai-models", "set-in-helm", "settings"] },
];

/** Where a topic's page is, per level. */
export function topicHref(level: "admin", topic: AdminTopic): string;
export function topicHref(level: "org", topic: OrgTopic, org: string): string;
export function topicHref(level: "workspace", topic: WorkspaceTopic, org: string, ws: string): string;
export function topicHref(level: Level, topic: string, org?: string, ws?: string): string {
  const path = topic === "overview" || topic === "workspaces" ? "" : `/${topic}`;
  if (level === "admin") return `/admin${path}`;
  // `/o/<org>/apps` without a tab is the old catalogue address (C-960 redirects it), so the topic link names its tab.
  if (level === "org") return topic === "apps" ? orgRoutes(org ?? "").accounts() : `/o/${seg(org ?? "")}${path}`;
  return `/o/${seg(org ?? "")}/w/${seg(ws ?? "")}${path}`;
}
