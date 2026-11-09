import { expect, type Page } from "@playwright/test";

import { agent, id, mockApi, now, ws } from "./mock-api";
import type { Scheme } from "./open";

// FIX-022: the agent Access tab with the connection grant editor. The data uses the real built-in packages
// (plugins/builtin/github, atlassian, indico) and their tool ids/names; the API is the shared mock plus these routes.

export const ACCESS_SCREENS = ["connections", "editor-custom", "editor-custom-only"] as const;
export type AccessScreen = (typeof ACCESS_SCREENS)[number];

const tool = (tid: string, name: string, group: "read" | "write", path: string, methods: string[], host = "api.github.com") => ({
  id: tid, name, group, host, path, methods, wildcard: false,
});
const githubTools = [
  tool("get_repo", "Get a repository", "read", "/repos/*/*", ["GET"]),
  tool("list_issues", "List issues", "read", "/repos/*/*/issues", ["GET"]),
  tool("get_issue", "Get an issue", "read", "/repos/*/*/issues/*", ["GET"]),
  tool("list_pulls", "List pull requests", "read", "/repos/*/*/pulls", ["GET"]),
  tool("search_code", "Search code", "read", "/search/code", ["GET"]),
  tool("graphql_query", "GraphQL query", "read", "/graphql", ["POST"]),
  tool("create_issue", "Create an issue", "write", "/repos/*/*/issues", ["POST"]),
  tool("create_issue_comment", "Comment on an issue or pull request", "write", "/repos/*/*/issues/*/comments", ["POST"]),
  tool("merge_pull", "Merge a pull request", "write", "/repos/*/*/pulls/*/merge", ["PUT"]),
  tool("git_push", "Push code over HTTPS", "write", "/*/*/**", ["GET", "POST"], "github.com"),
];
const indicoTools = [
  tool("get_current_user", "Get the connected Indico user", "read", "/api/user/", ["GET"], "indico.cern.ch"),
  tool("get_event", "Read an event (contributions, sessions, attachments, notes)", "read", "/export/event/*", ["GET"], "indico.cern.ch"),
  tool("get_timetable", "Read an event timetable", "read", "/export/timetable/*", ["GET"], "indico.cern.ch"),
];

const C_GH = id(40);
const C_JIRA = id(41);
const C_IND = id(42);
const conn = (cid: string, appId: string, label: string) => ({
  app_id: appId, app_version: "1.0.0", created_at: now, id: cid, injecting: [], label, metadata: {}, missing_scopes: [], org_id: id(1),
  package_id: appId, scope: "workspace", scopes: [], status: "active", updated_at: now, workspace_id: id(2),
});
const grant = (cid: string, over: Record<string, unknown>) => ({
  id: id(900 + Number(cid.slice(-2))), agent_id: agent.id, connection_id: cid, created_at: now, mode: "custom", allow_tools: [], ask_tools: [], ask_timeout_secs: null, ...over,
});

/** Mock routes for the Access tab; later routes win over the shared mock, the rest falls through to it. */
export async function mockAccess(page: Page) {
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (p === `/v1/workspaces/${ws.id}/connections`) return json({ items: [conn(C_GH, "github", "acme-bot"), conn(C_JIRA, "atlassian", "Jira Cloud"), conn(C_IND, "indico", "CERN Indico")] });
    if (p === `/v1/orgs/${ws.org_id}/connections`) return json({ items: [] });
    if (p === `/v1/agents/${agent.id}/grants`)
      return json({
        secrets: [],
        connections: [
          grant(C_GH, { allow_tools: ["get_repo", "list_issues", "list_pulls", "create_issue", "list_labels"], ask_tools: ["merge_pull"], ask_timeout_secs: 900 }),
          grant(C_JIRA, { mode: "full" }),
          grant(C_IND, { app_restrictions: { custom_only: true, block_untooled: true }, allow_tools: ["get_event"], ask_tools: [] }),
        ],
      });
    if (p === "/v1/apps/github") return json({ app_id: "github", name: "GitHub", package_id: "github", package_version: "1.0.1", available: true, connection_count: 1, description: "", has_dark_icon: true, connect: [], credentials: [], hosts: [], metadata: [], oauth_clients: [], tools: githubTools });
    if (p === "/v1/apps/indico") return json({ app_id: "indico", name: "Indico", package_id: "indico", package_version: "1.0.0", available: true, connection_count: 1, description: "", has_dark_icon: true, connect: [], credentials: [], hosts: [], metadata: [], oauth_clients: [], tools: indicoTools });
    if (p === `/v1/agents/${agent.id}/effective`) return json({ credentials: [], tools: [] });
    if (p === `/v1/agents/${agent.id}/model-readiness`) return json({ harness: "claude-code", override_dropped: false, default: null, keys: [] });
    if (p.endsWith("/secrets")) return json({ items: [] });
    return route.fallback();
  });
}

/** Opens `?tab=access` on the mocked agent; `editor-custom` opens the GitHub grant, `editor-custom-only` the Indico one. */
export async function openAccess(page: Page, screen: AccessScreen, scheme: Scheme) {
  // `openRoute` installs the shared mock; ours goes on top before the page loads.
  await mockAccess(page);
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/o/acme/w/main/agents/daneel?tab=access");
  await page.getByRole("list", { name: "Connection grants" }).waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  if (screen !== "connections") {
    await page.getByRole("button", { name: screen === "editor-custom" ? "Edit access to acme-bot" : "Edit access to CERN Indico" }).click();
    await page.getByRole("dialog").getByRole("radiogroup", { name: /^Access for / }).first().waitFor();
  }
  expect(errors, "the Access tab threw").toEqual([]);
}
