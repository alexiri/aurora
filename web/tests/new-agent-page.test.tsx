import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NewAgentFromTemplateView } from "@/components/templates/new-agent/page-view";
import { json, type Handler } from "./support/api";
import { wrap } from "./support/render";

// P6-TPL-4 Part B1, C-1116: **Use this template** opens the four-step page — the fold **About this template** (the
// README, closed, inert) above the steps. About asks name, handle and the template's inputs; Accounts asks each
// entry's app, its account (existing, new through the account-only journey, or skipped) and the access.

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: vi.fn(), push }),
  useParams: () => ({ org: "acme", ws: "main", template_id: "schedule-manager" }),
}));

const README =
  "# Schedule manager\n\nWhat it does.\n\n<img src=x onerror=alert(1)>\n\n[click](javascript:alert(1))\n\n![remote picture](https://evil.test/p.png)\n\n[docs](./other.md)\n";

const account = (id: string, name: string, over: Record<string, unknown> = {}) => ({
  ref: { type: "connection", id },
  kind: "app",
  name,
  scope: "workspace",
  levels: ["read", "ask", "act"],
  app: { app_id: "github", name: "GitHub", custom_only: false },
  health: { state: "not_used" },
  agents: [],
  connected_by: null,
  can: { give_access: true, change_access: true, manage: true },
  ...over,
});

const GITHUB = account("c1", "GitHub · bob-builder");
const LINEAR = { ...account("c2", "Linear · acme"), app: { app_id: "linear", name: "Linear", custom_only: false } };

const choice = (over: Record<string, unknown> = {}) => ({
  app_id: "github", app_name: "GitHub", available: true, accounts: [GITHUB], level: "ask", level_note: null,
  custom: null, unavailable_reason: null,
  ...over,
});

const plan = {
  template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work",
  readme: README, version: "1.0.0", has_avatar: false, has_helpers: false, ready: true, content_sha256: "c".repeat(64),
  apps: [{ app_ids: ["github"], required: true, available: true, skills: [] }],
  automations: [],
  content: {
    name: "Schedule manager", summary: "Triage and schedule the team's work", readme: README, effort: null, min_core: null,
    agent: { name: "Daneel", may_message_first: false }, instructions: "Triage carefully.",
    inputs: [
      { id: "repo", label: "Repository", help: "owner/name", kind: "text", required: true, pattern: null, max_length: 100, options: null, default: null },
      { id: "region", label: "Region", help: null, kind: "choice", required: true, pattern: null, max_length: null, options: ["eu", "us"], default: "eu" },
    ],
    apps: [], skills: [], skill_refs: [], helpers: [], memories: [], automations: [], has_avatar: false,
  },
  entries: [
    { index: 0, required: true, why: "It triages pull requests.", choices: [choice()] },
    { index: 1, required: false, why: "It files cards.", choices: [choice({ app_id: "linear", app_name: "Linear", accounts: [LINEAR], level: "read" })] },
  ],
  skill_refs: [],
};

const DETAIL = {
  app_id: "github", name: "GitHub", package_id: "github", package_version: "2.0.0", description: "d", available: true, connection_count: 0,
  has_dark_icon: false, category: "development", api_docs: null, connect_note: null, label_hint: null, credential_free: false,
  app_restrictions: { custom_only: false, block_untooled: false },
  connect: [
    {
      id: "token", kind: "fields", label: "Token", description: null, recommended: true, audience: "person", needs: [], ready: true,
      fields: [{ name: "access_token", label: "Token", type: "secret", store: "credentials", required: true }],
      scopes: [], default_scopes: [], access_levels: [], probe_scopes: [], client_key: null, callback: null,
      scope_presets: { read: [], write: [] }, permission_hints: [], show_context: [],
    },
    {
      id: "oauth", kind: "oauth2", label: "Sign in", description: null, recommended: false, audience: "person", needs: [], ready: true, fields: [],
      scopes: [], default_scopes: [], access_levels: [], probe_scopes: [], client_key: "gh", callback: "query",
      scope_presets: { read: [], write: [] }, permission_hints: [], show_context: [],
    },
  ],
  first_prompt: null, tools: [], hosts: [], agent_instructions: null, credentials: [], metadata: [], oauth_clients: [], instances: null,
};

