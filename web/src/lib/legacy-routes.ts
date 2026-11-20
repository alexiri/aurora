/**
 * Old route → new route (C-960, P5-IA-1). One pure function turns any pre-phase-5 dashboard path (with its query
 * and anchor) into the topic page and tab that replaced it; `null` means the path is not an old route.
 *
 * The same table serves three callers: `next.config.ts` (the `STATIC_REDIRECTS` below, answered before any render),
 * `LegacyRedirect` (a client redirect for the `?tab=` maps, on the pages that keep an old path) and the table test.
 * This file and `routes.ts` are the only places that spell old paths.
 */

// Relative: `next.config.ts` loads this file without the `@/` alias.
import { adminRoutes, orgRoutes, wsRoutes, type Query } from "./routes";

/** Query parameters the new pages accept on top of the tab; they ride along on every redirect. */
function carry(search: URLSearchParams, drop: string[] = ["tab"]): Query {
  const q: Query = {};
  search.forEach((v, k) => {
    if (!drop.includes(k)) q[k] = v;
  });
  return q;
}

/** Splits `/a/b?x=1#h` into its parts so a rule can add to the query and keep the anchor. */
function join(target: string, search: URLSearchParams, hash: string, drop?: string[]): string {
  const [pathAndQuery = "", ownHash] = target.split("#");
  const [path = "", ownQuery] = pathAndQuery.split("?");
  const params = new URLSearchParams(ownQuery ?? "");
  for (const [k, v] of Object.entries(carry(search, drop))) if (!params.has(k)) params.set(k, String(v));
  const qs = params.toString();
  const anchor = ownHash ?? hash.replace(/^#/, "");
  return `${path}${qs ? `?${qs}` : ""}${anchor ? `#${anchor}` : ""}`;
}

type Rule = (m: RegExpMatchArray, search: URLSearchParams) => string | null;
const ADMIN: [RegExp, Rule][] = [
  [/^\/admin\/setup$/, () => adminRoutes.home()],
  [/^\/admin\/users$/, () => adminRoutes.people("users")],
  [/^\/admin\/orgs$/, () => adminRoutes.people("organizations")],
  [/^\/admin\/orgs\/([^/]+)$/, (m) => adminRoutes.organization(decodeURIComponent(m[1]!))],
  [/^\/admin\/identity$/, () => adminRoutes.people("sign-in")],
  [/^\/admin\/placement$/, () => adminRoutes.people("new-users")],
  [/^\/admin\/plugins$/, () => adminRoutes.apps("apps")],
  [/^\/admin\/plugins\/([^/]+)$/, (m) => adminRoutes.package(decodeURIComponent(m[1]!))],
  [/^\/admin\/addon-sources$/, () => adminRoutes.advanced("package-sources")],
  [/^\/admin\/trusted-keys$/, () => adminRoutes.advanced("publishers")],
  [/^\/admin\/host-claims$/, () => adminRoutes.advanced("host-conflicts")],
  [/^\/admin\/instances$/, () => adminRoutes.selfHosted()],
  [/^\/admin\/oauth-clients$/, () => adminRoutes.oauthClients()],
  [/^\/admin\/connectors$/, () => adminRoutes.chatPlatforms()],
  [/^\/admin\/secret-stores$/, () => adminRoutes.platform("secret-stores")],
  [/^\/admin\/runners$/, () => adminRoutes.platform("runners")],
  [/^\/admin\/storage$/, () => adminRoutes.data("storage")],
  [/^\/admin\/audit$/, () => adminRoutes.data("audit")],
  // `/admin/settings/<section>` → the topic tab that now holds the section (C-971); an unknown section → All settings.
  [/^\/admin\/settings\/([^/]+)$/, (m) => adminRoutes.sectionPage(decodeURIComponent(m[1]!))],
];

/** The organization's Credentials page: the tab decides where the list now lives. */
function orgCredentials(o: ReturnType<typeof orgRoutes>, tab: string | null): string {
  if (tab === "llm") return o.accounts({ kind: "ai-key" });
  if (tab === "oauth") return o.apps("oauth-clients");
  return o.accounts();
}

function wsCredentials(w: ReturnType<typeof wsRoutes>, tab: string | null): string {
  if (tab === "llm") return w.accounts({ kind: "ai-key" });
  if (tab === "apps" || tab === "oauth") return w.apps("apps");
  if (tab === "proposals") return w.accounts(undefined, "requests");
  return w.accounts();
}

/** The new page for an old `pathname` + `search` + `hash`, or `null` when `pathname` is not an old route. */
export function legacyTarget(pathname: string, searchInput: string | URLSearchParams = "", hash = ""): string | null {
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  const search = typeof searchInput === "string" ? new URLSearchParams(searchInput) : searchInput;
  const tab = search.get("tab");

  for (const [re, rule] of ADMIN) {
    const m = path.match(re);
    if (m) {
      const to = rule(m, search);
      return to ? join(to, search, hash, ["tab"]) : null;
    }
  }

  const ws = path.match(/^\/o\/([^/]+)\/w\/([^/]+)\/(credentials|policy|settings)$/);
  if (ws) {
    const w = wsRoutes(decodeURIComponent(ws[1]!), decodeURIComponent(ws[2]!));
    if (ws[3] === "credentials") return join(wsCredentials(w, tab), search, hash);
    if (ws[3] === "policy") return join(w.internetAccess(), search, hash);
    // `…/settings?tab=access` is the only old settings address; every other tab keeps its page.
    return tab === "access" ? join(w.people(), search, hash) : null;
  }
  const connect = path.match(/^\/o\/([^/]+)\/w\/([^/]+)\/apps\/([^/]+)\/connect$/);
  if (connect) {
    const w = wsRoutes(decodeURIComponent(connect[1]!), decodeURIComponent(connect[2]!));
    return join(w.connect(decodeURIComponent(connect[3]!), search.get("reconnect")), search, hash, ["tab", "connect", "reconnect"]);
  }

  const org = path.match(/^\/o\/([^/]+)\/(.+)$/);
  if (!org || org[2]!.startsWith("w/")) return null;
  const o = orgRoutes(decodeURIComponent(org[1]!));
  const rest = org[2]!;
  const group = rest.match(/^groups\/([^/]+)$/);
  if (group) return join(o.group(decodeURIComponent(group[1]!)), search, hash);
  switch (rest) {
    case "members":
      return join(o.people("members"), search, hash);
    case "groups":
      return join(o.people("groups"), search, hash);
    case "settings/directory":
      return join(o.people("directory-sync"), search, hash);
    case "credentials":
      return join(orgCredentials(o, tab), search, hash);
    case "apps":
      // The catalogue was this page; the new page opens on Accounts, so an address without a tab is the old one.
      return tab ? null : join(o.apps("apps"), search, hash);
    case "governance":
      if (tab === "egress") return join(o.internetAccess(), search, hash);
      if (tab === "apps") return join(o.apps("apps"), search, hash);
      if (tab === "byo") return join(o.agents("byo"), search, hash);
      return join(o.aiModels(), search, hash);
    case "policy":
      return join(o.internetAccess(), search, hash);
    case "skills":
      return join(o.agents("skills"), search, hash);
    case "skills/libraries":
      return join(o.skillLibraries(), search, hash);
    case "settings/previews":
      return join(o.agents("byo"), search, hash);
    case "channels":
      return join(o.chatPlatforms(), search, hash);
    case "instances":
      return join(o.apps("self-hosted"), search, hash);
    case "api-keys":
      return join(o.settings("api-keys"), search, hash);
    case "audit":
      return join(o.data("audit"), search, hash);
    case "storage":
      return join(o.data("storage"), search, hash);
    default:
      return null;
  }
}

/**
 * The redirects `next.config.ts` answers before rendering anything: old paths whose destination does not depend on
 * `?tab=`. Next appends the request's own query string to the destination.
 */
export const STATIC_REDIRECTS: { source: string; destination: string }[] = [
  { source: "/admin/setup", destination: "/admin" },
  { source: "/admin/users", destination: "/admin/people?tab=users" },
  { source: "/admin/orgs", destination: "/admin/people?tab=organizations" },
  { source: "/admin/orgs/:id", destination: "/admin/people/organizations/:id" },
  { source: "/admin/identity", destination: "/admin/people?tab=sign-in" },
  { source: "/admin/placement", destination: "/admin/people?tab=new-users" },
  { source: "/admin/plugins", destination: "/admin/apps?tab=apps" },
  { source: "/admin/plugins/:id", destination: "/admin/apps/packages/:id" },
  { source: "/admin/addon-sources", destination: "/admin/apps?tab=advanced#package-sources" },
  { source: "/admin/trusted-keys", destination: "/admin/apps?tab=advanced#publishers" },
  { source: "/admin/host-claims", destination: "/admin/apps?tab=advanced#host-conflicts" },
  { source: "/admin/instances", destination: "/admin/apps?tab=self-hosted" },
  { source: "/admin/oauth-clients", destination: "/admin/apps?tab=oauth-clients" },
  { source: "/admin/connectors", destination: "/admin/chat-platforms" },
  { source: "/admin/secret-stores", destination: "/admin/platform?tab=secret-stores" },
  { source: "/admin/runners", destination: "/admin/platform?tab=runners" },
  { source: "/admin/storage", destination: "/admin/data?tab=storage" },
  { source: "/admin/audit", destination: "/admin/data?tab=audit" },
  { source: "/o/:org/members", destination: "/o/:org/people?tab=members" },
  { source: "/o/:org/groups", destination: "/o/:org/people?tab=groups" },
  { source: "/o/:org/groups/:id", destination: "/o/:org/people/groups/:id" },
  { source: "/o/:org/settings/directory", destination: "/o/:org/people?tab=directory-sync" },
  { source: "/o/:org/policy", destination: "/o/:org/internet-access" },
  { source: "/o/:org/skills", destination: "/o/:org/agents?tab=skills" },
  { source: "/o/:org/skills/libraries", destination: "/o/:org/agents/skills/libraries" },
  { source: "/o/:org/settings/previews", destination: "/o/:org/agents?tab=byo" },
  { source: "/o/:org/channels", destination: "/o/:org/chat-platforms" },
  { source: "/o/:org/instances", destination: "/o/:org/apps?tab=self-hosted" },
  { source: "/o/:org/api-keys", destination: "/o/:org/settings?tab=api-keys" },
  { source: "/o/:org/audit", destination: "/o/:org/data?tab=audit" },
  { source: "/o/:org/storage", destination: "/o/:org/data?tab=storage" },
  { source: "/o/:org/w/:ws/policy", destination: "/o/:org/w/:ws/internet-access" },
];
