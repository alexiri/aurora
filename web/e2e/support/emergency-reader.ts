import type { Page, Route } from "@playwright/test";

import { agent, id, mockApi, now, ws } from "./mock-api";

// What the API answers an organization owner who holds an active emergency grant (FIX-236, FIX-277): the workspace in
// its real shape, and a refusal for every workspace or agent route the grant doesn't reach. The two lists are the
// server's own, from `services/aurora-api/tests/it/route_inventory.rs`: READAUTH (own access or a grant, GET only) and
// the GETs of ADMINISTER (an organization owner also administers).
const READAUTH = [
  "/workspaces/{}/access",
  "/workspaces/{}/activity",
  "/workspaces/{}/agents",
  "/workspaces/{}/messages/search",
  "/workspaces/{}/skills",
  "/agents/{}",
  "/agents/{}/conversation-folders",
  "/agents/{}/conversations",
  "/agents/{}/memories",
  "/agents/{}/memories/{}",
  "/agents/{}/schedules",
  "/agents/{}/webhooks",
  "/agents/{}/skills",
  "/agents/{}/timeline",
  "/agents/{}/timeline/stream",
  "/schedules/{}",
  "/schedules/{}/runs",
  "/webhooks/{}",
  "/webhooks/{}/deliveries",
  "/skills/{}",
  "/conversations/{}",
  "/conversations/{}/events",
  "/conversations/{}/stream",
  "/conversations/{}/export",
  "/attachments/{}/content",
];
const ADMINISTERED = [
  "/workspaces/{}",
  "/workspaces/{}/access/pending",
  "/workspaces/{}/members/{}/offboarding",
  "/workspaces/{}/usage",
  "/workspaces/{}/agent-tokens",
  "/workspaces/{}/emergency-access",
];
/** Routes under these are the workspace's own: nobody reads them without access to it. */
const SCOPED = /^\/(workspaces|agents|conversations|schedules|webhooks|skills|attachments)\/[^/]+/;
const matches = (list: string[], path: string) => list.some((t) => new RegExp(`^${t.replaceAll("{}", "[^/]+")}$`).test(path));

/** Whether the server would refuse this request of an emergency reader. */
export function refusedForReader(method: string, url: URL): boolean {
  const path = url.pathname.replace(/^\/v1/, "");
  // A workspace named in the query (the accounts list and its like) needs own access; the counts answer `null` instead.
  if (path !== "/attention/counts" && ["workspace", "workspace_id"].some((k) => url.searchParams.get(k) === ws.id)) return true;
  if (!SCOPED.test(path)) return false;
  if (method !== "GET") return true;
  return !matches(READAUTH, path) && !matches(ADMINISTERED, path);
}

export const readerGrant = () => ({
  id: id(500),
  by: { id: id(99), display_name: "Ada Lovelace" },
  reason: "Legal hold on the account's records",
  opened_at: now,
  expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  ended_at: null,
  active: true,
});

export const readerWorkspace = () => {
  const emergency = readerGrant();
  return {
    ...ws,
    my_role: null,
    administers: true,
    emergency_reader: true,
    emergency_reader_id: emergency.id,
    emergency_reader_expires_at: emergency.expires_at,
    audience: { people: 2, owners: [{ id: id(99), display_name: "Ada Lovelace" }], owners_total: 1, emergency },
  };
};

const json = (route: Route, status: number, body: unknown) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

/**
 * The mocked API as the real one answers the reader. Call it after any other mock, so its refusals come first. Returns
 * the refused requests: a page that is right for a reader leaves the list empty.
 */
export async function mockEmergencyReader(page: Page, before: () => Promise<void> = async () => {}) {
  const refused: string[] = [];
  await mockApi(page);
  await before();
  const workspace = readerWorkspace();
  await page.route("**/v1/**", (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (refusedForReader(request.method(), url)) {
      refused.push(`${request.method()} ${url.pathname}${url.search}`);
      return json(route, 403, { error: { code: "workspace_access_required", message: "You manage this workspace but you are not one of its people." } });
    }
    if (url.pathname === `/v1/workspaces/${ws.id}` && request.method() === "GET") return json(route, 200, workspace);
    if (url.pathname === `/v1/workspaces/${ws.id}/agents`) return json(route, 200, { items: [{ ...agent, needs_update: true }], next_cursor: null });
    return route.fallback();
  });
  return refused;
}