function stubFetch(extra: Record<string, Handler> = {}) {
  let planGets = 0;
  const routes: Record<string, Handler> = {
    "GET /v1/resolve": () => json(200, { org: { id: "o1", name: "Acme" }, workspace: { id: "w1", name: "Main" } }),
    "GET /v1/workspaces/w1/agent-templates/schedule-manager": () => {
      planGets += 1;
      return json(200, plan);
    },
    "GET /v1/orgs/o1": () => json(200, { id: "o1", name: "Acme" }),
    "GET /v1/apps": () => json(200, { items: [{ ...DETAIL, connection_count: 0 }], next_cursor: null }),
    "GET /v1/apps/github": () => json(200, DETAIL),
    "GET /v1/apps/github/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/apps/github/connect-plan": () => json(200, { app_id: "github", available: true, suggested_method: "token", methods: DETAIL.connect }),
    "GET /v1/workspaces/w1/oauth-clients": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/accounts": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Daneel", handle: "daneel", kind: "hosted" }], next_cursor: null }),
    "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
    ...extra,
  };
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return { fn, planGets: () => planGets };
}

beforeEach(() => {
  // jsdom has no `matchMedia`; the journey's sheet asks whether the viewport is wide.
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("min-width"), media: query, onchange: null, addEventListener: () => undefined, removeEventListener: () => undefined,
    addListener: () => undefined, removeListener: () => undefined, dispatchEvent: () => false,
  }));
  vi.stubGlobal("BroadcastChannel", class {
    static open = new Set<never>();
    onmessage: ((e: { data: unknown }) => void) | null = null;
    postMessage() {}
    close() {}
  });
  vi.stubGlobal("open", vi.fn(() => ({ closed: false, opener: window, location: { replace: vi.fn() }, focus: vi.fn(), close: vi.fn() })));
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const openPlan = (extra: Record<string, Handler> = {}) => {
  const stub = stubFetch(extra);
  wrap(<NewAgentFromTemplateView />);
  return stub;
};

/** Fills About with valid answers (leaving `over` to override) and goes to Accounts. */
const gotoAccounts = async (name = "Rex", repo = "acme/town") => {
  await screen.findByLabelText("Repository");
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: name } });
  if (repo !== null) fireEvent.change(screen.getByLabelText("Repository"), { target: { value: repo } });
  fireEvent.click(screen.getByRole("button", { name: "Next" }));
};

describe("the four-step page (C-1116)", () => {
  it("the fold holds the inert README above the steps, and About asks name, handle and the inputs", async () => {
    openPlan();
    await screen.findByLabelText("Repository");
    const fold = screen.getByTestId("template-fold") as HTMLDetailsElement;
    expect(fold.open).toBe(false);
    const readme = within(fold).getByTestId("template-readme");
    expect(readme.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(readme.querySelector("img")).toBeNull();
    expect(readme.querySelector("a[href^='javascript:']")).toBeNull();
    const steps = screen.getByRole("list", { name: "New agent steps" });
    const items = within(steps).getAllByRole("listitem");
    expect(items.map((l) => l.textContent)).toEqual(["About", "Accounts"]);
    expect(items[0]).toHaveAttribute("aria-current", "step");
    expect(items[1].getAttribute("aria-current")).toBeNull();
    expect(screen.getByLabelText("Name")).toBeTruthy();
    expect(screen.getByLabelText("Handle (optional)")).toBeTruthy();
    expect(screen.getByLabelText("Repository")).toBeTruthy();
    // A choice input starts at the manifest's default.
    expect((screen.getByLabelText("Region") as HTMLSelectElement).value).toBe("eu");
    // Crumbs lead back to the agents page.
    expect(screen.getByRole("link", { name: "Agents" })).toBeTruthy();
  });

  it("Next refuses an empty name, a bad handle and an empty required input; then it goes on", async () => {
    openPlan();
    await screen.findByLabelText("Repository");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Name must be 1 to 80 characters.")).toBeTruthy();
    expect(screen.getByText("This is needed.")).toBeTruthy(); // the required Repository input
    expect(screen.getAllByRole("listitem")[1]!.getAttribute("aria-current")).toBeNull();

    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Rex" } });
    fireEvent.change(screen.getByLabelText("Repository"), { target: { value: "acme/town" } });
    fireEvent.change(screen.getByLabelText("Handle (optional)"), { target: { value: "Rex!" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Use lowercase letters, digits and dashes, starting with a letter (max 40).")).toBeTruthy();

    fireEvent.change(screen.getByLabelText("Handle (optional)"), { target: { value: "rex" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("It triages pull requests.")).toBeTruthy();
  });

  it("Accounts asks each entry: the first existing account is recommended, the access is preselected, optional entries can be skipped", async () => {
    openPlan();
    await gotoAccounts();
    await screen.findByText("It triages pull requests.");
    const githubEntry = screen.getByRole("region", { name: "It triages pull requests." });
    expect(within(githubEntry).getByRole("radio", { name: /Use GitHub · bob-builder/ })).toBeChecked();
    // Two badges: the first existing account, and the Read only card of the access (its plan level is ask).
    expect(within(githubEntry).getAllByText("Recommended").length).toBe(2);
    expect(within(githubEntry).queryByRole("radio", { name: /Skip GitHub/ })).toBeNull(); // a required entry cannot be skipped
    expect(within(githubEntry).getByRole("radio", { name: /Ask before changes/ })).toBeChecked(); // the plan's level
    expect(within(githubEntry).getByRole("radio", { name: /Read only/ })).toBeTruthy();

    const linearEntry = screen.getByRole("region", { name: "It files cards." });
    expect(within(linearEntry).getByRole("radio", { name: /Skip Linear/ })).toBeTruthy();
    fireEvent.click(within(linearEntry).getByRole("radio", { name: /Skip Linear/ }));
    expect(within(linearEntry).getByRole("radio", { name: /Skip Linear/ })).toBeChecked();
    // Leaving the access alone changes nothing: the level is sent only when the person changes it.
    expect(within(githubEntry).getByRole("radio", { name: /Read only/ })).toBeTruthy();
  });

  it("a custom_only account says the organization asks before each change, with the tool lists", async () => {
    const customPlan = {
      ...plan,
      entries: [
        {
          index: 0, required: true, why: "It triages pull requests.",
          choices: [choice({ level: "ask", level_note: "custom_only", custom: { allow_tools: ["read_issues"], ask_tools: ["create_issue"], dropped_tools: ["delete_issue"] } })],
        },
      ],
    };
    stubFetch({ "GET /v1/workspaces/w1/agent-templates/schedule-manager": () => json(200, customPlan) });
    wrap(<NewAgentFromTemplateView />);
    await gotoAccounts();
    await screen.findByText("It triages pull requests.");
    expect(screen.getByText("The organization limits this app: it asks you before each change.")).toBeTruthy();
    expect(screen.getByText(/Allowed without asking:/)).toHaveTextContent("read_issues");
    expect(screen.getByText(/Needs your approval:/)).toHaveTextContent("create_issue");
    expect(screen.getByText(/Not available here:/)).toHaveTextContent("delete_issue");
    expect(screen.queryByRole("radio", { name: /Ask before changes/ })).toBeNull(); // no access to choose
  });

  it("an app that is not available says so instead of asking for an account", async () => {
    const downPlan = {
      ...plan,
      entries: [{ index: 0, required: true, why: "It triages pull requests.", choices: [choice({ available: false, accounts: [], unavailable_reason: "not_installed" })] }],
    };
    stubFetch({ "GET /v1/workspaces/w1/agent-templates/schedule-manager": () => json(200, downPlan) });
    wrap(<NewAgentFromTemplateView />);
    await gotoAccounts();
    expect(await screen.findByText("Needs GitHub, which isn't installed here.")).toBeTruthy();
  });

  it("Connect a new account opens the account-only journey; closing it refetches the plan", async () => {
    const stub = openPlan();
    await gotoAccounts();
    const githubEntry = await screen.findByRole("region", { name: "It triages pull requests." });
    const before = stub.planGets();
    fireEvent.click(within(githubEntry).getByRole("radio", { name: /Connect a new GitHub account/ }));
    const journey = await screen.findByRole("dialog");
    expect(journey.getAttribute("data-testid")).toBe("ws-connect-journey");
    // Straight to Sign in: no Agent or Access step. "Token" names both the method choice and its field.
    expect((await screen.findAllByLabelText("Token")).length).toBeGreaterThanOrEqual(1);
    fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]!);
    await waitFor(() => expect(stub.planGets()).toBe(before + 1));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("Back keeps what was filled in", async () => {
    openPlan();
    await gotoAccounts();
    fireEvent.click(await screen.findByRole("button", { name: "Back" }));
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Rex");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("It triages pull requests.")).toBeTruthy();
  });
});
